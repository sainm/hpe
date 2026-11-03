use std::collections::{HashMap, VecDeque};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures_util::{StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::postgres::{PgArguments, PgConnectOptions, PgConnection};
use sqlx::query::Query;
use sqlx::types::Json;
use sqlx::{AssertSqlSafe, Column, Connection, FromRow, Postgres, Row};
use thiserror::Error;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use time::macros::format_description;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::db::{Db, DbError};
use hpa_local::postgres_connector::{
    ConnectionRecord, ConnectorAuditRecord, ConnectorDefinitionRecord, PostgresConnectorError,
};

pub const CONNECTOR_SCHEMA: &str = "hpa.connector/v1";
pub const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum ConnectorError {
    #[error("{0}")]
    Db(#[from] DbError),
    #[error("{0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Forbidden(String),
    #[error("{0}")]
    RateLimited(String),
    #[error("{0}")]
    Upstream(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectorManifest {
    pub schema_version: String,
    pub id: String,
    pub version: String,
    pub name: String,
    pub kind: ConnectorKind,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub docs: Option<String>,
    #[serde(default)]
    pub auth: Vec<AuthMethod>,
    pub operations: Vec<ConnectorOperation>,
    #[serde(default)]
    pub permissions: Vec<ConnectorPermission>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectorKind {
    Http,
    File,
    Database,
    Git,
    Mq,
    S3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OperationRisk {
    Read,
    Write,
    Destructive,
}

impl OperationRisk {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Destructive => "destructive",
        }
    }

    pub fn requires_approval(self) -> bool {
        !matches!(self, Self::Read)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectorOperation {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub risk: OperationRisk,
    pub input_schema: Value,
    #[serde(default)]
    pub output_schema: Option<Value>,
    #[serde(default)]
    pub rate_limit_per_minute: Option<u32>,
    #[serde(default)]
    pub max_attempts: Option<u32>,
    #[serde(default)]
    pub idempotency: Option<String>,
    #[serde(default)]
    pub http: Option<HttpOperation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpOperation {
    pub method: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthMethod {
    pub kind: String,
    #[serde(default)]
    pub env: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectorPermission {
    pub kind: String,
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(default)]
    pub read: Vec<String>,
    #[serde(default)]
    pub write: Vec<String>,
}

#[derive(Debug, Clone, FromRow)]
pub struct ConnectorDefinitionRow {
    pub connector_id: String,
    pub version: String,
    pub name: String,
    pub kind: String,
    pub manifest_json: String,
    pub plugin_id: Option<String>,
    pub plugin_version: Option<String>,
    pub status: String,
    pub created_at: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct ConnectionRow {
    pub id: String,
    pub name: String,
    pub connector_id: String,
    pub connector_version: String,
    pub config_json: String,
    pub credential_env: Option<String>,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorAuditRow {
    pub id: String,
    pub at: String,
    pub connection_id: String,
    pub operation: String,
    pub status: String,
    pub latency_ms: i64,
    #[serde(skip_serializing)]
    pub detail_json: String,
}

#[derive(Debug, Clone)]
pub struct ConnectionInput {
    pub id: Option<String>,
    pub name: String,
    pub connector_id: String,
    pub connector_version: Option<String>,
    pub config: Value,
    pub credential_env: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HttpConnectionConfig {
    base_url: String,
    #[serde(default)]
    allowed_hosts: Vec<String>,
    #[serde(default)]
    auth: HttpAuth,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    max_attempts: Option<u32>,
    #[serde(default)]
    rate_limit_per_minute: Option<u32>,
    #[serde(default)]
    max_response_bytes: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HttpAuth {
    #[serde(default = "default_auth_kind")]
    kind: String,
    #[serde(default)]
    header: Option<String>,
}

impl Default for HttpAuth {
    fn default() -> Self {
        Self {
            kind: "none".to_string(),
            header: None,
        }
    }
}

fn default_auth_kind() -> String {
    "none".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FileConnectionConfig {
    root: String,
    #[serde(default)]
    allow_write: bool,
    #[serde(default)]
    max_file_bytes: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DatabaseConnectionConfig {
    allowed_hosts: Vec<String>,
    #[serde(default = "default_database_timeout_ms")]
    statement_timeout_ms: u64,
    #[serde(default = "default_database_max_rows")]
    max_rows: usize,
    #[serde(default = "default_connector_max_bytes")]
    max_response_bytes: usize,
    #[serde(default)]
    rate_limit_per_minute: Option<u32>,
}

fn default_database_timeout_ms() -> u64 {
    15_000
}

fn default_database_max_rows() -> usize {
    500
}

fn default_connector_max_bytes() -> usize {
    MAX_RESPONSE_BYTES
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GitConnectionConfig {
    repository_root: String,
    #[serde(default)]
    allow_write: bool,
    #[serde(default = "default_git_max_output_bytes")]
    max_output_bytes: usize,
    #[serde(default)]
    rate_limit_per_minute: Option<u32>,
}

fn default_git_max_output_bytes() -> usize {
    MAX_RESPONSE_BYTES
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MqConnectionConfig {
    url: String,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    allowed_hosts: Vec<String>,
    #[serde(default)]
    allowed_channels: Vec<String>,
    #[serde(default)]
    database: u32,
    #[serde(default = "default_mq_timeout_ms")]
    timeout_ms: u64,
    #[serde(default = "default_mq_max_message_bytes")]
    max_message_bytes: usize,
    #[serde(default)]
    rate_limit_per_minute: Option<u32>,
}

fn default_mq_timeout_ms() -> u64 {
    5_000
}

fn default_mq_max_message_bytes() -> usize {
    1024 * 1024
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct S3ConnectionConfig {
    endpoint: String,
    region: String,
    bucket: String,
    #[serde(default)]
    allowed_hosts: Vec<String>,
    #[serde(default = "default_s3_timeout_ms")]
    timeout_ms: u64,
    #[serde(default = "default_s3_max_object_bytes")]
    max_object_bytes: usize,
    #[serde(default)]
    rate_limit_per_minute: Option<u32>,
}

fn default_s3_timeout_ms() -> u64 {
    15_000
}

fn default_s3_max_object_bytes() -> usize {
    2 * 1024 * 1024
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct S3Credentials {
    access_key: String,
    secret_key: String,
    #[serde(default)]
    session_token: Option<String>,
}

#[derive(Clone)]
pub struct ConnectorRuntime {
    pub db: Db,
    http: reqwest::Client,
    limits: Arc<Mutex<HashMap<String, VecDeque<Instant>>>>,
}

impl ConnectorRuntime {
    pub fn new(db: Db) -> Self {
        Self {
            db,
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            limits: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn initialize(&self) -> Result<(), ConnectorError> {
        let http = builtin_http_manifest()?;
        let file = builtin_file_manifest()?;
        let database = builtin_database_manifest()?;
        let git = builtin_git_manifest()?;
        let mq = builtin_mq_manifest()?;
        let s3 = builtin_s3_manifest()?;
        for manifest in [http, file, database, git, mq, s3] {
            self.upsert_definition(&manifest, None, None, "enabled")
                .await?;
        }
        Ok(())
    }

    pub async fn upsert_definition(
        &self,
        manifest: &ConnectorManifest,
        plugin_id: Option<&str>,
        plugin_version: Option<&str>,
        status: &str,
    ) -> Result<bool, ConnectorError> {
        validate_manifest(manifest)?;
        if let Some(store) = self.db.postgres_connector() {
            return store
                .upsert_definition(
                    &serde_json::to_string(manifest)?,
                    &manifest.id,
                    &manifest.version,
                    &manifest.name,
                    connector_kind(manifest.kind),
                    plugin_id,
                    plugin_version,
                    status,
                    &now_rfc3339(),
                )
                .await
                .map_err(pg_connector_error);
        }
        let existing: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT manifest_json, plugin_id FROM connector_definitions
             WHERE connector_id = ? AND version = ?",
        )
        .bind(&manifest.id)
        .bind(&manifest.version)
        .fetch_optional(&self.db.pool)
        .await?;
        if let Some((existing_manifest, existing_plugin)) = existing {
            let previous: Value = serde_json::from_str(&existing_manifest)?;
            let current = serde_json::to_value(manifest)?;
            if existing_plugin.as_deref() != plugin_id {
                return Err(ConnectorError::Forbidden(format!(
                    "connector `{}@{}` is already registered by {}",
                    manifest.id,
                    manifest.version,
                    existing_plugin.as_deref().unwrap_or("a built-in connector")
                )));
            }
            if previous == current {
                if plugin_id.is_some() {
                    sqlx::query(
                        "UPDATE connector_definitions SET plugin_version = ?, status = ?
                         WHERE connector_id = ? AND version = ?",
                    )
                    .bind(plugin_version)
                    .bind(status)
                    .bind(&manifest.id)
                    .bind(&manifest.version)
                    .execute(&self.db.pool)
                    .await?;
                }
                return Ok(false);
            }
            return Err(ConnectorError::Invalid(format!(
                "connector `{}@{}` is frozen with different content; bump the connector version",
                manifest.id, manifest.version
            )));
        }
        sqlx::query(
            "INSERT INTO connector_definitions
             (connector_id, version, name, kind, manifest_json, plugin_id, plugin_version, status, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(connector_id, version) DO UPDATE SET
                name = excluded.name,
                kind = excluded.kind,
                manifest_json = excluded.manifest_json,
                plugin_id = excluded.plugin_id,
                plugin_version = excluded.plugin_version,
                status = excluded.status",
        )
        .bind(&manifest.id)
        .bind(&manifest.version)
        .bind(&manifest.name)
        .bind(connector_kind(manifest.kind))
        .bind(serde_json::to_string(manifest)?)
        .bind(plugin_id)
        .bind(plugin_version)
        .bind(status)
        .bind(now_rfc3339())
        .execute(&self.db.pool)
        .await?;
        Ok(true)
    }

    pub async fn list_definitions(&self) -> Result<Vec<ConnectorDefinitionRow>, ConnectorError> {
        if let Some(store) = self.db.postgres_connector() {
            return store
                .list_definitions()
                .await
                .map(|records| records.into_iter().map(definition_from_pg).collect())
                .map_err(pg_connector_error);
        }
        Ok(sqlx::query_as::<_, ConnectorDefinitionRow>(
            "SELECT connector_id, version, name, kind, manifest_json, plugin_id, plugin_version,
                    status, created_at
             FROM connector_definitions ORDER BY name ASC, version DESC",
        )
        .fetch_all(&self.db.pool)
        .await?)
    }

    pub async fn definition(
        &self,
        connector_id: &str,
        version: Option<&str>,
    ) -> Result<ConnectorDefinitionRow, ConnectorError> {
        if let Some(store) = self.db.postgres_connector() {
            return match store
                .definition(connector_id, version)
                .await
                .map_err(pg_connector_error)?
            {
                Some(record) => Ok(definition_from_pg(record)),
                None => Err(match version {
                    Some(version) => ConnectorError::NotFound(format!(
                        "connector `{connector_id}@{version}` not found"
                    )),
                    None => {
                        ConnectorError::NotFound(format!("connector `{connector_id}` not found"))
                    }
                }),
            };
        }
        match version {
            Some(version) => sqlx::query_as::<_, ConnectorDefinitionRow>(
                "SELECT connector_id, version, name, kind, manifest_json, plugin_id, plugin_version,
                        status, created_at
                 FROM connector_definitions WHERE connector_id = ? AND version = ?",
            )
            .bind(connector_id)
            .bind(version)
            .fetch_optional(&self.db.pool)
            .await?
            .ok_or_else(|| {
                ConnectorError::NotFound(format!("connector `{connector_id}@{version}` not found"))
            }),
            None => sqlx::query_as::<_, ConnectorDefinitionRow>(
                "SELECT connector_id, version, name, kind, manifest_json, plugin_id, plugin_version,
                        status, created_at
                 FROM connector_definitions WHERE connector_id = ? AND status = 'enabled'
                 ORDER BY version DESC LIMIT 1",
            )
            .bind(connector_id)
            .fetch_optional(&self.db.pool)
            .await?
            .ok_or_else(|| ConnectorError::NotFound(format!("connector `{connector_id}` not found"))),
        }
    }

    pub async fn list_connections(&self) -> Result<Vec<ConnectionRow>, ConnectorError> {
        if let Some(store) = self.db.postgres_connector() {
            return store
                .list_connections()
                .await
                .map(|records| records.into_iter().map(connection_from_pg).collect())
                .map_err(pg_connector_error);
        }
        Ok(sqlx::query_as::<_, ConnectionRow>(
            "SELECT id, name, connector_id, connector_version, config_json, credential_env, status,
                    created_at, updated_at
             FROM connections ORDER BY name ASC",
        )
        .fetch_all(&self.db.pool)
        .await?)
    }

    pub async fn connection(&self, id: &str) -> Result<ConnectionRow, ConnectorError> {
        if let Some(store) = self.db.postgres_connector() {
            return match store.connection(id).await.map_err(pg_connector_error)? {
                Some(record) => Ok(connection_from_pg(record)),
                None => Err(ConnectorError::NotFound(format!(
                    "connection `{id}` not found"
                ))),
            };
        }
        sqlx::query_as::<_, ConnectionRow>(
            "SELECT id, name, connector_id, connector_version, config_json, credential_env, status,
                    created_at, updated_at
             FROM connections WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.db.pool)
        .await?
        .ok_or_else(|| ConnectorError::NotFound(format!("connection `{id}` not found")))
    }

    pub async fn save_connection(
        &self,
        input: ConnectionInput,
    ) -> Result<ConnectionRow, ConnectorError> {
        if input.name.trim().is_empty() {
            return Err(ConnectorError::Invalid(
                "connection name is required".to_string(),
            ));
        }
        let definition = self
            .definition(&input.connector_id, input.connector_version.as_deref())
            .await?;
        if definition.status != "enabled" {
            return Err(ConnectorError::Forbidden(format!(
                "connector `{}` is disabled",
                definition.connector_id
            )));
        }
        let credential_env = input
            .credential_env
            .filter(|value| !value.trim().is_empty());
        if let Some(name) = credential_env.as_deref()
            && !valid_env_name(name)
        {
            return Err(ConnectorError::Invalid(format!(
                "credential environment variable `{name}` is invalid"
            )));
        }
        let manifest: ConnectorManifest = serde_json::from_str(&definition.manifest_json)?;
        validate_connection_config(&manifest, &input.config, credential_env.as_deref())?;
        let now = now_rfc3339();
        let id = input
            .id
            .unwrap_or_else(|| format!("conn_{}", Uuid::now_v7()));
        if let Some(store) = self.db.postgres_connector() {
            return store
                .upsert_connection(
                    &id,
                    &input.name,
                    &definition.connector_id,
                    &definition.version,
                    &serde_json::to_string(&input.config)?,
                    credential_env.as_deref(),
                    &now,
                    &now,
                )
                .await
                .map(connection_from_pg)
                .map_err(pg_connector_error);
        }
        let existing: Option<String> =
            sqlx::query_scalar("SELECT id FROM connections WHERE id = ?")
                .bind(&id)
                .fetch_optional(&self.db.pool)
                .await?;
        if existing.is_some() {
            sqlx::query(
                "UPDATE connections SET name = ?, connector_id = ?, connector_version = ?,
                        config_json = ?, credential_env = ?, status = 'active', updated_at = ?
                 WHERE id = ?",
            )
            .bind(&input.name)
            .bind(&definition.connector_id)
            .bind(&definition.version)
            .bind(serde_json::to_string(&input.config)?)
            .bind(&credential_env)
            .bind(&now)
            .bind(&id)
            .execute(&self.db.pool)
            .await?;
        } else {
            sqlx::query(
                "INSERT INTO connections
                 (id, name, connector_id, connector_version, config_json, credential_env, status, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, 'active', ?, ?)",
            )
            .bind(&id)
            .bind(&input.name)
            .bind(&definition.connector_id)
            .bind(&definition.version)
            .bind(serde_json::to_string(&input.config)?)
            .bind(&credential_env)
            .bind(&now)
            .bind(&now)
            .execute(&self.db.pool)
            .await?;
        }
        self.connection(&id).await
    }

    pub async fn delete_connection(&self, id: &str) -> Result<bool, ConnectorError> {
        if let Some(store) = self.db.postgres_connector() {
            return store
                .delete_connection(id)
                .await
                .map_err(pg_connector_error);
        }
        let result = sqlx::query("DELETE FROM connections WHERE id = ?")
            .bind(id)
            .execute(&self.db.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn set_plugin_definitions_status(
        &self,
        plugin_id: &str,
        plugin_version: &str,
        status: &str,
    ) -> Result<(), ConnectorError> {
        if let Some(store) = self.db.postgres_connector() {
            store
                .set_plugin_definitions_status(plugin_id, plugin_version, status)
                .await
                .map_err(pg_connector_error)?;
            return Ok(());
        }
        sqlx::query(
            "UPDATE connector_definitions SET status = ?
             WHERE plugin_id = ? AND plugin_version = ?",
        )
        .bind(status)
        .bind(plugin_id)
        .bind(plugin_version)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_plugin_definitions(&self, plugin_id: &str) -> Result<(), ConnectorError> {
        if let Some(store) = self.db.postgres_connector() {
            store
                .delete_plugin_definitions(plugin_id)
                .await
                .map_err(pg_connector_error)?;
            return Ok(());
        }
        sqlx::query("DELETE FROM connector_definitions WHERE plugin_id = ?")
            .bind(plugin_id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    pub async fn test_connection(&self, id: &str) -> Result<Value, ConnectorError> {
        let connection = self.connection(id).await?;
        let definition = self
            .definition(
                &connection.connector_id,
                Some(&connection.connector_version),
            )
            .await?;
        if definition.status != "enabled" {
            return Err(ConnectorError::Forbidden(format!(
                "connector `{}` is disabled",
                definition.connector_id
            )));
        }
        let manifest: ConnectorManifest = serde_json::from_str(&definition.manifest_json)?;
        let started = Instant::now();
        let result = match manifest.kind {
            ConnectorKind::Http => self.test_http(&connection).await,
            ConnectorKind::File => self.test_file(&connection).await,
            ConnectorKind::Database => self.test_database(&connection).await,
            ConnectorKind::Git => self.test_git(&connection).await,
            ConnectorKind::Mq => self.test_mq(&connection).await,
            ConnectorKind::S3 => self.test_s3(&connection).await,
        };
        let latency = started.elapsed().as_millis() as i64;
        let (status, detail) = match &result {
            Ok(value) => ("succeeded", audit_summary(value)),
            Err(error) => ("failed", json!({"error": error.to_string()})),
        };
        self.audit(&connection.id, "test", status, latency, detail)
            .await?;
        result
    }

    pub async fn invoke(
        &self,
        connection_id: &str,
        operation_name: &str,
        arguments: Value,
    ) -> Result<Value, ConnectorError> {
        let connection = self.connection(connection_id).await?;
        let definition = self
            .definition(
                &connection.connector_id,
                Some(&connection.connector_version),
            )
            .await?;
        if definition.status != "enabled" {
            return Err(ConnectorError::Forbidden(format!(
                "connector `{}` is disabled",
                definition.connector_id
            )));
        }
        let manifest: ConnectorManifest = serde_json::from_str(&definition.manifest_json)?;
        let operation = manifest
            .operations
            .iter()
            .find(|operation| operation.name == operation_name)
            .ok_or_else(|| {
                ConnectorError::NotFound(format!(
                    "operation `{operation_name}` is not defined by connector `{}`",
                    manifest.id
                ))
            })?;
        if !arguments.is_object() {
            return Err(ConnectorError::Invalid(
                "connector arguments must be an object".to_string(),
            ));
        }
        if jsonschema::validator_for(&operation.input_schema)
            .map_err(|error| ConnectorError::Invalid(format!("invalid input schema: {error}")))?
            .validate(&arguments)
            .is_err()
        {
            return Err(ConnectorError::Invalid(
                "arguments do not match the operation input schema".to_string(),
            ));
        }
        let connection_limit = match manifest.kind {
            ConnectorKind::Http => {
                let config: HttpConnectionConfig = serde_json::from_str(&connection.config_json)?;
                config.rate_limit_per_minute
            }
            ConnectorKind::File => None,
            ConnectorKind::Database => {
                let config: DatabaseConnectionConfig =
                    serde_json::from_str(&connection.config_json)?;
                config.rate_limit_per_minute
            }
            ConnectorKind::Git => {
                let config: GitConnectionConfig = serde_json::from_str(&connection.config_json)?;
                config.rate_limit_per_minute
            }
            ConnectorKind::Mq => {
                let config: MqConnectionConfig = serde_json::from_str(&connection.config_json)?;
                config.rate_limit_per_minute
            }
            ConnectorKind::S3 => {
                let config: S3ConnectionConfig = serde_json::from_str(&connection.config_json)?;
                config.rate_limit_per_minute
            }
        };
        self.check_rate_limit(
            connection_id,
            operation_name,
            connection_limit.or(operation.rate_limit_per_minute),
        )
        .await?;
        let started = Instant::now();
        let result = match manifest.kind {
            ConnectorKind::Http => {
                self.invoke_http(&connection, &manifest, operation, &arguments)
                    .await
            }
            ConnectorKind::File => self.invoke_file(&connection, operation, &arguments).await,
            ConnectorKind::Database => {
                self.invoke_database(&connection, operation, &arguments)
                    .await
            }
            ConnectorKind::Git => self.invoke_git(&connection, operation, &arguments).await,
            ConnectorKind::Mq => self.invoke_mq(&connection, operation, &arguments).await,
            ConnectorKind::S3 => self.invoke_s3(&connection, operation, &arguments).await,
        };
        let latency = started.elapsed().as_millis() as i64;
        let (status, detail) = match &result {
            Ok(value) => ("succeeded", audit_summary(value)),
            Err(error) => ("failed", json!({"error": error.to_string()})),
        };
        self.audit(&connection.id, operation_name, status, latency, detail)
            .await?;
        result
    }

    pub async fn list_audit(&self, limit: i64) -> Result<Vec<Value>, ConnectorError> {
        if let Some(store) = self.db.postgres_connector() {
            let rows = store.list_audit(limit).await.map_err(pg_connector_error)?;
            return Ok(rows
                .into_iter()
                .map(|row| {
                    json!({
                        "id": row.id,
                        "at": row.at,
                        "connectionId": row.connection_id,
                        "operation": row.operation,
                        "status": row.status,
                        "latencyMs": row.latency_ms,
                        "detail": serde_json::from_str::<Value>(&row.detail_json).unwrap_or(Value::Null),
                    })
                })
                .collect());
        }
        let rows = sqlx::query_as::<_, ConnectorAuditRow>(
            "SELECT id, at, connection_id, operation, status, latency_ms, detail_json
             FROM connector_audit ORDER BY at DESC LIMIT ?",
        )
        .bind(limit.clamp(1, 200))
        .fetch_all(&self.db.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| {
                json!({
                    "id": row.id,
                    "at": row.at,
                    "connectionId": row.connection_id,
                    "operation": row.operation,
                    "status": row.status,
                    "latencyMs": row.latency_ms,
                    "detail": serde_json::from_str::<Value>(&row.detail_json).unwrap_or(Value::Null),
                })
            })
            .collect())
    }

    pub async fn tool_catalog(&self) -> Result<Vec<Value>, ConnectorError> {
        let connections = self.list_connections().await?;
        let mut tools = Vec::new();
        for connection in connections {
            let Ok(definition) = self
                .definition(
                    &connection.connector_id,
                    Some(&connection.connector_version),
                )
                .await
            else {
                continue;
            };
            if definition.status != "enabled" {
                continue;
            }
            let manifest: ConnectorManifest = serde_json::from_str(&definition.manifest_json)?;
            for operation in manifest.operations {
                tools.push(json!({
                    "tool": format!("connector.{}.{}", connection.id, operation.name),
                    "risk": "high",
                    "kind": "connector",
                    "label": format!("{} / {}", connection.name, operation.name),
                    "connectionId": connection.id,
                    "connectorId": connection.connector_id,
                    "operation": operation.name,
                    "operationRisk": operation.risk.as_str(),
                    "description": operation.description,
                    "inputSchema": operation.input_schema,
                }));
            }
        }
        Ok(tools)
    }

    pub async fn operation_schema(
        &self,
        connection_id: &str,
        operation_name: &str,
    ) -> Result<Option<Value>, ConnectorError> {
        let connection = self.connection(connection_id).await?;
        let definition = self
            .definition(
                &connection.connector_id,
                Some(&connection.connector_version),
            )
            .await?;
        let manifest: ConnectorManifest = serde_json::from_str(&definition.manifest_json)?;
        Ok(manifest
            .operations
            .into_iter()
            .find(|operation| operation.name == operation_name)
            .map(|operation| operation.input_schema))
    }

    async fn test_http(&self, connection: &ConnectionRow) -> Result<Value, ConnectorError> {
        let config: HttpConnectionConfig = serde_json::from_str(&connection.config_json)?;
        let url = validate_http_url(&config.base_url, &config.allowed_hosts).await?;
        let mut request = self.http.get(url).timeout(Duration::from_secs(5));
        request = self.apply_auth(request, connection, &config.auth)?;
        let response = request
            .send()
            .await
            .map_err(|error| ConnectorError::Upstream(classify_reqwest(&error)))?;
        Ok(json!({
            "ok": response.status().is_success() || response.status().is_redirection(),
            "status": response.status().as_u16(),
            "kind": "http",
        }))
    }

    async fn test_file(&self, connection: &ConnectionRow) -> Result<Value, ConnectorError> {
        let config: FileConnectionConfig = serde_json::from_str(&connection.config_json)?;
        let root = std::fs::canonicalize(&config.root).map_err(|error| {
            ConnectorError::Invalid(format!("cannot open root `{}`: {error}", config.root))
        })?;
        if !root.is_dir() {
            return Err(ConnectorError::Invalid(
                "root must be a directory".to_string(),
            ));
        }
        let writable = config.allow_write
            && !std::fs::metadata(&root)
                .map(|metadata| metadata.permissions().readonly())
                .unwrap_or(true);
        Ok(json!({
            "ok": true,
            "kind": "file",
            "root": root,
            "writable": writable,
        }))
    }

    async fn test_database(&self, connection: &ConnectionRow) -> Result<Value, ConnectorError> {
        let config: DatabaseConnectionConfig = serde_json::from_str(&connection.config_json)?;
        let mut database = connect_database(connection, &config).await?;
        let value: i32 = sqlx::query_scalar("SELECT 1")
            .fetch_one(&mut database)
            .await?;
        Ok(json!({
            "ok": value == 1,
            "kind": "database",
            "database": "postgresql"
        }))
    }

    async fn test_git(&self, connection: &ConnectionRow) -> Result<Value, ConnectorError> {
        let config: GitConnectionConfig = serde_json::from_str(&connection.config_json)?;
        let root = canonical_git_root(&config)?;
        let output = git_command(
            &[
                "-C".to_string(),
                root.to_string_lossy().to_string(),
                "rev-parse".to_string(),
                "--show-toplevel".to_string(),
            ],
            config.max_output_bytes.min(MAX_RESPONSE_BYTES),
            Duration::from_secs(5),
        )
        .await?;
        Ok(json!({
            "ok": output["exitCode"] == 0,
            "kind": "git",
            "root": output["stdout"].as_str().unwrap_or_default().trim()
        }))
    }

    async fn test_mq(&self, connection: &ConnectionRow) -> Result<Value, ConnectorError> {
        let config: MqConnectionConfig = serde_json::from_str(&connection.config_json)?;
        let mut redis = connect_mq(connection, &config).await?;
        let response = redis_command(&mut redis, &["PING"]).await?;
        Ok(json!({
            "ok": response == RedisReply::Simple("PONG".to_string()),
            "kind": "mq",
            "protocol": "redis"
        }))
    }

    async fn invoke_http(
        &self,
        connection: &ConnectionRow,
        manifest: &ConnectorManifest,
        operation: &ConnectorOperation,
        arguments: &Value,
    ) -> Result<Value, ConnectorError> {
        let config: HttpConnectionConfig = serde_json::from_str(&connection.config_json)?;
        let builtin = manifest.id == "http" && operation.name == "request";
        let method = if builtin {
            arguments
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or("GET")
        } else {
            operation
                .http
                .as_ref()
                .map(|http| http.method.as_str())
                .unwrap_or("POST")
        };
        let method = reqwest::Method::from_bytes(method.to_ascii_uppercase().as_bytes())
            .map_err(|_| ConnectorError::Invalid("invalid HTTP method".to_string()))?;
        if method == reqwest::Method::CONNECT || method == reqwest::Method::TRACE {
            return Err(ConnectorError::Forbidden(
                "HTTP CONNECT and TRACE are not allowed".to_string(),
            ));
        }
        let path = if builtin {
            arguments
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("/")
                .to_string()
        } else {
            operation
                .http
                .as_ref()
                .map(|http| http.path.clone())
                .unwrap_or_else(|| "/".to_string())
        };
        let mut url = validate_http_url(&config.base_url, &config.allowed_hosts).await?;
        let mut base_path = url.path().trim_end_matches('/').to_string();
        let clean_path = path.trim().trim_start_matches('/');
        if clean_path
            .split('/')
            .any(|segment| segment == ".." || segment == ".")
        {
            return Err(ConnectorError::Forbidden(
                "path traversal is not allowed".to_string(),
            ));
        }
        base_path.push('/');
        base_path.push_str(clean_path);
        url.set_path(&base_path);
        let query = if builtin {
            arguments.get("query")
        } else if method == reqwest::Method::GET || method == reqwest::Method::DELETE {
            Some(arguments)
        } else {
            arguments.get("query")
        };
        if let Some(Value::Object(query)) = query {
            let mut pairs = url.query_pairs_mut();
            for (key, value) in query {
                if let Some(value) = value.as_str() {
                    pairs.append_pair(key, value);
                } else if !value.is_null() {
                    pairs.append_pair(key, &value.to_string());
                }
            }
        }
        let timeout_ms = arguments
            .get("timeoutMs")
            .and_then(Value::as_u64)
            .map(|value| value.clamp(100, 60_000))
            .unwrap_or_else(|| config.timeout_ms.unwrap_or(15_000).clamp(100, 60_000));
        let max_bytes = config
            .max_response_bytes
            .unwrap_or(MAX_RESPONSE_BYTES)
            .clamp(1024, MAX_RESPONSE_BYTES);
        let max_attempts = config
            .max_attempts
            .or(operation.max_attempts)
            .unwrap_or(1)
            .clamp(1, 4);
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(Value::Object(custom)) = arguments.get("headers") {
            for (key, value) in custom {
                let Some(value) = value.as_str() else {
                    return Err(ConnectorError::Invalid(format!(
                        "header `{key}` must be a string"
                    )));
                };
                if key.contains('\r')
                    || key.contains('\n')
                    || value.contains('\r')
                    || value.contains('\n')
                {
                    return Err(ConnectorError::Invalid(format!(
                        "header `{key}` contains invalid characters"
                    )));
                }
                let lower = key.to_ascii_lowercase();
                if matches!(
                    lower.as_str(),
                    "host"
                        | "content-length"
                        | "connection"
                        | "transfer-encoding"
                        | "authorization"
                ) {
                    return Err(ConnectorError::Forbidden(format!(
                        "header `{key}` cannot be set by the caller"
                    )));
                }
                let name = reqwest::header::HeaderName::from_bytes(key.as_bytes())
                    .map_err(|_| ConnectorError::Invalid(format!("invalid header `{key}`")))?;
                let value = reqwest::header::HeaderValue::from_str(value).map_err(|_| {
                    ConnectorError::Invalid(format!("invalid header value for `{key}`"))
                })?;
                headers.insert(name, value);
            }
        }
        let body = if builtin {
            arguments.get("body").cloned()
        } else if method == reqwest::Method::GET || method == reqwest::Method::DELETE {
            None
        } else {
            Some(
                arguments
                    .get("body")
                    .cloned()
                    .unwrap_or_else(|| arguments.clone()),
            )
        };
        let idempotency_key = arguments
            .get("idempotencyKey")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                operation
                    .risk
                    .requires_approval()
                    .then(|| format!("idem_{}", Uuid::now_v7()))
            });
        let auth = config.auth.clone();
        let mut last_error = None;
        for attempt in 0..max_attempts {
            let mut request = self
                .http
                .request(method.clone(), url.clone())
                .timeout(Duration::from_millis(timeout_ms))
                .headers(headers.clone());
            request = self.apply_auth(request, connection, &auth)?;
            if let Some(body) = &body {
                request = request.json(body);
            }
            if let Some(key) = &idempotency_key {
                request = request.header("Idempotency-Key", key);
            }
            match request.send().await {
                Ok(response) => {
                    let status = response.status();
                    let retryable = status.as_u16() == 429 || status.is_server_error();
                    if retryable && attempt + 1 < max_attempts {
                        let delay = retry_delay(&response);
                        last_error = Some(format!("upstream returned HTTP {status}"));
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    let content_type = response
                        .headers()
                        .get(reqwest::header::CONTENT_TYPE)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    let mut body_bytes = Vec::new();
                    let mut stream = response.bytes_stream();
                    while let Some(chunk) = stream.next().await {
                        let chunk = chunk
                            .map_err(|error| ConnectorError::Upstream(classify_reqwest(&error)))?;
                        if body_bytes.len() + chunk.len() > max_bytes {
                            return Err(ConnectorError::Upstream(format!(
                                "response exceeds {max_bytes} bytes"
                            )));
                        }
                        body_bytes.extend_from_slice(&chunk);
                    }
                    if !status.is_success() {
                        return Err(ConnectorError::Upstream(format!(
                            "upstream returned HTTP {} with {} bytes",
                            status.as_u16(),
                            body_bytes.len()
                        )));
                    }
                    let value = if content_type.contains("json") {
                        let parsed = serde_json::from_slice::<Value>(&body_bytes);
                        match parsed {
                            Ok(value) => value,
                            Err(_) => {
                                Value::String(String::from_utf8_lossy(&body_bytes).to_string())
                            }
                        }
                    } else if body_bytes.is_empty() {
                        Value::Null
                    } else {
                        Value::String(String::from_utf8_lossy(&body_bytes).to_string())
                    };
                    return Ok(json!({
                        "status": status.as_u16(),
                        "contentType": content_type,
                        "body": value,
                        "idempotencyKey": idempotency_key,
                        "attempts": attempt + 1,
                    }));
                }
                Err(error)
                    if attempt + 1 < max_attempts && (error.is_timeout() || error.is_connect()) =>
                {
                    last_error = Some(classify_reqwest(&error));
                    tokio::time::sleep(Duration::from_millis(100 * (attempt as u64 + 1))).await;
                }
                Err(error) => {
                    return Err(ConnectorError::Upstream(classify_reqwest(&error)));
                }
            }
        }
        Err(ConnectorError::Upstream(
            last_error.unwrap_or_else(|| "request failed".to_string()),
        ))
    }

    async fn invoke_file(
        &self,
        connection: &ConnectionRow,
        operation: &ConnectorOperation,
        arguments: &Value,
    ) -> Result<Value, ConnectorError> {
        let config: FileConnectionConfig = serde_json::from_str(&connection.config_json)?;
        let root = std::fs::canonicalize(&config.root).map_err(|error| {
            ConnectorError::Invalid(format!("cannot open root `{}`: {error}", config.root))
        })?;
        let relative = arguments.get("path").and_then(Value::as_str).unwrap_or("");
        let target = resolve_under_root(&root, relative)?;
        match operation.name.as_str() {
            "read" => {
                let canonical = tokio::fs::canonicalize(&target).await.map_err(|error| {
                    ConnectorError::NotFound(format!("file `{relative}` cannot be read: {error}"))
                })?;
                if !canonical.starts_with(&root) {
                    return Err(ConnectorError::Forbidden(
                        "path escapes the connection root".to_string(),
                    ));
                }
                let metadata = tokio::fs::metadata(&canonical).await?;
                if !metadata.is_file() {
                    return Err(ConnectorError::Invalid("path is not a file".to_string()));
                }
                let max_bytes = config
                    .max_file_bytes
                    .unwrap_or(MAX_FILE_BYTES)
                    .min(MAX_FILE_BYTES);
                if metadata.len() > max_bytes as u64 {
                    return Err(ConnectorError::Invalid(format!(
                        "file exceeds {max_bytes} bytes"
                    )));
                }
                let bytes = tokio::fs::read(&canonical).await?;
                if let Ok(text) = String::from_utf8(bytes.clone()) {
                    Ok(json!({
                        "path": relative,
                        "bytes": bytes.len(),
                        "encoding": "utf8",
                        "content": text,
                    }))
                } else {
                    Ok(json!({
                        "path": relative,
                        "bytes": bytes.len(),
                        "encoding": "base64",
                        "content": BASE64.encode(bytes),
                    }))
                }
            }
            "list" => {
                let canonical = tokio::fs::canonicalize(&target).await.map_err(|error| {
                    ConnectorError::NotFound(format!(
                        "directory `{relative}` cannot be read: {error}"
                    ))
                })?;
                if !canonical.starts_with(&root) {
                    return Err(ConnectorError::Forbidden(
                        "path escapes the connection root".to_string(),
                    ));
                }
                let mut entries = Vec::new();
                let mut reader = tokio::fs::read_dir(&canonical).await?;
                while let Some(entry) = reader.next_entry().await? {
                    let metadata = tokio::fs::symlink_metadata(entry.path()).await?;
                    entries.push(json!({
                        "name": entry.file_name().to_string_lossy(),
                        "kind": if metadata.file_type().is_symlink() {
                            "symlink"
                        } else if metadata.is_dir() {
                            "dir"
                        } else if metadata.is_file() {
                            "file"
                        } else {
                            "other"
                        },
                        "size": metadata.len(),
                    }));
                }
                entries.sort_by(|left, right| {
                    left["name"]
                        .as_str()
                        .unwrap_or("")
                        .cmp(right["name"].as_str().unwrap_or(""))
                });
                Ok(json!({"path": relative, "entries": entries}))
            }
            "write" => {
                if !config.allow_write {
                    return Err(ConnectorError::Forbidden(
                        "connection is read-only".to_string(),
                    ));
                }
                let content = arguments
                    .get("content")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ConnectorError::Invalid("content is required".to_string()))?;
                let max_bytes = config
                    .max_file_bytes
                    .unwrap_or(MAX_FILE_BYTES)
                    .min(MAX_FILE_BYTES);
                if content.len() > max_bytes {
                    return Err(ConnectorError::Invalid(format!(
                        "content exceeds {max_bytes} bytes"
                    )));
                }
                let parent = target
                    .parent()
                    .ok_or_else(|| ConnectorError::Invalid("path has no parent".to_string()))?;
                let canonical_parent = ensure_directory(&root, parent).await?;
                if let Ok(metadata) = tokio::fs::symlink_metadata(&target).await
                    && metadata.file_type().is_symlink()
                {
                    return Err(ConnectorError::Forbidden(
                        "symlink targets are not writable".to_string(),
                    ));
                }
                let temp = canonical_parent.join(format!(".hpa-write-{}", Uuid::now_v7()));
                tokio::fs::write(&temp, content.as_bytes()).await?;
                tokio::fs::rename(&temp, &target).await?;
                let mut hasher = Sha256::new();
                hasher.update(content.as_bytes());
                Ok(json!({
                    "path": relative,
                    "bytes": content.len(),
                    "sha256": format!("{:x}", hasher.finalize()),
                }))
            }
            other => Err(ConnectorError::NotFound(format!(
                "file operation `{other}` is not supported"
            ))),
        }
    }

    async fn invoke_database(
        &self,
        connection: &ConnectionRow,
        operation: &ConnectorOperation,
        arguments: &Value,
    ) -> Result<Value, ConnectorError> {
        let config: DatabaseConnectionConfig = serde_json::from_str(&connection.config_json)?;
        let sql = arguments
            .get("sql")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ConnectorError::Invalid("database operation requires sql".to_string())
            })?;
        if sql.len() > 256 * 1024 {
            return Err(ConnectorError::Invalid(
                "database SQL exceeds 256 KiB".to_string(),
            ));
        }
        let parameters = arguments
            .get("parameters")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if parameters.len() > 100 {
            return Err(ConnectorError::Invalid(
                "database parameters exceed 100 values".to_string(),
            ));
        }
        let timeout_ms = arguments
            .get("timeoutMs")
            .and_then(Value::as_u64)
            .unwrap_or(config.statement_timeout_ms)
            .clamp(100, 60_000);
        let mut database = connect_database(connection, &config).await?;
        let mut transaction = database.begin().await?;
        sqlx::query("SELECT set_config('statement_timeout', $1, true)")
            .bind(format!("{timeout_ms}ms"))
            .execute(&mut *transaction)
            .await?;

        match operation.name.as_str() {
            "query" => {
                if !database_read_statement(sql) {
                    return Err(ConnectorError::Forbidden(
                        "database query must start with SELECT, WITH, SHOW, VALUES or EXPLAIN"
                            .to_string(),
                    ));
                }
                sqlx::query("SET TRANSACTION READ ONLY")
                    .execute(&mut *transaction)
                    .await?;
                let mut query = sqlx::query::<Postgres>(AssertSqlSafe(sql.to_string()));
                for parameter in &parameters {
                    query = bind_postgres_value(query, parameter);
                }
                let mut rows = query.fetch(&mut *transaction);
                let mut values = Vec::new();
                let mut truncated = false;
                while let Some(row) = rows.try_next().await? {
                    if values.len() >= config.max_rows {
                        truncated = true;
                        break;
                    }
                    values.push(postgres_row_json(&row));
                }
                let bytes = serde_json::to_vec(&values)?.len();
                if bytes > config.max_response_bytes.min(MAX_RESPONSE_BYTES) {
                    return Err(ConnectorError::Invalid(format!(
                        "database response exceeds {} bytes",
                        config.max_response_bytes.min(MAX_RESPONSE_BYTES)
                    )));
                }
                drop(rows);
                transaction.commit().await?;
                Ok(json!({
                    "ok": true,
                    "kind": "database",
                    "database": "postgresql",
                    "rows": values,
                    "rowCount": values.len(),
                    "truncated": truncated
                }))
            }
            "execute" => {
                if database_read_statement(sql) {
                    return Err(ConnectorError::Invalid(
                        "read statements must use the query operation".to_string(),
                    ));
                }
                let mut query = sqlx::query::<Postgres>(AssertSqlSafe(sql.to_string()));
                for parameter in &parameters {
                    query = bind_postgres_value(query, parameter);
                }
                let result = query.execute(&mut *transaction).await?;
                transaction.commit().await?;
                Ok(json!({
                    "ok": true,
                    "kind": "database",
                    "database": "postgresql",
                    "rowsAffected": result.rows_affected()
                }))
            }
            other => Err(ConnectorError::Invalid(format!(
                "database operation `{other}` is not supported"
            ))),
        }
    }

    async fn invoke_git(
        &self,
        connection: &ConnectionRow,
        operation: &ConnectorOperation,
        arguments: &Value,
    ) -> Result<Value, ConnectorError> {
        let config: GitConnectionConfig = serde_json::from_str(&connection.config_json)?;
        let root = canonical_git_root(&config)?;
        let output_limit = config.max_output_bytes.min(MAX_RESPONSE_BYTES);
        let timeout = Duration::from_millis(
            arguments
                .get("timeoutMs")
                .and_then(Value::as_u64)
                .unwrap_or(10_000)
                .clamp(100, 30_000),
        );
        let mut args = match operation.name.as_str() {
            "status" => vec![
                "status".to_string(),
                "--porcelain=v1".to_string(),
                "--branch".to_string(),
            ],
            "log" => {
                let limit = arguments
                    .get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(20)
                    .clamp(1, 200);
                let mut args = vec![
                    "log".to_string(),
                    "-n".to_string(),
                    limit.to_string(),
                    "--format=%H%x09%an%x09%aI%x09%s".to_string(),
                ];
                if let Some(revision) = arguments.get("revision").and_then(Value::as_str) {
                    validate_git_revision(revision)?;
                    args.push(revision.to_string());
                }
                args.push("--".to_string());
                args
            }
            "show" => {
                let revision = arguments
                    .get("revision")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        ConnectorError::Invalid("git show requires revision".to_string())
                    })?;
                validate_git_revision(revision)?;
                let mut args = vec![
                    "show".to_string(),
                    "--no-ext-diff".to_string(),
                    "--format=fuller".to_string(),
                    revision.to_string(),
                    "--".to_string(),
                ];
                if let Some(path) = arguments.get("path").and_then(Value::as_str) {
                    args.push(git_relative_path(path)?);
                }
                args
            }
            "diff" => {
                let mut args = vec!["diff".to_string(), "--no-ext-diff".to_string()];
                if arguments.get("stat").and_then(Value::as_bool) == Some(true) {
                    args.push("--stat".to_string());
                }
                for field in ["from", "to"] {
                    if let Some(revision) = arguments.get(field).and_then(Value::as_str) {
                        validate_git_revision(revision)?;
                        args.push(revision.to_string());
                    }
                }
                args.push("--".to_string());
                if let Some(path) = arguments.get("path").and_then(Value::as_str) {
                    args.push(git_relative_path(path)?);
                }
                args
            }
            "ls-files" => {
                let mut args = vec![
                    "ls-files".to_string(),
                    "--cached".to_string(),
                    "--".to_string(),
                ];
                if let Some(path) = arguments.get("path").and_then(Value::as_str) {
                    args.push(git_relative_path(path)?);
                }
                args
            }
            "commit" => {
                if !config.allow_write {
                    return Err(ConnectorError::Forbidden(
                        "git connector write is disabled".to_string(),
                    ));
                }
                let message = arguments
                    .get("message")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        ConnectorError::Invalid("git commit requires message".to_string())
                    })?;
                if message.trim().is_empty() || message.len() > 4096 || message.contains('\0') {
                    return Err(ConnectorError::Invalid(
                        "git commit message must be 1-4096 bytes without NUL".to_string(),
                    ));
                }
                vec!["commit".to_string(), "-m".to_string(), message.to_string()]
            }
            other => {
                return Err(ConnectorError::Invalid(format!(
                    "git operation `{other}` is not supported"
                )));
            }
        };
        args.insert(0, "-C".to_string());
        args.insert(1, root.to_string_lossy().to_string());
        git_command(&args, output_limit, timeout).await
    }

    async fn invoke_mq(
        &self,
        connection: &ConnectionRow,
        operation: &ConnectorOperation,
        arguments: &Value,
    ) -> Result<Value, ConnectorError> {
        let config: MqConnectionConfig = serde_json::from_str(&connection.config_json)?;
        let mut redis = connect_mq(connection, &config).await?;
        match operation.name.as_str() {
            "ping" => {
                let response = redis_command(&mut redis, &["PING"]).await?;
                Ok(json!({
                    "ok": response == RedisReply::Simple("PONG".to_string()),
                    "kind": "mq"
                }))
            }
            "publish" => {
                let channel = arguments
                    .get("channel")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        ConnectorError::Invalid("MQ publish requires channel".to_string())
                    })?;
                if !mq_channel_allowed(channel, &config.allowed_channels) {
                    return Err(ConnectorError::Forbidden(format!(
                        "channel `{channel}` is not allowed"
                    )));
                }
                let message = arguments
                    .get("message")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        ConnectorError::Invalid("MQ publish requires message".to_string())
                    })?;
                if message.len() > config.max_message_bytes {
                    return Err(ConnectorError::Invalid(format!(
                        "MQ message exceeds {} bytes",
                        config.max_message_bytes
                    )));
                }
                let idempotency_key = arguments
                    .get("idempotencyKey")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        ConnectorError::Invalid("MQ publish requires idempotencyKey".to_string())
                    })?;
                if idempotency_key.len() > 256 || idempotency_key.trim().is_empty() {
                    return Err(ConnectorError::Invalid(
                        "MQ idempotencyKey must be 1-256 bytes".to_string(),
                    ));
                }
                let digest = format!(
                    "{:x}",
                    Sha256::digest(format!("{}:{idempotency_key}", connection.id).as_bytes())
                );
                let key = format!("hpa:mq:idem:{digest}");
                let script = "local inserted=redis.call('SET',KEYS[1],'1','NX','EX',ARGV[2]); \
                              if not inserted then return -1 end; \
                              return redis.call('PUBLISH',KEYS[2],ARGV[1])";
                let response = redis_command(
                    &mut redis,
                    &["EVAL", script, "2", &key, channel, message, "86400"],
                )
                .await?;
                let RedisReply::Integer(published) = response else {
                    return Err(ConnectorError::Upstream(
                        "unexpected Redis EVAL response".to_string(),
                    ));
                };
                Ok(json!({
                    "ok": true,
                    "kind": "mq",
                    "channel": channel,
                    "published": published.max(0),
                    "duplicate": published < 0,
                    "idempotencyKey": idempotency_key
                }))
            }
            other => Err(ConnectorError::Invalid(format!(
                "MQ operation `{other}` is not supported"
            ))),
        }
    }

    async fn test_s3(&self, connection: &ConnectionRow) -> Result<Value, ConnectorError> {
        let config: S3ConnectionConfig = serde_json::from_str(&connection.config_json)?;
        validate_s3_endpoint(&config).await?;
        let response = self
            .s3_request(
                connection,
                &config,
                reqwest::Method::GET,
                "",
                vec![("list-type", "2"), ("max-keys", "1")],
                Vec::new(),
                None,
            )
            .await?;
        let status = response.status();
        if !status.is_success() {
            return Err(ConnectorError::Upstream(format!(
                "S3 bucket probe returned HTTP {}",
                status.as_u16()
            )));
        }
        Ok(json!({
            "ok": true,
            "kind": "s3",
            "bucket": config.bucket,
            "region": config.region
        }))
    }

    async fn invoke_s3(
        &self,
        connection: &ConnectionRow,
        operation: &ConnectorOperation,
        arguments: &Value,
    ) -> Result<Value, ConnectorError> {
        let config: S3ConnectionConfig = serde_json::from_str(&connection.config_json)?;
        validate_s3_endpoint(&config).await?;
        match operation.name.as_str() {
            "put" => {
                let key = s3_key(
                    arguments
                        .get("key")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            ConnectorError::Invalid("S3 put requires key".to_string())
                        })?,
                )?;
                let body = arguments
                    .get("body")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ConnectorError::Invalid("S3 put requires body".to_string()))?;
                let bytes = BASE64.decode(body).map_err(|error| {
                    ConnectorError::Invalid(format!("invalid base64 body: {error}"))
                })?;
                if bytes.len() > config.max_object_bytes {
                    return Err(ConnectorError::Invalid(format!(
                        "S3 object exceeds {} bytes",
                        config.max_object_bytes
                    )));
                }
                let content_type = arguments
                    .get("contentType")
                    .and_then(Value::as_str)
                    .unwrap_or("application/octet-stream");
                let response = self
                    .s3_request(
                        connection,
                        &config,
                        reqwest::Method::PUT,
                        &key,
                        Vec::new(),
                        bytes.clone(),
                        Some(content_type),
                    )
                    .await?;
                if !response.status().is_success() {
                    return Err(s3_response_error("put", response).await);
                }
                let etag = response
                    .headers()
                    .get(reqwest::header::ETAG)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string);
                Ok(json!({
                    "ok": true,
                    "kind": "s3",
                    "key": key,
                    "bytes": bytes.len(),
                    "sha256": format!("{:x}", Sha256::digest(&bytes)),
                    "etag": etag
                }))
            }
            "get" => {
                let key = s3_key(
                    arguments
                        .get("key")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            ConnectorError::Invalid("S3 get requires key".to_string())
                        })?,
                )?;
                let response = self
                    .s3_request(
                        connection,
                        &config,
                        reqwest::Method::GET,
                        &key,
                        Vec::new(),
                        Vec::new(),
                        None,
                    )
                    .await?;
                if !response.status().is_success() {
                    return Err(s3_response_error("get", response).await);
                }
                let content_type = response
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("application/octet-stream")
                    .to_string();
                let mut bytes = Vec::new();
                let mut stream = response.bytes_stream();
                while let Some(chunk) = stream.try_next().await.map_err(|error| {
                    ConnectorError::Upstream(format!("S3 response failed: {error}"))
                })? {
                    if bytes.len() + chunk.len() > config.max_object_bytes.min(MAX_RESPONSE_BYTES) {
                        return Err(ConnectorError::Invalid(format!(
                            "S3 object exceeds {} bytes",
                            config.max_object_bytes.min(MAX_RESPONSE_BYTES)
                        )));
                    }
                    bytes.extend_from_slice(&chunk);
                }
                Ok(json!({
                    "ok": true,
                    "kind": "s3",
                    "key": key,
                    "bytes": bytes.len(),
                    "sha256": format!("{:x}", Sha256::digest(&bytes)),
                    "contentType": content_type,
                    "body": BASE64.encode(&bytes)
                }))
            }
            "head" => {
                let key = s3_key(
                    arguments
                        .get("key")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            ConnectorError::Invalid("S3 head requires key".to_string())
                        })?,
                )?;
                let response = self
                    .s3_request(
                        connection,
                        &config,
                        reqwest::Method::HEAD,
                        &key,
                        Vec::new(),
                        Vec::new(),
                        None,
                    )
                    .await?;
                if !response.status().is_success() {
                    return Err(s3_response_error("head", response).await);
                }
                let content_length = response
                    .headers()
                    .get(reqwest::header::CONTENT_LENGTH)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.parse::<u64>().ok())
                    .or_else(|| response.content_length());
                Ok(json!({
                    "ok": true,
                    "kind": "s3",
                    "key": key,
                    "contentLength": content_length,
                    "contentType": response
                        .headers()
                        .get(reqwest::header::CONTENT_TYPE)
                        .and_then(|value| value.to_str().ok()),
                    "etag": response
                        .headers()
                        .get(reqwest::header::ETAG)
                        .and_then(|value| value.to_str().ok())
                }))
            }
            "list" => {
                let prefix = arguments
                    .get("prefix")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                s3_prefix(prefix)?;
                let limit = arguments
                    .get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(100)
                    .clamp(1, 1000);
                let response = self
                    .s3_request(
                        connection,
                        &config,
                        reqwest::Method::GET,
                        "",
                        vec![
                            ("list-type", "2"),
                            ("max-keys", &limit.to_string()),
                            ("prefix", prefix),
                        ],
                        Vec::new(),
                        None,
                    )
                    .await?;
                if !response.status().is_success() {
                    return Err(s3_response_error("list", response).await);
                }
                let body = response.text().await.map_err(|error| {
                    ConnectorError::Upstream(format!("cannot read S3 list response: {error}"))
                })?;
                if body.len() > MAX_RESPONSE_BYTES {
                    return Err(ConnectorError::Invalid(
                        "S3 list response exceeded 2 MiB".to_string(),
                    ));
                }
                let keys = xml_values(&body, "Key");
                Ok(json!({
                    "ok": true,
                    "kind": "s3",
                    "prefix": prefix,
                    "keys": keys,
                    "count": keys.len(),
                    "truncated": xml_values(&body, "IsTruncated")
                        .first()
                        .is_some_and(|value| value == "true")
                }))
            }
            "delete" => {
                let key = s3_key(arguments.get("key").and_then(Value::as_str).ok_or_else(
                    || ConnectorError::Invalid("S3 delete requires key".to_string()),
                )?)?;
                let response = self
                    .s3_request(
                        connection,
                        &config,
                        reqwest::Method::DELETE,
                        &key,
                        Vec::new(),
                        Vec::new(),
                        None,
                    )
                    .await?;
                if !response.status().is_success() {
                    return Err(s3_response_error("delete", response).await);
                }
                Ok(json!({"ok": true, "kind": "s3", "key": key, "deleted": true}))
            }
            other => Err(ConnectorError::Invalid(format!(
                "S3 operation `{other}` is not supported"
            ))),
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn s3_request(
        &self,
        connection: &ConnectionRow,
        config: &S3ConnectionConfig,
        method: reqwest::Method,
        key: &str,
        mut query: Vec<(&str, &str)>,
        body: Vec<u8>,
        content_type: Option<&str>,
    ) -> Result<reqwest::Response, ConnectorError> {
        query.sort_unstable();
        let mut url = s3_url(config, key)?;
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        let headers = sign_s3_request(connection, config, &method, &url, &body, content_type)?;
        let mut request = self
            .http
            .request(method, url)
            .timeout(Duration::from_millis(config.timeout_ms));
        for (name, value) in headers {
            request = request.header(name, value);
        }
        request
            .body(body)
            .send()
            .await
            .map_err(|error| ConnectorError::Upstream(classify_reqwest(&error)))
    }

    fn apply_auth(
        &self,
        request: reqwest::RequestBuilder,
        connection: &ConnectionRow,
        auth: &HttpAuth,
    ) -> Result<reqwest::RequestBuilder, ConnectorError> {
        if auth.kind == "none" {
            return Ok(request);
        }
        let Some(env_name) = connection.credential_env.as_deref() else {
            return Err(ConnectorError::Forbidden(
                "connection has no credential environment variable".to_string(),
            ));
        };
        let token = std::env::var(env_name).map_err(|_| {
            ConnectorError::Forbidden(format!(
                "credential environment variable `{env_name}` is not set"
            ))
        })?;
        match auth.kind.as_str() {
            "bearer" => Ok(request.bearer_auth(token)),
            "header" => {
                let header = auth.header.as_deref().ok_or_else(|| {
                    ConnectorError::Invalid("auth.header is required".to_string())
                })?;
                Ok(request.header(header, token))
            }
            other => Err(ConnectorError::Invalid(format!(
                "unsupported auth kind `{other}`"
            ))),
        }
    }

    async fn check_rate_limit(
        &self,
        connection_id: &str,
        operation: &str,
        configured: Option<u32>,
    ) -> Result<(), ConnectorError> {
        let limit = configured.unwrap_or(60).max(1) as usize;
        let key = format!("{connection_id}:{operation}");
        let now = Instant::now();
        let mut limits = self.limits.lock().await;
        let window = limits.entry(key).or_default();
        while window
            .front()
            .is_some_and(|time| now.duration_since(*time) > Duration::from_secs(60))
        {
            window.pop_front();
        }
        if window.len() >= limit {
            return Err(ConnectorError::RateLimited(format!(
                "connector rate limit {limit}/minute exceeded"
            )));
        }
        window.push_back(now);
        Ok(())
    }

    async fn audit(
        &self,
        connection_id: &str,
        operation: &str,
        status: &str,
        latency_ms: i64,
        detail: Value,
    ) -> Result<(), ConnectorError> {
        if let Some(store) = self.db.postgres_connector() {
            let record = ConnectorAuditRecord {
                id: format!("connaudit_{}", Uuid::now_v7()),
                at: now_rfc3339(),
                connection_id: connection_id.to_string(),
                operation: operation.to_string(),
                status: status.to_string(),
                latency_ms,
                detail_json: serde_json::to_string(&redact_detail(detail))?,
            };
            return store
                .insert_audit(&record)
                .await
                .map_err(pg_connector_error);
        }
        sqlx::query(
            "INSERT INTO connector_audit
             (id, at, connection_id, operation, status, latency_ms, detail_json)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(format!("connaudit_{}", Uuid::now_v7()))
        .bind(now_rfc3339())
        .bind(connection_id)
        .bind(operation)
        .bind(status)
        .bind(latency_ms)
        .bind(serde_json::to_string(&redact_detail(detail))?)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }
}

fn definition_from_pg(record: ConnectorDefinitionRecord) -> ConnectorDefinitionRow {
    ConnectorDefinitionRow {
        connector_id: record.connector_id,
        version: record.version,
        name: record.name,
        kind: record.kind,
        manifest_json: record.manifest_json,
        plugin_id: record.plugin_id,
        plugin_version: record.plugin_version,
        status: record.status,
        created_at: record.created_at,
    }
}

fn connection_from_pg(record: ConnectionRecord) -> ConnectionRow {
    ConnectionRow {
        id: record.id,
        name: record.name,
        connector_id: record.connector_id,
        connector_version: record.connector_version,
        config_json: record.config_json,
        credential_env: record.credential_env,
        status: record.status,
        created_at: record.created_at,
        updated_at: record.updated_at,
    }
}

fn pg_connector_error(error: PostgresConnectorError) -> ConnectorError {
    match error {
        PostgresConnectorError::Database(error) => ConnectorError::Sqlx(error),
        PostgresConnectorError::Json(error) => ConnectorError::Json(error),
        PostgresConnectorError::Frozen(message) => ConnectorError::Invalid(message),
        PostgresConnectorError::Owned(message) => ConnectorError::Forbidden(message),
        PostgresConnectorError::Invalid(message) => ConnectorError::Invalid(message),
    }
}

pub fn definition_value(row: &ConnectorDefinitionRow) -> Value {
    let manifest: Value = serde_json::from_str(&row.manifest_json).unwrap_or(Value::Null);
    json!({
        "connectorId": row.connector_id,
        "version": row.version,
        "name": row.name,
        "kind": row.kind,
        "status": row.status,
        "pluginId": row.plugin_id,
        "pluginVersion": row.plugin_version,
        "createdAt": row.created_at,
        "manifest": manifest,
    })
}

pub fn connection_value(row: &ConnectionRow, definition: Option<&ConnectorDefinitionRow>) -> Value {
    json!({
        "id": row.id,
        "name": row.name,
        "connectorId": row.connector_id,
        "connectorVersion": row.connector_version,
        "config": serde_json::from_str::<Value>(&row.config_json).unwrap_or(Value::Null),
        "credentialEnv": row.credential_env,
        "hasCredential": row.credential_env.is_some(),
        "status": row.status,
        "operations": definition
            .and_then(|row| serde_json::from_str::<ConnectorManifest>(&row.manifest_json).ok())
            .map(|manifest| manifest.operations.iter().map(|operation| json!({
                "name": operation.name,
                "description": operation.description,
                "risk": operation.risk.as_str(),
                "inputSchema": operation.input_schema,
            })).collect::<Vec<_>>())
            .unwrap_or_default(),
        "createdAt": row.created_at,
        "updatedAt": row.updated_at,
    })
}

pub fn validate_manifest(manifest: &ConnectorManifest) -> Result<(), ConnectorError> {
    let mut issues = Vec::new();
    if manifest.schema_version != CONNECTOR_SCHEMA {
        issues.push(format!("schemaVersion must be `{CONNECTOR_SCHEMA}`"));
    }
    if manifest.id.trim().is_empty() {
        issues.push("id is required".to_string());
    }
    if manifest.name.trim().is_empty() {
        issues.push("name is required".to_string());
    }
    if semver::Version::parse(&manifest.version).is_err() {
        issues.push("version must be semver".to_string());
    }
    if manifest.operations.is_empty() {
        issues.push("at least one operation is required".to_string());
    }
    let mut names = std::collections::BTreeSet::new();
    for operation in &manifest.operations {
        if !names.insert(&operation.name) {
            issues.push(format!("duplicate operation `{}`", operation.name));
        }
        if !operation.input_schema.is_object() {
            issues.push(format!(
                "operation `{}` inputSchema must be an object",
                operation.name
            ));
        }
        match manifest.kind {
            ConnectorKind::Http => {
                let Some(http) = &operation.http else {
                    issues.push(format!(
                        "operation `{}` requires http method/path",
                        operation.name
                    ));
                    continue;
                };
                if !matches!(
                    http.method.to_ascii_uppercase().as_str(),
                    "GET" | "POST" | "PUT" | "PATCH" | "DELETE"
                ) {
                    issues.push(format!(
                        "operation `{}` has an unsupported method",
                        operation.name
                    ));
                }
                if http.path.contains("..") {
                    issues.push(format!(
                        "operation `{}` path cannot contain `..`",
                        operation.name
                    ));
                }
            }
            ConnectorKind::S3 => {
                if !matches!(
                    operation.name.as_str(),
                    "put" | "get" | "head" | "list" | "delete"
                ) {
                    issues.push(format!(
                        "S3 connector operation `{}` is not supported",
                        operation.name
                    ));
                }
                if operation.http.is_some() {
                    issues.push(format!(
                        "S3 connector operation `{}` cannot declare http",
                        operation.name
                    ));
                }
            }
            ConnectorKind::Mq => {
                if !matches!(operation.name.as_str(), "ping" | "publish") {
                    issues.push(format!(
                        "MQ connector operation `{}` must be ping/publish",
                        operation.name
                    ));
                }
                if operation.http.is_some() {
                    issues.push(format!(
                        "MQ connector operation `{}` cannot declare http",
                        operation.name
                    ));
                }
            }
            ConnectorKind::File => {
                if !matches!(operation.name.as_str(), "read" | "list" | "write") {
                    issues.push(format!(
                        "file connector operation `{}` must be read/list/write",
                        operation.name
                    ));
                }
                if operation.http.is_some() {
                    issues.push(format!(
                        "file connector operation `{}` cannot declare http",
                        operation.name
                    ));
                }
            }
            ConnectorKind::Database => {
                if !matches!(operation.name.as_str(), "query" | "execute") {
                    issues.push(format!(
                        "database connector operation `{}` must be query/execute",
                        operation.name
                    ));
                }
                if operation.http.is_some() {
                    issues.push(format!(
                        "database connector operation `{}` cannot declare http",
                        operation.name
                    ));
                }
            }
            ConnectorKind::Git => {
                if !matches!(
                    operation.name.as_str(),
                    "status" | "log" | "show" | "diff" | "ls-files" | "commit"
                ) {
                    issues.push(format!(
                        "git connector operation `{}` is not supported",
                        operation.name
                    ));
                }
                if operation.http.is_some() {
                    issues.push(format!(
                        "git connector operation `{}` cannot declare http",
                        operation.name
                    ));
                }
            }
        }
    }
    for permission in &manifest.permissions {
        if !matches!(
            permission.kind.as_str(),
            "network" | "filesystem" | "database"
        ) {
            issues.push(format!("unsupported permission `{}`", permission.kind));
        }
    }
    if issues.is_empty() {
        Ok(())
    } else {
        Err(ConnectorError::Invalid(issues.join("; ")))
    }
}

pub fn builtin_http_manifest() -> Result<ConnectorManifest, ConnectorError> {
    Ok(ConnectorManifest {
        schema_version: CONNECTOR_SCHEMA.to_string(),
        id: "http".to_string(),
        version: "1.0.0".to_string(),
        name: "HTTP".to_string(),
        kind: ConnectorKind::Http,
        description: Some(
            "Allowlisted HTTP requests with local credential references.".to_string(),
        ),
        docs: None,
        auth: vec![
            AuthMethod {
                kind: "bearer".to_string(),
                env: None,
            },
            AuthMethod {
                kind: "header".to_string(),
                env: None,
            },
        ],
        operations: vec![ConnectorOperation {
            name: "request".to_string(),
            description: Some(
                "Send an HTTP request relative to the connection base URL.".to_string(),
            ),
            risk: OperationRisk::Write,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "method": {"type": "string", "enum": ["GET", "POST", "PUT", "PATCH", "DELETE"]},
                    "path": {"type": "string"},
                    "query": {"type": "object"},
                    "headers": {"type": "object"},
                    "body": {},
                    "timeoutMs": {"type": "integer", "minimum": 100, "maximum": 60000},
                    "idempotencyKey": {"type": "string"}
                }
            }),
            output_schema: None,
            rate_limit_per_minute: None,
            max_attempts: None,
            idempotency: Some("optional".to_string()),
            http: Some(HttpOperation {
                method: "GET".to_string(),
                path: "/".to_string(),
            }),
        }],
        permissions: vec![ConnectorPermission {
            kind: "network".to_string(),
            hosts: Vec::new(),
            read: Vec::new(),
            write: Vec::new(),
        }],
    })
}

pub fn builtin_file_manifest() -> Result<ConnectorManifest, ConnectorError> {
    let read_schema = json!({
        "type": "object",
        "properties": {"path": {"type": "string"}},
        "required": ["path"]
    });
    Ok(ConnectorManifest {
        schema_version: CONNECTOR_SCHEMA.to_string(),
        id: "file".to_string(),
        version: "1.0.0".to_string(),
        name: "File".to_string(),
        kind: ConnectorKind::File,
        description: Some(
            "Read, list and optionally write files under one workspace root.".to_string(),
        ),
        docs: None,
        auth: Vec::new(),
        operations: vec![
            ConnectorOperation {
                name: "read".to_string(),
                description: Some("Read one file under the connection root.".to_string()),
                risk: OperationRisk::Read,
                input_schema: read_schema.clone(),
                output_schema: None,
                rate_limit_per_minute: None,
                max_attempts: None,
                idempotency: None,
                http: None,
            },
            ConnectorOperation {
                name: "list".to_string(),
                description: Some("List a directory under the connection root.".to_string()),
                risk: OperationRisk::Read,
                input_schema: json!({
                    "type": "object",
                    "properties": {"path": {"type": "string"}}
                }),
                output_schema: None,
                rate_limit_per_minute: None,
                max_attempts: None,
                idempotency: None,
                http: None,
            },
            ConnectorOperation {
                name: "write".to_string(),
                description: Some("Write a file under the connection root.".to_string()),
                risk: OperationRisk::Write,
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "content": {"type": "string"}
                    },
                    "required": ["path", "content"]
                }),
                output_schema: None,
                rate_limit_per_minute: None,
                max_attempts: None,
                idempotency: None,
                http: None,
            },
        ],
        permissions: vec![ConnectorPermission {
            kind: "filesystem".to_string(),
            hosts: Vec::new(),
            read: vec!["${root}".to_string()],
            write: vec!["${root}".to_string()],
        }],
    })
}

