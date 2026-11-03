use serde_json::Value;
use sqlx::Row;
use sqlx::postgres::{PgPool, PgRow};
use sqlx::types::Json;
use thiserror::Error;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

#[derive(Debug, Error)]
pub enum PostgresConnectorError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Frozen(String),
    #[error("{0}")]
    Owned(String),
    #[error("invalid connector metadata: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone)]
pub struct ConnectorDefinitionRecord {
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

#[derive(Debug, Clone)]
pub struct ConnectionRecord {
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

#[derive(Debug, Clone)]
pub struct ConnectorAuditRecord {
    pub id: String,
    pub at: String,
    pub connection_id: String,
    pub operation: String,
    pub status: String,
    pub latency_ms: i64,
    pub detail_json: String,
}

#[derive(Clone)]
pub struct PostgresConnectorStore {
    pool: PgPool,
    tenant_id: String,
}

impl PostgresConnectorStore {
    pub fn new(pool: PgPool, tenant_id: impl Into<String>) -> Self {
        Self {
            pool,
            tenant_id: tenant_id.into(),
        }
    }

    async fn begin(&self) -> Result<sqlx::Transaction<'_, sqlx::Postgres>, PostgresConnectorError> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query("SET LOCAL ROLE hpa_tenant_runtime")
            .execute(&mut *transaction)
            .await?;
        sqlx::query("SELECT set_config('hpa.tenant_id', $1, true)")
            .bind(&self.tenant_id)
            .execute(&mut *transaction)
            .await?;
        Ok(transaction)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_definition(
        &self,
        manifest_json: &str,
        connector_id: &str,
        version: &str,
        name: &str,
        kind: &str,
        plugin_id: Option<&str>,
        plugin_version: Option<&str>,
        status: &str,
        created_at: &str,
    ) -> Result<bool, PostgresConnectorError> {
        let current: Value = serde_json::from_str(manifest_json)?;
        let mut transaction = self.begin().await?;
        let existing = sqlx::query(
            "SELECT manifest_json, plugin_id
             FROM hpa_tenant.connector_definitions
             WHERE tenant_id = $1 AND connector_id = $2 AND version = $3",
        )
        .bind(&self.tenant_id)
        .bind(connector_id)
        .bind(version)
        .fetch_optional(&mut *transaction)
        .await?;
        if let Some(row) = existing {
            let previous: Value = row.try_get("manifest_json")?;
            let existing_plugin: Option<String> = row.try_get("plugin_id")?;
            if existing_plugin.as_deref() != plugin_id {
                transaction.commit().await?;
                return Err(PostgresConnectorError::Owned(format!(
                    "connector `{connector_id}@{version}` is already registered by {}",
                    existing_plugin.as_deref().unwrap_or("a built-in connector")
                )));
            }
            if previous != current {
                transaction.commit().await?;
                return Err(PostgresConnectorError::Frozen(format!(
                    "connector `{connector_id}@{version}` is frozen with different content; bump the connector version"
                )));
            }
            if plugin_id.is_some() {
                sqlx::query(
                    "UPDATE hpa_tenant.connector_definitions
                     SET plugin_version = $1, status = $2
                     WHERE tenant_id = $3 AND connector_id = $4 AND version = $5",
                )
                .bind(plugin_version)
                .bind(status)
                .bind(&self.tenant_id)
                .bind(connector_id)
                .bind(version)
                .execute(&mut *transaction)
                .await?;
            }
            transaction.commit().await?;
            return Ok(false);
        }
        sqlx::query(
            "INSERT INTO hpa_tenant.connector_definitions
             (tenant_id, connector_id, version, name, kind, manifest_json, plugin_id,
              plugin_version, status, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             ON CONFLICT (tenant_id, connector_id, version) DO UPDATE SET
                name = EXCLUDED.name,
                kind = EXCLUDED.kind,
                manifest_json = EXCLUDED.manifest_json,
                plugin_id = EXCLUDED.plugin_id,
                plugin_version = EXCLUDED.plugin_version,
                status = EXCLUDED.status",
        )
        .bind(&self.tenant_id)
        .bind(connector_id)
        .bind(version)
        .bind(name)
        .bind(kind)
        .bind(Json(&current))
        .bind(plugin_id)
        .bind(plugin_version)
        .bind(status)
        .bind(parse_time(created_at)?)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(true)
    }

    pub async fn list_definitions(
        &self,
    ) -> Result<Vec<ConnectorDefinitionRecord>, PostgresConnectorError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT connector_id, version, name, kind, manifest_json, plugin_id,
                    plugin_version, status, created_at
             FROM hpa_tenant.connector_definitions
             WHERE tenant_id = $1 ORDER BY name ASC, version DESC",
        )
        .bind(&self.tenant_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.iter().map(definition_from_row).collect()
    }