pub fn builtin_database_manifest() -> Result<ConnectorManifest, ConnectorError> {
    let database_schema = json!({
        "type": "object",
        "properties": {
            "sql": {"type": "string", "minLength": 1, "maxLength": 262144},
            "parameters": {"type": "array", "maxItems": 100},
            "timeoutMs": {"type": "integer", "minimum": 100, "maximum": 60000}
        },
        "required": ["sql"],
        "additionalProperties": false
    });
    Ok(ConnectorManifest {
        schema_version: CONNECTOR_SCHEMA.to_string(),
        id: "database".to_string(),
        version: "1.0.0".to_string(),
        name: "PostgreSQL".to_string(),
        kind: ConnectorKind::Database,
        description: Some(
            "PostgreSQL parameterized query and execute connector with read-only query transactions."
                .to_string(),
        ),
        docs: None,
        auth: vec![AuthMethod {
            kind: "env".to_string(),
            env: None,
        }],
        operations: vec![
            ConnectorOperation {
                name: "query".to_string(),
                description: Some("Run one bounded read-only SQL statement.".to_string()),
                risk: OperationRisk::Read,
                input_schema: database_schema.clone(),
                output_schema: None,
                rate_limit_per_minute: Some(60),
                max_attempts: Some(1),
                idempotency: None,
                http: None,
            },
            ConnectorOperation {
                name: "execute".to_string(),
                description: Some("Run one parameterized write SQL statement.".to_string()),
                risk: OperationRisk::Destructive,
                input_schema: database_schema,
                output_schema: None,
                rate_limit_per_minute: Some(20),
                max_attempts: Some(1),
                idempotency: Some("caller-required-for-write".to_string()),
                http: None,
            },
        ],
        permissions: vec![ConnectorPermission {
            kind: "database".to_string(),
            hosts: Vec::new(),
            read: Vec::new(),
            write: Vec::new(),
        }],
    })
}

pub fn builtin_git_manifest() -> Result<ConnectorManifest, ConnectorError> {
    Ok(ConnectorManifest {
        schema_version: CONNECTOR_SCHEMA.to_string(),
        id: "git".to_string(),
        version: "1.0.0".to_string(),
        name: "Git".to_string(),
        kind: ConnectorKind::Git,
        description: Some(
            "Inspect a local Git worktree or bare repository without shell execution.".to_string(),
        ),
        docs: None,
        auth: Vec::new(),
        operations: vec![
            ConnectorOperation {
                name: "status".to_string(),
                description: Some("Show porcelain status.".to_string()),
                risk: OperationRisk::Read,
                input_schema: json!({"type": "object", "additionalProperties": false}),
                output_schema: None,
                rate_limit_per_minute: Some(60),
                max_attempts: Some(1),
                idempotency: None,
                http: None,
            },
            ConnectorOperation {
                name: "log".to_string(),
                description: Some("Show bounded commit history.".to_string()),
                risk: OperationRisk::Read,
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "limit": {"type": "integer", "minimum": 1, "maximum": 200},
                        "revision": {"type": "string", "maxLength": 256}
                    },
                    "additionalProperties": false
                }),
                output_schema: None,
                rate_limit_per_minute: Some(60),
                max_attempts: Some(1),
                idempotency: None,
                http: None,
            },
            ConnectorOperation {
                name: "show".to_string(),
                description: Some("Show one commit or object.".to_string()),
                risk: OperationRisk::Read,
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "revision": {"type": "string", "maxLength": 256},
                        "path": {"type": "string"}
                    },
                    "required": ["revision"],
                    "additionalProperties": false
                }),
                output_schema: None,
                rate_limit_per_minute: Some(30),
                max_attempts: Some(1),
                idempotency: None,
                http: None,
            },
            ConnectorOperation {
                name: "diff".to_string(),
                description: Some("Show a bounded diff.".to_string()),
                risk: OperationRisk::Read,
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "from": {"type": "string", "maxLength": 256},
                        "to": {"type": "string", "maxLength": 256},
                        "path": {"type": "string"},
                        "stat": {"type": "boolean"}
                    },
                    "additionalProperties": false
                }),
                output_schema: None,
                rate_limit_per_minute: Some(30),
                max_attempts: Some(1),
                idempotency: None,
                http: None,
            },
            ConnectorOperation {
                name: "ls-files".to_string(),
                description: Some("List tracked files.".to_string()),
                risk: OperationRisk::Read,
                input_schema: json!({
                    "type": "object",
                    "properties": {"path": {"type": "string"}},
                    "additionalProperties": false
                }),
                output_schema: None,
                rate_limit_per_minute: Some(60),
                max_attempts: Some(1),
                idempotency: None,
                http: None,
            },
            ConnectorOperation {
                name: "commit".to_string(),
                description: Some(
                    "Commit already-staged changes; connection must enable allowWrite.".to_string(),
                ),
                risk: OperationRisk::Write,
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "message": {"type": "string", "minLength": 1, "maxLength": 4096}
                    },
                    "required": ["message"],
                    "additionalProperties": false
                }),
                output_schema: None,
                rate_limit_per_minute: Some(10),
                max_attempts: Some(1),
                idempotency: None,
                http: None,
            },
        ],
        permissions: vec![ConnectorPermission {
            kind: "filesystem".to_string(),
            hosts: Vec::new(),
            read: vec!["${repositoryRoot}".to_string()],
            write: vec!["${repositoryRoot}".to_string()],
        }],
    })
}