    pub async fn definition(
        &self,
        connector_id: &str,
        version: Option<&str>,
    ) -> Result<Option<ConnectorDefinitionRecord>, PostgresConnectorError> {
        let mut transaction = self.begin().await?;
        let row = match version {
            Some(version) => {
                sqlx::query(
                    "SELECT connector_id, version, name, kind, manifest_json, plugin_id,
                        plugin_version, status, created_at
                 FROM hpa_tenant.connector_definitions
                 WHERE tenant_id = $1 AND connector_id = $2 AND version = $3",
                )
                .bind(&self.tenant_id)
                .bind(connector_id)
                .bind(version)
                .fetch_optional(&mut *transaction)
                .await?
            }
            None => {
                sqlx::query(
                    "SELECT connector_id, version, name, kind, manifest_json, plugin_id,
                        plugin_version, status, created_at
                 FROM hpa_tenant.connector_definitions
                 WHERE tenant_id = $1 AND connector_id = $2 AND status = 'enabled'
                 ORDER BY version DESC LIMIT 1",
                )
                .bind(&self.tenant_id)
                .bind(connector_id)
                .fetch_optional(&mut *transaction)
                .await?
            }
        };
        transaction.commit().await?;
        row.as_ref().map(definition_from_row).transpose()
    }

    pub async fn list_connections(&self) -> Result<Vec<ConnectionRecord>, PostgresConnectorError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT id, name, connector_id, connector_version, config_json, credential_env,
                    status, created_at, updated_at
             FROM hpa_tenant.connections
             WHERE tenant_id = $1 ORDER BY name ASC",
        )
        .bind(&self.tenant_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.iter().map(connection_from_row).collect()
    }