pub fn builtin_mq_manifest() -> Result<ConnectorManifest, ConnectorError> {
    Ok(ConnectorManifest {
        schema_version: CONNECTOR_SCHEMA.to_string(),
        id: "mq".to_string(),
        version: "1.0.0".to_string(),
        name: "Redis MQ".to_string(),
        kind: ConnectorKind::Mq,
        description: Some(
            "Allowlisted Redis pub/sub with caller idempotency keys and bounded messages."
                .to_string(),
        ),
        docs: None,
        auth: vec![AuthMethod {
            kind: "env".to_string(),
            env: None,
        }],
        operations: vec![
            ConnectorOperation {
                name: "ping".to_string(),
                description: Some("Check Redis connectivity.".to_string()),
                risk: OperationRisk::Read,
                input_schema: json!({"type": "object", "additionalProperties": false}),
                output_schema: None,
                rate_limit_per_minute: Some(60),
                max_attempts: Some(1),
                idempotency: None,
                http: None,
            },
            ConnectorOperation {
                name: "publish".to_string(),
                description: Some(
                    "Publish one bounded message with a Redis-backed idempotency key.".to_string(),
                ),
                risk: OperationRisk::Write,
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "channel": {"type": "string", "minLength": 1, "maxLength": 512},
                        "message": {"type": "string", "minLength": 1, "maxLength": 1048576},
                        "idempotencyKey": {"type": "string", "minLength": 1, "maxLength": 256}
                    },
                    "required": ["channel", "message", "idempotencyKey"],
                    "additionalProperties": false
                }),
                output_schema: None,
                rate_limit_per_minute: Some(600),
                max_attempts: Some(1),
                idempotency: Some("required".to_string()),
                http: None,
            },
        ],
        permissions: vec![ConnectorPermission {
            kind: "network".to_string(),
            hosts: Vec::new(),
            read: Vec::new(),
            write: Vec::new(),
        }],
    })
}

pub fn builtin_s3_manifest() -> Result<ConnectorManifest, ConnectorError> {
    let key_schema = json!({
        "type": "object",
        "properties": {"key": {"type": "string", "minLength": 1, "maxLength": 1024}},
        "required": ["key"],
        "additionalProperties": false
    });
    Ok(ConnectorManifest {
        schema_version: CONNECTOR_SCHEMA.to_string(),
        id: "s3".to_string(),
        version: "1.0.0".to_string(),
        name: "S3 object storage".to_string(),
        kind: ConnectorKind::S3,
        description: Some(
            "Path-style S3-compatible object access with SigV4, bounded payloads and explicit hosts."
                .to_string(),
        ),
        docs: None,
        auth: vec![AuthMethod {
            kind: "env".to_string(),
            env: None,
        }],
        operations: vec![
            ConnectorOperation {
                name: "put".to_string(),
                description: Some("Put one base64-encoded object.".to_string()),
                risk: OperationRisk::Write,
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "key": {"type": "string", "minLength": 1, "maxLength": 1024},
                        "body": {"type": "string", "maxLength": 2796204},
                        "contentType": {"type": "string", "maxLength": 256}
                    },
                    "required": ["key", "body"],
                    "additionalProperties": false
                }),
                output_schema: None,
                rate_limit_per_minute: Some(120),
                max_attempts: Some(1),
                idempotency: Some("object-key".to_string()),
                http: None,
            },
            ConnectorOperation {
                name: "get".to_string(),
                description: Some("Get one base64-encoded object.".to_string()),
                risk: OperationRisk::Read,
                input_schema: key_schema.clone(),
                output_schema: None,
                rate_limit_per_minute: Some(120),
                max_attempts: Some(1),
                idempotency: None,
                http: None,
            },
            ConnectorOperation {
                name: "head".to_string(),
                description: Some("Read object metadata.".to_string()),
                risk: OperationRisk::Read,
                input_schema: key_schema,
                output_schema: None,
                rate_limit_per_minute: Some(120),
                max_attempts: Some(1),
                idempotency: None,
                http: None,
            },
            ConnectorOperation {
                name: "list".to_string(),
                description: Some("List objects under a prefix.".to_string()),
                risk: OperationRisk::Read,
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "prefix": {"type": "string", "maxLength": 1024},
                        "limit": {"type": "integer", "minimum": 1, "maximum": 1000}
                    },
                    "additionalProperties": false
                }),
                output_schema: None,
                rate_limit_per_minute: Some(60),
                max_attempts: Some(1),
                idempotency: None,
                http: None,
            },
            ConnectorOperation {
                name: "delete".to_string(),
                description: Some("Delete one object.".to_string()),
                risk: OperationRisk::Destructive,
                input_schema: json!({
                    "type": "object",
                    "properties": {"key": {"type": "string", "minLength": 1, "maxLength": 1024}},
                    "required": ["key"],
                    "additionalProperties": false
                }),
                output_schema: None,
                rate_limit_per_minute: Some(30),
                max_attempts: Some(1),
                idempotency: Some("object-key".to_string()),
                http: None,
            },
        ],
        permissions: vec![ConnectorPermission {
            kind: "network".to_string(),
            hosts: Vec::new(),
            read: Vec::new(),
            write: Vec::new(),
        }],
    })
}