    pub async fn connection(
        &self,
        id: &str,
    ) -> Result<Option<ConnectionRecord>, PostgresConnectorError> {
        let mut transaction = self.begin().await?;
        let row = sqlx::query(
            "SELECT id, name, connector_id, connector_version, config_json, credential_env,
                    status, created_at, updated_at
             FROM hpa_tenant.connections WHERE tenant_id = $1 AND id = $2",
        )
        .bind(&self.tenant_id)
        .bind(id)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.as_ref().map(connection_from_row).transpose()
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_connection(
        &self,
        id: &str,
        name: &str,
        connector_id: &str,
        connector_version: &str,
        config_json: &str,
        credential_env: Option<&str>,
        created_at: &str,
        updated_at: &str,
    ) -> Result<ConnectionRecord, PostgresConnectorError> {
        let config: Value = serde_json::from_str(config_json)?;
        let mut transaction = self.begin().await?;
        let existing: Option<i32> = sqlx::query_scalar(
            "SELECT 1 FROM hpa_tenant.connections WHERE tenant_id = $1 AND id = $2",
        )
        .bind(&self.tenant_id)
        .bind(id)
        .fetch_optional(&mut *transaction)
        .await?;
        if existing.is_some() {
            sqlx::query(
                "UPDATE hpa_tenant.connections
                 SET name = $1, connector_id = $2, connector_version = $3, config_json = $4,
                     credential_env = $5, status = 'active', updated_at = $6
                 WHERE tenant_id = $7 AND id = $8",
            )
            .bind(name)
            .bind(connector_id)
            .bind(connector_version)
            .bind(Json(&config))
            .bind(credential_env)
            .bind(parse_time(updated_at)?)
            .bind(&self.tenant_id)
            .bind(id)
            .execute(&mut *transaction)
            .await?;
        } else {
            sqlx::query(
                "INSERT INTO hpa_tenant.connections
                 (tenant_id, id, name, connector_id, connector_version, config_json,
                  credential_env, status, created_at, updated_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, 'active', $8, $9)",
            )
            .bind(&self.tenant_id)
            .bind(id)
            .bind(name)
            .bind(connector_id)
            .bind(connector_version)
            .bind(Json(&config))
            .bind(credential_env)
            .bind(parse_time(created_at)?)
            .bind(parse_time(updated_at)?)
            .execute(&mut *transaction)
            .await?;
        }
        let row = sqlx::query(
            "SELECT id, name, connector_id, connector_version, config_json, credential_env,
                    status, created_at, updated_at
             FROM hpa_tenant.connections WHERE tenant_id = $1 AND id = $2",
        )
        .bind(&self.tenant_id)
        .bind(id)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        connection_from_row(&row)
    }

    pub async fn delete_connection(&self, id: &str) -> Result<bool, PostgresConnectorError> {
        let mut transaction = self.begin().await?;
        let result =
            sqlx::query("DELETE FROM hpa_tenant.connections WHERE tenant_id = $1 AND id = $2")
                .bind(&self.tenant_id)
                .bind(id)
                .execute(&mut *transaction)
                .await?;
        transaction.commit().await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn insert_audit(
        &self,
        record: &ConnectorAuditRecord,
    ) -> Result<(), PostgresConnectorError> {
        let detail: Value = serde_json::from_str(&record.detail_json)?;
        let mut transaction = self.begin().await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.connector_audit
             (tenant_id, id, at, connection_id, operation, status, latency_ms, detail_json)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(&self.tenant_id)
        .bind(&record.id)
        .bind(parse_time(&record.at)?)
        .bind(&record.connection_id)
        .bind(&record.operation)
        .bind(&record.status)
        .bind(record.latency_ms)
        .bind(Json(&detail))
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn list_audit(
        &self,
        limit: i64,
    ) -> Result<Vec<ConnectorAuditRecord>, PostgresConnectorError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT id, at, connection_id, operation, status, latency_ms, detail_json
             FROM hpa_tenant.connector_audit
             WHERE tenant_id = $1 ORDER BY at DESC LIMIT $2",
        )
        .bind(&self.tenant_id)
        .bind(limit.clamp(1, 200))
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.iter().map(audit_from_row).collect()
    }

    pub async fn set_plugin_definitions_status(
        &self,
        plugin_id: &str,
        plugin_version: &str,
        status: &str,
    ) -> Result<u64, PostgresConnectorError> {
        let mut transaction = self.begin().await?;
        let result = sqlx::query(
            "UPDATE hpa_tenant.connector_definitions SET status = $1
             WHERE tenant_id = $2 AND plugin_id = $3 AND plugin_version = $4",
        )
        .bind(status)
        .bind(&self.tenant_id)
        .bind(plugin_id)
        .bind(plugin_version)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(result.rows_affected())
    }

    pub async fn delete_plugin_definitions(
        &self,
        plugin_id: &str,
    ) -> Result<u64, PostgresConnectorError> {
        let mut transaction = self.begin().await?;
        let result = sqlx::query(
            "DELETE FROM hpa_tenant.connector_definitions
             WHERE tenant_id = $1 AND plugin_id = $2",
        )
        .bind(&self.tenant_id)
        .bind(plugin_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(result.rows_affected())
    }
}

fn definition_from_row(row: &PgRow) -> Result<ConnectorDefinitionRecord, PostgresConnectorError> {
    let manifest: Value = row.try_get("manifest_json")?;
    Ok(ConnectorDefinitionRecord {
        connector_id: row.try_get("connector_id")?,
        version: row.try_get("version")?,
        name: row.try_get("name")?,
        kind: row.try_get("kind")?,
        manifest_json: serde_json::to_string(&manifest)?,
        plugin_id: row.try_get("plugin_id")?,
        plugin_version: row.try_get("plugin_version")?,
        status: row.try_get("status")?,
        created_at: format_time(row.try_get("created_at")?),
    })
}

fn connection_from_row(row: &PgRow) -> Result<ConnectionRecord, PostgresConnectorError> {
    let config: Value = row.try_get("config_json")?;
    Ok(ConnectionRecord {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        connector_id: row.try_get("connector_id")?,
        connector_version: row.try_get("connector_version")?,
        config_json: serde_json::to_string(&config)?,
        credential_env: row.try_get("credential_env")?,
        status: row.try_get("status")?,
        created_at: format_time(row.try_get("created_at")?),
        updated_at: format_time(row.try_get("updated_at")?),
    })
}

fn audit_from_row(row: &PgRow) -> Result<ConnectorAuditRecord, PostgresConnectorError> {
    let detail: Value = row.try_get("detail_json")?;
    Ok(ConnectorAuditRecord {
        id: row.try_get("id")?,
        at: format_time(row.try_get("at")?),
        connection_id: row.try_get("connection_id")?,
        operation: row.try_get("operation")?,
        status: row.try_get("status")?,
        latency_ms: row.try_get("latency_ms")?,
        detail_json: serde_json::to_string(&detail)?,
    })
}

fn parse_time(raw: &str) -> Result<OffsetDateTime, PostgresConnectorError> {
    OffsetDateTime::parse(raw, &Rfc3339).map_err(|error| {
        PostgresConnectorError::Invalid(format!("invalid timestamp `{raw}`: {error}"))
    })
}

fn format_time(value: OffsetDateTime) -> String {
    value.format(&Rfc3339).unwrap_or_default()
}