fn validate_connection_config(
    manifest: &ConnectorManifest,
    config: &Value,
    credential_env: Option<&str>,
) -> Result<(), ConnectorError> {
    match manifest.kind {
        ConnectorKind::Http => {
            let parsed: HttpConnectionConfig = serde_json::from_value(config.clone())?;
            reqwest::Url::parse(&parsed.base_url)
                .map_err(|error| ConnectorError::Invalid(format!("invalid baseUrl: {error}")))?;
            if !matches!(parsed.auth.kind.as_str(), "none" | "bearer" | "header") {
                return Err(ConnectorError::Invalid(
                    "auth.kind must be none, bearer or header".to_string(),
                ));
            }
            if parsed.auth.kind != "none" && credential_env.is_none() {
                return Err(ConnectorError::Invalid(
                    "credentialEnv is required for bearer/header auth".to_string(),
                ));
            }
            if parsed.auth.kind == "header" && parsed.auth.header.is_none() {
                return Err(ConnectorError::Invalid(
                    "auth.header is required for header auth".to_string(),
                ));
            }
            if parsed
                .allowed_hosts
                .iter()
                .any(|host| !valid_host_rule(host))
            {
                return Err(ConnectorError::Invalid(
                    "allowedHosts contains an invalid host".to_string(),
                ));
            }
        }
        ConnectorKind::File => {
            let parsed: FileConnectionConfig = serde_json::from_value(config.clone())?;
            let root = std::fs::canonicalize(&parsed.root).map_err(|error| {
                ConnectorError::Invalid(format!("cannot open root `{}`: {error}", parsed.root))
            })?;
            if !root.is_dir() {
                return Err(ConnectorError::Invalid(
                    "root must be a directory".to_string(),
                ));
            }
        }
        ConnectorKind::Database => {
            let parsed: DatabaseConnectionConfig = serde_json::from_value(config.clone())?;
            if credential_env.is_none() {
                return Err(ConnectorError::Invalid(
                    "credentialEnv is required for database connections".to_string(),
                ));
            }
            if parsed.allowed_hosts.is_empty()
                || parsed
                    .allowed_hosts
                    .iter()
                    .any(|host| !valid_host_rule(host))
            {
                return Err(ConnectorError::Invalid(
                    "database allowedHosts must be non-empty and valid".to_string(),
                ));
            }
            if !(100..=60_000).contains(&parsed.statement_timeout_ms) {
                return Err(ConnectorError::Invalid(
                    "statementTimeoutMs must be between 100 and 60000".to_string(),
                ));
            }
            if !(1..=10_000).contains(&parsed.max_rows) {
                return Err(ConnectorError::Invalid(
                    "maxRows must be between 1 and 10000".to_string(),
                ));
            }
        }
        ConnectorKind::Git => {
            let parsed: GitConnectionConfig = serde_json::from_value(config.clone())?;
            canonical_git_root(&parsed)?;
            if !(1024..=MAX_RESPONSE_BYTES).contains(&parsed.max_output_bytes) {
                return Err(ConnectorError::Invalid(
                    "git maxOutputBytes must be between 1 KiB and 2 MiB".to_string(),
                ));
            }
        }
        ConnectorKind::Mq => {
            let parsed: MqConnectionConfig = serde_json::from_value(config.clone())?;
            let url = reqwest::Url::parse(&parsed.url)
                .map_err(|error| ConnectorError::Invalid(format!("invalid Redis URL: {error}")))?;
            if url.scheme() != "redis" {
                return Err(ConnectorError::Invalid(
                    "MQ URL must use redis://".to_string(),
                ));
            }
            if !url.username().is_empty() || url.password().is_some() {
                return Err(ConnectorError::Forbidden(
                    "Redis credentials must use credentialEnv".to_string(),
                ));
            }
            if parsed.allowed_hosts.is_empty()
                || parsed
                    .allowed_hosts
                    .iter()
                    .any(|host| !valid_host_rule(host))
            {
                return Err(ConnectorError::Invalid(
                    "MQ allowedHosts must be non-empty and valid".to_string(),
                ));
            }
            if parsed.allowed_channels.is_empty()
                || parsed
                    .allowed_channels
                    .iter()
                    .any(|channel| !valid_mq_channel_pattern(channel))
            {
                return Err(ConnectorError::Invalid(
                    "MQ allowedChannels must be non-empty valid channel patterns".to_string(),
                ));
            }
            if !(100..=60_000).contains(&parsed.timeout_ms) {
                return Err(ConnectorError::Invalid(
                    "MQ timeoutMs must be between 100 and 60000".to_string(),
                ));
            }
            if !(1..=MAX_RESPONSE_BYTES).contains(&parsed.max_message_bytes) {
                return Err(ConnectorError::Invalid(
                    "MQ maxMessageBytes must be between 1 and 2 MiB".to_string(),
                ));
            }
        }
        ConnectorKind::S3 => {
            let parsed: S3ConnectionConfig = serde_json::from_value(config.clone())?;
            let url = reqwest::Url::parse(&parsed.endpoint).map_err(|error| {
                ConnectorError::Invalid(format!("invalid S3 endpoint: {error}"))
            })?;
            if !matches!(url.scheme(), "http" | "https")
                || !url.username().is_empty()
                || url.password().is_some()
                || url.path() != "/"
            {
                return Err(ConnectorError::Invalid(
                    "S3 endpoint must be an HTTP(S) origin without credentials or base path"
                        .to_string(),
                ));
            }
            if credential_env.is_none() {
                return Err(ConnectorError::Invalid(
                    "credentialEnv is required for S3 connections".to_string(),
                ));
            }
            if parsed.bucket.is_empty()
                || parsed.bucket.len() > 63
                || parsed.bucket.contains('/')
                || !parsed.bucket.chars().all(|character| {
                    character.is_ascii_lowercase()
                        || character.is_ascii_digit()
                        || matches!(character, '-' | '.')
                })
            {
                return Err(ConnectorError::Invalid("S3 bucket is invalid".to_string()));
            }
            if parsed.region.is_empty()
                || parsed.region.len() > 64
                || !parsed
                    .region
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-')
            {
                return Err(ConnectorError::Invalid("S3 region is invalid".to_string()));
            }
            if parsed.allowed_hosts.is_empty()
                || parsed
                    .allowed_hosts
                    .iter()
                    .any(|host| host == "*" || !valid_host_rule(host))
            {
                return Err(ConnectorError::Invalid(
                    "S3 allowedHosts must be explicit and valid".to_string(),
                ));
            }
            if !(100..=60_000).contains(&parsed.timeout_ms) {
                return Err(ConnectorError::Invalid(
                    "S3 timeoutMs must be between 100 and 60000".to_string(),
                ));
            }
            if !(1..=MAX_RESPONSE_BYTES).contains(&parsed.max_object_bytes) {
                return Err(ConnectorError::Invalid(
                    "S3 maxObjectBytes must be between 1 and 2 MiB".to_string(),
                ));
            }
        }
    }
    Ok(())
}

async fn validate_http_url(
    raw: &str,
    allowed_hosts: &[String],
) -> Result<reqwest::Url, ConnectorError> {
    let url = reqwest::Url::parse(raw)
        .map_err(|error| ConnectorError::Invalid(format!("invalid URL: {error}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ConnectorError::Invalid(
            "URL must use http or https".to_string(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ConnectorError::Forbidden(
            "URL must not contain userinfo".to_string(),
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| ConnectorError::Invalid("URL has no host".to_string()))?;
    let port = url.port_or_known_default().unwrap_or(80);
    let literal = host.parse::<std::net::IpAddr>().ok();
    if host == "localhost" || literal.is_some_and(|ip| ip.is_loopback()) {
        return Ok(url);
    }
    if !allowed_hosts
        .iter()
        .any(|rule| host_rule_matches(rule, host, port))
    {
        return Err(ConnectorError::Forbidden(format!(
            "host `{host}:{port}` is not in allowedHosts"
        )));
    }
    let addresses = tokio::net::lookup_host((host, port))
        .await
        .map_err(|error| ConnectorError::Upstream(format!("cannot resolve `{host}`: {error}")))?;
    for address in addresses {
        let ip = address.ip();
        if ip.is_loopback() {
            continue;
        }
        let private = match ip {
            std::net::IpAddr::V4(ip) => {
                ip.is_private()
                    || ip.is_link_local()
                    || ip.is_broadcast()
                    || ip.is_unspecified()
                    || ip.is_documentation()
            }
            std::net::IpAddr::V6(ip) => {
                ip.is_unique_local()
                    || ip.is_unicast_link_local()
                    || ip.is_unspecified()
                    || ip.is_multicast()
            }
        };
        if private {
            return Err(ConnectorError::Forbidden(
                "resolved address is private or link-local".to_string(),
            ));
        }
    }
    Ok(url)
}

fn host_rule_matches(rule: &str, host: &str, port: u16) -> bool {
    if rule == "*" {
        return true;
    }
    let (rule_host, rule_port) = match rule.rsplit_once(':') {
        Some((host, port)) if port.parse::<u16>().is_ok() => (host, port.parse::<u16>().ok()),
        _ => (rule, None),
    };
    if rule_port.is_some_and(|rule_port| rule_port != port) {
        return false;
    }
    if let Some(suffix) = rule_host.strip_prefix("*.") {
        host.ends_with(suffix) && host.len() > suffix.len()
    } else {
        rule_host == host
    }
}

async fn ensure_directory(root: &Path, directory: &Path) -> Result<PathBuf, ConnectorError> {
    let relative = directory
        .strip_prefix(root)
        .map_err(|_| ConnectorError::Forbidden("path escapes the connection root".to_string()))?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(ConnectorError::Forbidden(
                "path traversal is not allowed".to_string(),
            ));
        };
        current.push(name);
        match tokio::fs::symlink_metadata(&current).await {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(ConnectorError::Forbidden(
                        "symlink directories are not writable".to_string(),
                    ));
                }
                if !metadata.is_dir() {
                    return Err(ConnectorError::Invalid(
                        "path parent is not a directory".to_string(),
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                tokio::fs::create_dir(&current).await?;
            }
            Err(error) => return Err(ConnectorError::Io(error)),
        }
    }
    let canonical = tokio::fs::canonicalize(directory).await?;
    if !canonical.starts_with(root) {
        return Err(ConnectorError::Forbidden(
            "path escapes the connection root".to_string(),
        ));
    }
    Ok(canonical)
}

async fn connect_database(
    connection: &ConnectionRow,
    config: &DatabaseConnectionConfig,
) -> Result<PgConnection, ConnectorError> {
    let credential_env = connection.credential_env.as_deref().ok_or_else(|| {
        ConnectorError::Invalid("database connector requires credentialEnv".to_string())
    })?;
    let database_url = std::env::var(credential_env).map_err(|_| {
        ConnectorError::Invalid(format!(
            "credential environment variable `{credential_env}` is not set"
        ))
    })?;
    validate_database_url(&database_url, &config.allowed_hosts).await?;
    let options: PgConnectOptions = database_url.parse().map_err(|error| {
        ConnectorError::Invalid(format!("invalid PostgreSQL connection string: {error}"))
    })?;
    PgConnection::connect_with(&options)
        .await
        .map_err(|error| ConnectorError::Upstream(format!("database connection failed: {error}")))
}

async fn validate_database_url(raw: &str, allowed_hosts: &[String]) -> Result<(), ConnectorError> {
    let url = reqwest::Url::parse(raw)
        .map_err(|error| ConnectorError::Invalid(format!("invalid database URL: {error}")))?;
    if !matches!(url.scheme(), "postgres" | "postgresql") {
        return Err(ConnectorError::Invalid(
            "database URL must use postgres or postgresql".to_string(),
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| ConnectorError::Invalid("database URL has no host".to_string()))?;
    let port = url.port_or_known_default().unwrap_or(5432);
    if host != "localhost"
        && host
            .parse::<std::net::IpAddr>()
            .ok()
            .is_none_or(|ip| !ip.is_loopback())
        && !allowed_hosts
            .iter()
            .any(|rule| host_rule_matches(rule, host, port))
    {
        return Err(ConnectorError::Forbidden(format!(
            "database host `{host}:{port}` is not in allowedHosts"
        )));
    }
    let addresses = tokio::net::lookup_host((host, port))
        .await
        .map_err(|error| {
            ConnectorError::Upstream(format!("cannot resolve database host `{host}`: {error}"))
        })?;
    for address in addresses {
        let ip = address.ip();
        let blocked = match ip {
            std::net::IpAddr::V4(ip) => {
                ip.is_link_local()
                    || ip.is_broadcast()
                    || ip.is_unspecified()
                    || ip.is_documentation()
            }
            std::net::IpAddr::V6(ip) => {
                ip.is_unicast_link_local() || ip.is_unspecified() || ip.is_multicast()
            }
        };
        if blocked {
            return Err(ConnectorError::Forbidden(
                "database resolves to a blocked address".to_string(),
            ));
        }
    }
    Ok(())
}

fn database_read_statement(sql: &str) -> bool {
    let trimmed = sql.trim_start();
    if let Some(rest) = trimmed.strip_prefix("/*")
        && let Some((_, rest)) = rest.split_once("*/")
    {
        return database_read_statement(rest);
    }
    let first = trimmed
        .split(|character: char| character.is_whitespace() || character == '(')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    matches!(
        first.as_str(),
        "SELECT" | "WITH" | "SHOW" | "VALUES" | "EXPLAIN"
    )
}

fn bind_postgres_value<'q>(
    query: Query<'q, Postgres, PgArguments>,
    value: &Value,
) -> Query<'q, Postgres, PgArguments> {
    match value {
        Value::Null => query.bind(Option::<String>::None),
        Value::Bool(value) => query.bind(*value),
        Value::Number(value) if value.is_i64() => query.bind(value.as_i64()),
        Value::Number(value) if value.is_u64() => {
            let value = value.as_u64().unwrap_or_default();
            if let Ok(value) = i64::try_from(value) {
                query.bind(value)
            } else {
                query.bind(value.to_string())
            }
        }
        Value::Number(value) => query.bind(value.as_f64()),
        Value::String(value) => query.bind(value.clone()),
        Value::Array(_) | Value::Object(_) => query.bind(Json(value.clone())),
    }
}

fn postgres_row_json(row: &sqlx::postgres::PgRow) -> Value {
    let mut value = serde_json::Map::new();
    for (index, column) in row.columns().iter().enumerate() {
        let decoded = row
            .try_get::<Option<String>, _>(index)
            .map(Value::from)
            .or_else(|_| row.try_get::<Option<i32>, _>(index).map(Value::from))
            .or_else(|_| row.try_get::<Option<i64>, _>(index).map(Value::from))
            .or_else(|_| row.try_get::<Option<f64>, _>(index).map(Value::from))
            .or_else(|_| row.try_get::<Option<bool>, _>(index).map(Value::from))
            .or_else(|_| {
                row.try_get::<Option<Json<Value>>, _>(index)
                    .map(|value| value.map_or(Value::Null, |value| value.0))
            })
            .or_else(|_| {
                row.try_get::<Option<OffsetDateTime>, _>(index)
                    .map(|value| {
                        value.map_or(Value::Null, |value| Value::String(value.to_string()))
                    })
            })
            .or_else(|_| {
                row.try_get::<Option<sqlx::types::Uuid>, _>(index)
                    .map(|value| {
                        value.map_or(Value::Null, |value| Value::String(value.to_string()))
                    })
            })
            .or_else(|_| {
                row.try_get::<Option<Vec<u8>>, _>(index).map(|value| {
                    value.map_or(Value::Null, |value| Value::String(BASE64.encode(value)))
                })
            })
            .unwrap_or(Value::Null);
        value.insert(column.name().to_string(), decoded);
    }
    Value::Object(value)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RedisReply {
    Simple(String),
    Integer(i64),
    Bulk(Option<String>),
    Array(Vec<RedisReply>),
}

async fn connect_mq(
    connection: &ConnectionRow,
    config: &MqConnectionConfig,
) -> Result<BufReader<TcpStream>, ConnectorError> {
    let url = reqwest::Url::parse(&config.url)
        .map_err(|error| ConnectorError::Invalid(format!("invalid Redis URL: {error}")))?;
    if url.scheme() != "redis" {
        return Err(ConnectorError::Invalid(
            "MQ URL must use redis:// (TLS is not yet supported)".to_string(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ConnectorError::Forbidden(
            "Redis URL must not contain credentials; use credentialEnv".to_string(),
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| ConnectorError::Invalid("Redis URL has no host".to_string()))?;
    let port = url.port().unwrap_or(6379);
    if host != "localhost"
        && host
            .parse::<std::net::IpAddr>()
            .ok()
            .is_none_or(|ip| !ip.is_loopback())
        && !config
            .allowed_hosts
            .iter()
            .any(|rule| host_rule_matches(rule, host, port))
    {
        return Err(ConnectorError::Forbidden(format!(
            "Redis host `{host}:{port}` is not in allowedHosts"
        )));
    }
    let addresses = tokio::net::lookup_host((host, port))
        .await
        .map_err(|error| ConnectorError::Upstream(format!("cannot resolve Redis host: {error}")))?
        .collect::<Vec<_>>();
    if addresses.is_empty() {
        return Err(ConnectorError::Upstream(
            "Redis host resolved to no addresses".to_string(),
        ));
    }
    for address in &addresses {
        let blocked = match address.ip() {
            std::net::IpAddr::V4(ip) => ip.is_link_local() || ip.is_unspecified(),
            std::net::IpAddr::V6(ip) => ip.is_unicast_link_local() || ip.is_unspecified(),
        };
        if blocked {
            return Err(ConnectorError::Forbidden(
                "Redis resolves to a blocked address".to_string(),
            ));
        }
    }
    let stream = tokio::time::timeout(
        Duration::from_millis(config.timeout_ms),
        TcpStream::connect(addresses[0]),
    )
    .await
    .map_err(|_| ConnectorError::Upstream("Redis connection timed out".to_string()))??;
    stream.set_nodelay(true)?;
    let mut reader = BufReader::new(stream);
    let password = connection
        .credential_env
        .as_deref()
        .map(|name| {
            std::env::var(name).map_err(|_| {
                ConnectorError::Invalid(format!(
                    "credential environment variable `{name}` is not set"
                ))
            })
        })
        .transpose()?;
    if let Some(password) = password {
        let response = if let Some(username) = config.username.as_deref() {
            redis_command(&mut reader, &["AUTH", username, &password]).await?
        } else {
            redis_command(&mut reader, &["AUTH", &password]).await?
        };
        if response != RedisReply::Simple("OK".to_string()) {
            return Err(ConnectorError::Upstream(
                "Redis authentication failed".to_string(),
            ));
        }
    } else if config.username.is_some() {
        return Err(ConnectorError::Invalid(
            "Redis username requires credentialEnv".to_string(),
        ));
    }
    if config.database != 0 {
        let database = config.database.to_string();
        let response = redis_command(&mut reader, &["SELECT", &database]).await?;
        if response != RedisReply::Simple("OK".to_string()) {
            return Err(ConnectorError::Upstream("Redis SELECT failed".to_string()));
        }
    }
    Ok(reader)
}

async fn redis_command(
    reader: &mut BufReader<TcpStream>,
    args: &[&str],
) -> Result<RedisReply, ConnectorError> {
    let mut encoded = format!("*{}\r\n", args.len());
    for arg in args {
        encoded.push_str(&format!("${}\r\n{arg}\r\n", arg.len()));
    }
    reader.get_mut().write_all(encoded.as_bytes()).await?;
    reader.get_mut().flush().await?;
    read_redis_reply(reader).await
}

async fn read_redis_reply(reader: &mut BufReader<TcpStream>) -> Result<RedisReply, ConnectorError> {
    let prefix = read_redis_byte(reader).await?;
    match prefix {
        b'+' => Ok(RedisReply::Simple(read_redis_line(reader).await?)),
        b'-' => Err(ConnectorError::Upstream(format!(
            "Redis error: {}",
            read_redis_line(reader).await?
        ))),
        b':' => Ok(RedisReply::Integer(
            read_redis_line(reader).await?.parse().map_err(|error| {
                ConnectorError::Upstream(format!("invalid Redis integer: {error}"))
            })?,
        )),
        b'$' => {
            let length: i64 = read_redis_line(reader).await?.parse().map_err(|error| {
                ConnectorError::Upstream(format!("invalid Redis bulk length: {error}"))
            })?;
            if length < 0 {
                return Ok(RedisReply::Bulk(None));
            }
            let length = usize::try_from(length)
                .map_err(|_| ConnectorError::Upstream("Redis bulk length overflow".to_string()))?;
            if length > MAX_RESPONSE_BYTES {
                return Err(ConnectorError::Upstream(
                    "Redis bulk response exceeds 2 MiB".to_string(),
                ));
            }
            let mut bytes = vec![0u8; length + 2];
            reader.read_exact(&mut bytes).await?;
            bytes.truncate(length);
            Ok(RedisReply::Bulk(Some(
                String::from_utf8_lossy(&bytes).to_string(),
            )))
        }
        b'*' => {
            let length: i64 = read_redis_line(reader).await?.parse().map_err(|error| {
                ConnectorError::Upstream(format!("invalid Redis array length: {error}"))
            })?;
            if length < 0 {
                return Ok(RedisReply::Array(Vec::new()));
            }
            if length > 10_000 {
                return Err(ConnectorError::Upstream(
                    "Redis array response is too large".to_string(),
                ));
            }
            let mut values = Vec::with_capacity(length as usize);
            for _ in 0..length {
                values.push(Box::pin(read_redis_reply(reader)).await?);
            }
            Ok(RedisReply::Array(values))
        }
        other => Err(ConnectorError::Upstream(format!(
            "unsupported Redis response prefix {:?}",
            other as char
        ))),
    }
}

async fn read_redis_byte(reader: &mut BufReader<TcpStream>) -> Result<u8, ConnectorError> {
    let mut byte = [0u8; 1];
    reader.read_exact(&mut byte).await?;
    Ok(byte[0])
}

async fn read_redis_line(reader: &mut BufReader<TcpStream>) -> Result<String, ConnectorError> {
    let mut bytes = Vec::new();
    reader.read_until(b'\n', &mut bytes).await?;
    if bytes.len() < 2 || !bytes.ends_with(b"\r\n") {
        return Err(ConnectorError::Upstream(
            "invalid Redis line terminator".to_string(),
        ));
    }
    bytes.truncate(bytes.len() - 2);
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

fn mq_channel_allowed(channel: &str, allowed: &[String]) -> bool {
    !channel.is_empty()
        && channel.len() <= 512
        && !channel.contains('\0')
        && allowed.iter().any(|rule| {
            rule.strip_suffix('*')
                .map_or(rule == channel, |prefix| channel.starts_with(prefix))
        })
}

fn valid_mq_channel_pattern(pattern: &str) -> bool {
    !pattern.is_empty()
        && pattern.len() <= 512
        && !pattern.contains('\0')
        && pattern
            .strip_suffix('*')
            .is_none_or(|prefix| !prefix.contains('*'))
}

async fn validate_s3_endpoint(config: &S3ConnectionConfig) -> Result<(), ConnectorError> {
    let url = reqwest::Url::parse(&config.endpoint)
        .map_err(|error| ConnectorError::Invalid(format!("invalid S3 endpoint: {error}")))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(ConnectorError::Forbidden(
            "S3 endpoint must be HTTP(S) without URL credentials".to_string(),
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| ConnectorError::Invalid("S3 endpoint has no host".to_string()))?;
    let port = url.port_or_known_default().unwrap_or(80);
    if host != "localhost"
        && host
            .parse::<std::net::IpAddr>()
            .ok()
            .is_none_or(|ip| !ip.is_loopback())
        && !config
            .allowed_hosts
            .iter()
            .any(|rule| host_rule_matches(rule, host, port))
    {
        return Err(ConnectorError::Forbidden(format!(
            "S3 host `{host}:{port}` is not in allowedHosts"
        )));
    }
    let addresses = tokio::net::lookup_host((host, port))
        .await
        .map_err(|error| ConnectorError::Upstream(format!("cannot resolve S3 host: {error}")))?;
    for address in addresses {
        let blocked = match address.ip() {
            std::net::IpAddr::V4(ip) => {
                ip.is_link_local() || ip.is_broadcast() || ip.is_unspecified()
            }
            std::net::IpAddr::V6(ip) => {
                ip.is_unicast_link_local() || ip.is_unspecified() || ip.is_multicast()
            }
        };
        if blocked {
            return Err(ConnectorError::Forbidden(
                "S3 endpoint resolves to a blocked address".to_string(),
            ));
        }
    }
    Ok(())
}

fn s3_url(config: &S3ConnectionConfig, key: &str) -> Result<reqwest::Url, ConnectorError> {
    let mut url = reqwest::Url::parse(&config.endpoint)
        .map_err(|error| ConnectorError::Invalid(format!("invalid S3 endpoint: {error}")))?;
    if url.path() != "/" {
        return Err(ConnectorError::Invalid(
            "S3 endpoint must not contain a base path".to_string(),
        ));
    }
    {
        let mut segments = url.path_segments_mut().map_err(|_| {
            ConnectorError::Invalid("S3 endpoint cannot be used as a base URL".to_string())
        })?;
        segments.clear().push(&config.bucket);
        if !key.is_empty() {
            for segment in key.split('/') {
                segments.push(segment);
            }
        }
    }
    Ok(url)
}

fn s3_key(key: &str) -> Result<String, ConnectorError> {
    validate_s3_key(key, false)?;
    Ok(key.to_string())
}

fn s3_prefix(prefix: &str) -> Result<(), ConnectorError> {
    validate_s3_key(prefix, true)
}

fn validate_s3_key(key: &str, allow_empty: bool) -> Result<(), ConnectorError> {
    if (!allow_empty && key.is_empty())
        || key.len() > 1024
        || key.starts_with('/')
        || key.contains('\0')
        || key
            .split('/')
            .any(|segment| segment == ".." || segment == ".")
    {
        return Err(ConnectorError::Invalid("invalid S3 object key".to_string()));
    }
    Ok(())
}

fn sign_s3_request(
    connection: &ConnectionRow,
    config: &S3ConnectionConfig,
    method: &reqwest::Method,
    url: &reqwest::Url,
    body: &[u8],
    content_type: Option<&str>,
) -> Result<Vec<(String, String)>, ConnectorError> {
    let credential_env = connection.credential_env.as_deref().ok_or_else(|| {
        ConnectorError::Invalid("S3 connector requires credentialEnv".to_string())
    })?;
    let credentials_json = std::env::var(credential_env).map_err(|_| {
        ConnectorError::Invalid(format!(
            "credential environment variable `{credential_env}` is not set"
        ))
    })?;
    let credentials: S3Credentials = serde_json::from_str(&credentials_json).map_err(|error| {
        ConnectorError::Invalid(format!("invalid S3 credentials JSON: {error}"))
    })?;
    if credentials.access_key.is_empty() || credentials.secret_key.is_empty() {
        return Err(ConnectorError::Invalid(
            "S3 accessKey and secretKey must not be empty".to_string(),
        ));
    }
    let now = OffsetDateTime::now_utc();
    let date = now
        .format(format_description!("[year][month][day]"))
        .map_err(|error| ConnectorError::Invalid(format!("cannot format S3 date: {error}")))?;
    let timestamp = now
        .format(format_description!(
            "[year][month][day]T[hour][minute][second]Z"
        ))
        .map_err(|error| ConnectorError::Invalid(format!("cannot format S3 time: {error}")))?;
    let payload_hash = format!("{:x}", Sha256::digest(body));
    let host = match url.port() {
        Some(port) => format!("{}:{port}", url.host_str().unwrap_or_default()),
        None => url.host_str().unwrap_or_default().to_string(),
    };
    let mut headers = vec![
        ("host".to_string(), host),
        ("x-amz-content-sha256".to_string(), payload_hash.clone()),
        ("x-amz-date".to_string(), timestamp.clone()),
    ];
    if let Some(content_type) = content_type {
        headers.push(("content-type".to_string(), content_type.to_string()));
    }
    if let Some(token) = credentials.session_token.as_deref() {
        headers.push(("x-amz-security-token".to_string(), token.to_string()));
    }
    headers.sort_by(|left, right| left.0.cmp(&right.0));
    let canonical_headers = headers
        .iter()
        .map(|(name, value)| format!("{name}:{}\n", value.trim()))
        .collect::<String>();
    let signed_headers = headers
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(";");
    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        method.as_str(),
        url.path(),
        url.query().unwrap_or_default(),
        canonical_headers,
        signed_headers,
        payload_hash
    );
    let scope = format!("{date}/{}/s3/aws4_request", config.region);
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{timestamp}\n{scope}\n{:x}",
        Sha256::digest(canonical_request.as_bytes())
    );
    let date_key = hmac_sha256(
        format!("AWS4{}", credentials.secret_key).as_bytes(),
        date.as_bytes(),
    );
    let region_key = hmac_sha256(&date_key, config.region.as_bytes());
    let service_key = hmac_sha256(&region_key, b"s3");
    let signing_key = hmac_sha256(&service_key, b"aws4_request");
    let signature = hmac_sha256(&signing_key, string_to_sign.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    headers.push((
        "authorization".to_string(),
        format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
            credentials.access_key
        ),
    ));
    Ok(headers)
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key);
    ring::hmac::sign(&key, data).as_ref().to_vec()
}

async fn s3_response_error(operation: &str, response: reqwest::Response) -> ConnectorError {
    let status = response.status();
    let body = response
        .text()
        .await
        .unwrap_or_default()
        .chars()
        .take(4096)
        .collect::<String>();
    ConnectorError::Upstream(format!(
        "S3 {operation} returned HTTP {}: {body}",
        status.as_u16()
    ))
}

fn xml_values(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut values = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let value_start = start + open.len();
        let Some(end) = rest[value_start..].find(&close) else {
            break;
        };
        let value = rest[value_start..value_start + end]
            .replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'");
        values.push(value);
        rest = &rest[value_start + end + close.len()..];
    }
    values
}

fn canonical_git_root(config: &GitConnectionConfig) -> Result<PathBuf, ConnectorError> {
    let root = std::fs::canonicalize(&config.repository_root).map_err(|error| {
        ConnectorError::Invalid(format!(
            "cannot open Git repository `{}`: {error}",
            config.repository_root
        ))
    })?;
    if !root.is_dir() || (!root.join(".git").exists() && !root.join("HEAD").is_file()) {
        return Err(ConnectorError::Invalid(
            "repositoryRoot is not a Git worktree or bare repository".to_string(),
        ));
    }
    Ok(root)
}

fn validate_git_revision(revision: &str) -> Result<(), ConnectorError> {
    if revision.is_empty()
        || revision.len() > 256
        || revision.starts_with('-')
        || revision.contains('\0')
        || revision.chars().any(|character| {
            !(character.is_ascii_alphanumeric()
                || matches!(
                    character,
                    '_' | '.' | '/' | '^' | '~' | '@' | '{' | '}' | '-'
                ))
        })
    {
        return Err(ConnectorError::Invalid(
            "Git revision contains unsupported characters".to_string(),
        ));
    }
    Ok(())
}

fn git_relative_path(path: &str) -> Result<String, ConnectorError> {
    let relative = resolve_under_root(Path::new("/"), path)?;
    Ok(relative
        .strip_prefix("/")
        .unwrap_or(&relative)
        .to_string_lossy()
        .replace('\\', "/"))
}

async fn git_command(
    args: &[String],
    max_output_bytes: usize,
    timeout: Duration,
) -> Result<Value, ConnectorError> {
    let mut command = tokio::process::Command::new("git");
    command
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| ConnectorError::Upstream("git command timed out".to_string()))??;
    let (stdout, stdout_truncated) = truncate_output(&output.stdout, max_output_bytes);
    let (stderr, stderr_truncated) = truncate_output(&output.stderr, max_output_bytes);
    if !output.status.success() {
        return Err(ConnectorError::Upstream(format!(
            "git exited with {}: {}",
            output.status.code().unwrap_or(-1),
            stderr
        )));
    }
    Ok(json!({
        "ok": true,
        "kind": "git",
        "exitCode": output.status.code().unwrap_or(0),
        "stdout": stdout,
        "stderr": stderr,
        "truncated": stdout_truncated || stderr_truncated
    }))
}

fn truncate_output(bytes: &[u8], limit: usize) -> (String, bool) {
    let truncated = bytes.len() > limit;
    let bytes = &bytes[..bytes.len().min(limit)];
    let mut boundary = bytes.len();
    while boundary > 0 && std::str::from_utf8(&bytes[..boundary]).is_err() {
        boundary -= 1;
    }
    (
        String::from_utf8_lossy(&bytes[..boundary]).to_string(),
        truncated,
    )
}

fn audit_summary(value: &Value) -> Value {
    let mut summary = serde_json::Map::new();
    for key in [
        "ok",
        "kind",
        "status",
        "contentType",
        "attempts",
        "idempotencyKey",
        "bytes",
        "encoding",
        "writable",
        "path",
        "rowCount",
        "rowsAffected",
        "truncated",
        "exitCode",
        "channel",
        "published",
        "duplicate",
        "sha256",
        "etag",
        "count",
        "deleted",
    ] {
        if let Some(value) = value.get(key) {
            summary.insert(key.to_string(), value.clone());
        }
    }
    if let Some(entries) = value.get("entries").and_then(Value::as_array) {
        summary.insert("entryCount".to_string(), json!(entries.len()));
    }
    Value::Object(summary)
}

fn resolve_under_root(root: &Path, relative: &str) -> Result<PathBuf, ConnectorError> {
    let relative = relative.trim();
    let path = Path::new(relative);
    if path.is_absolute() {
        return Err(ConnectorError::Forbidden(
            "absolute paths are not allowed".to_string(),
        ));
    }
    for component in path.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            _ => {
                return Err(ConnectorError::Forbidden(
                    "path traversal is not allowed".to_string(),
                ));
            }
        }
    }
    Ok(root.join(path))
}

fn valid_host_rule(rule: &str) -> bool {
    if rule == "*" {
        return true;
    }
    let host = rule.strip_prefix("*.").unwrap_or(rule);
    let (host, port) = match host.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (host, None),
    };
    !host.is_empty()
        && host
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '-'))
        && port.is_none_or(|port| port.parse::<u16>().is_ok())
}

fn valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn connector_kind(kind: ConnectorKind) -> &'static str {
    match kind {
        ConnectorKind::Http => "http",
        ConnectorKind::File => "file",
        ConnectorKind::Database => "database",
        ConnectorKind::Git => "git",
        ConnectorKind::Mq => "mq",
        ConnectorKind::S3 => "s3",
    }
}

fn classify_reqwest(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "upstream timeout".to_string()
    } else if error.is_connect() {
        "upstream connection failed".to_string()
    } else if error.is_redirect() {
        "redirects are disabled".to_string()
    } else {
        format!("upstream request failed: {error}")
    }
}

fn retry_delay(response: &reqwest::Response) -> Duration {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(|seconds| Duration::from_secs(seconds.min(2)))
        .unwrap_or(Duration::from_millis(200))
}

fn redact_detail(detail: Value) -> Value {
    match detail {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| {
                    let lower = key.to_ascii_lowercase();
                    if matches!(
                        lower.as_str(),
                        "authorization" | "token" | "secret" | "password" | "credential"
                    ) {
                        (key, Value::String("[redacted]".to_string()))
                    } else {
                        (key, redact_detail(value))
                    }
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.into_iter().map(redact_detail).collect()),
        other => other,
    }
}

fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default()
}

pub async fn seed_connection_workspace(data_dir: &Path) -> Result<PathBuf, ConnectorError> {
    let root = data_dir.join("connector-workspace");
    tokio::fs::create_dir_all(&root).await?;
    let note = root.join("welcome.txt");
    if !note.exists() {
        tokio::fs::write(
            &note,
            "hpa connector workspace\nThis file is readable through the File connector.\n",
        )
        .await?;
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_and_config_validation() {
        let manifest = builtin_http_manifest().expect("manifest");
        validate_manifest(&manifest).expect("valid");
        let file = builtin_file_manifest().expect("file manifest");
        validate_manifest(&file).expect("file valid");
        let database = builtin_database_manifest().expect("database manifest");
        validate_manifest(&database).expect("database valid");
        let git = builtin_git_manifest().expect("git manifest");
        validate_manifest(&git).expect("git valid");
        let mq = builtin_mq_manifest().expect("mq manifest");
        validate_manifest(&mq).expect("mq valid");
        let s3 = builtin_s3_manifest().expect("s3 manifest");
        validate_manifest(&s3).expect("s3 valid");
        assert!(valid_env_name("HPA_CONNECTOR_TOKEN"));
        assert!(!valid_env_name("1TOKEN"));
        assert!(!valid_env_name("TOKEN-NAME"));
    }

    #[test]
    fn path_resolution_blocks_escape() {
        let root = Path::new("/tmp/hpa-root");
        assert!(resolve_under_root(root, "../secret").is_err());
        assert!(resolve_under_root(root, "/etc/passwd").is_err());
        assert!(resolve_under_root(root, "nested/file.txt").is_ok());
    }

    #[test]
    fn host_rules_are_exact_or_wildcard() {
        assert!(host_rule_matches("api.example.com", "api.example.com", 443));
        assert!(host_rule_matches("*.example.com", "api.example.com", 443));
        assert!(!host_rule_matches("*.example.com", "example.com", 443));
        assert!(!host_rule_matches(
            "api.example.com:8080",
            "api.example.com",
            443
        ));
    }

    #[test]
    fn database_statement_and_git_revision_guards_are_strict() {
        assert!(database_read_statement("SELECT 1"));
        assert!(database_read_statement(
            "/* audit */ WITH value AS (SELECT 1) SELECT * FROM value"
        ));
        assert!(!database_read_statement("INSERT INTO t VALUES (1)"));
        assert!(validate_git_revision("HEAD~1").is_ok());
        assert!(validate_git_revision("origin/main").is_ok());
        assert!(validate_git_revision("--upload-pack=evil").is_err());
        assert!(validate_git_revision("HEAD;rm").is_err());
    }
}
