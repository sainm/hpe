use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use uuid::Uuid;

#[path = "browser_auth.rs"]
pub mod browser_auth;
#[path = "oidc_verify.rs"]
pub mod oidc_verify;

use oidc_verify::Jwk;

pub const WORKER_OFFLINE_MS: i64 = 30_000;

const OIDC_CACHE_TTL: Duration = Duration::from_secs(300);
const JWKS_MAX_BYTES: usize = 1_048_576;
const SESSION_TTL_MIN: i64 = 300;
const SESSION_TTL_MAX: i64 = 604_800;
const DEFAULT_SCOPES: &str = "openid email profile";
const DEFAULT_SESSION_TTL: i64 = 28_800;

#[derive(Debug, thiserror::Error)]
pub enum OpsError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid request: {0}")]
    Invalid(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("upstream error: {0}")]
    Upstream(String),
    #[error("forbidden: {0}")]
    Forbidden(String),
}

#[derive(Clone)]
pub struct OpsService {
    pub control_pool: SqlitePool,
    pub data_dir: PathBuf,
}

pub struct OidcPrincipal {
    pub subject: String,
    pub tenant_id: String,
    pub role: String,
    pub email: Option<String>,
    pub name: Option<String>,
}

struct CachedJwks {
    fetched_at: Instant,
    keys: Vec<Jwk>,
}

fn jwks_cache() -> &'static Mutex<HashMap<String, CachedJwks>> {
    static CACHE: OnceLock<Mutex<HashMap<String, CachedJwks>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

impl OpsService {
    pub fn new(control_pool: SqlitePool, data_dir: PathBuf) -> Self {
        Self {
            control_pool,
            data_dir,
        }
    }

    pub async fn ensure_schema(&self) -> Result<(), OpsError> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS enterprise_backups (
                id TEXT PRIMARY KEY,
                tenant_id TEXT NOT NULL,
                path TEXT NOT NULL,
                size INTEGER NOT NULL,
                sha256 TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at INTEGER NOT NULL
            )",
        )
        .execute(&self.control_pool)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS enterprise_workers (
                id TEXT PRIMARY KEY,
                tenant_id TEXT NOT NULL,
                name TEXT NOT NULL,
                kind TEXT NOT NULL,
                status TEXT NOT NULL,
                capabilities_json TEXT NOT NULL DEFAULT '[\"workflow\"]',
                max_concurrency INTEGER NOT NULL DEFAULT 1,
                runtime_version TEXT,
                last_heartbeat_at INTEGER NOT NULL,
                registered_at INTEGER NOT NULL,
                metadata_json TEXT NOT NULL
            )",
        )
        .execute(&self.control_pool)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS enterprise_oidc (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                issuer TEXT NOT NULL,
                audience TEXT NOT NULL,
                jwks_url TEXT NOT NULL,
                network_json TEXT NOT NULL,
                updated_at INTEGER NOT NULL,
                client_id TEXT NOT NULL DEFAULT '',
                authorization_endpoint TEXT NOT NULL DEFAULT '',
                token_endpoint TEXT NOT NULL DEFAULT '',
                redirect_uri TEXT NOT NULL DEFAULT '',
                scopes TEXT NOT NULL DEFAULT 'openid email profile',
                admin_subjects_json TEXT NOT NULL DEFAULT '[]',
                cookie_secure INTEGER NOT NULL DEFAULT 0,
                session_ttl_seconds INTEGER NOT NULL DEFAULT 28800
            )",
        )
        .execute(&self.control_pool)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS enterprise_backups_tenant
             ON enterprise_backups (tenant_id, created_at DESC)",
        )
        .execute(&self.control_pool)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS enterprise_workers_tenant
             ON enterprise_workers (tenant_id, registered_at DESC)",
        )
        .execute(&self.control_pool)
        .await?;
        ensure_worker_columns(&self.control_pool).await?;
        ensure_oidc_columns(&self.control_pool).await?;
        Ok(())
    }

    pub async fn create_backup(&self, tenant_id: &str) -> Result<Value, OpsError> {
        validate_tenant(tenant_id)?;
        let source = self.tenant_db_path(tenant_id)?;
        if !source.is_file() {
            return Err(OpsError::NotFound(format!(
                "tenant database for `{tenant_id}` does not exist"
            )));
        }
        let backup_root = self.data_dir.join("backups");
        let backup_dir = backup_root.join(tenant_id);
        tokio::fs::create_dir_all(&backup_dir).await?;
        let mut chosen = None;
        for _ in 0..100 {
            let millis = now_ms();
            let candidate = backup_dir.join(format!("backup-{millis}.db"));
            if !candidate.exists() {
                chosen = Some((millis, candidate));
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        let (created_at, target) = chosen
            .ok_or_else(|| OpsError::Invalid("could not allocate a backup path".to_string()))?;
        let options = SqliteConnectOptions::new()
            .filename(&source)
            .create_if_missing(false);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        let target_text = target.to_string_lossy().to_string();
        sqlx::query("VACUUM INTO ?")
            .bind(&target_text)
            .execute(&pool)
            .await?;
        pool.close().await;
        let canonical_root = tokio::fs::canonicalize(&backup_root).await?;
        let canonical_target = tokio::fs::canonicalize(&target).await?;
        if !canonical_target.starts_with(&canonical_root) {
            let _ = tokio::fs::remove_file(&target).await;
            return Err(OpsError::Forbidden(
                "backup path escapes the backups directory".to_string(),
            ));
        }
        let bytes = tokio::fs::read(&target).await?;
        let sha256 = sha256_hex(&bytes);
        let size = bytes.len() as i64;
        let id = format!("backup_{}", Uuid::now_v7());
        sqlx::query(
            "INSERT INTO enterprise_backups
             (id, tenant_id, path, size, sha256, status, created_at)
             VALUES (?, ?, ?, ?, ?, 'ready', ?)",
        )
        .bind(&id)
        .bind(tenant_id)
        .bind(&target_text)
        .bind(size)
        .bind(&sha256)
        .bind(created_at)
        .execute(&self.control_pool)
        .await?;
        Ok(backup_json(
            &id,
            tenant_id,
            &target_text,
            size,
            &sha256,
            created_at,
        ))
    }

    pub async fn list_backups(&self, tenant_id: Option<&str>) -> Result<Vec<Value>, OpsError> {
        let rows: Vec<(String, String, String, i64, String, i64)> = match tenant_id {
            Some(tenant) => {
                validate_tenant(tenant)?;
                sqlx::query_as(
                    "SELECT id, tenant_id, path, size, sha256, created_at
                     FROM enterprise_backups WHERE tenant_id = ?
                     ORDER BY created_at DESC, id DESC",
                )
                .bind(tenant)
                .fetch_all(&self.control_pool)
                .await?
            }
            None => {
                sqlx::query_as(
                    "SELECT id, tenant_id, path, size, sha256, created_at
                     FROM enterprise_backups
                     ORDER BY created_at DESC, id DESC",
                )
                .fetch_all(&self.control_pool)
                .await?
            }
        };
        Ok(rows
            .into_iter()
            .map(|(id, tenant, path, size, sha256, created_at)| {
                backup_json(&id, &tenant, &path, size, &sha256, created_at)
            })
            .collect())
    }

    pub async fn delete_backup(&self, id: &str) -> Result<bool, OpsError> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT path FROM enterprise_backups WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.control_pool)
                .await?;
        let Some((path,)) = row else {
            return Ok(false);
        };
        if !self.inside_backups(&path) {
            return Err(OpsError::Forbidden(
                "backup path is outside the backups directory".to_string(),
            ));
        }
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(OpsError::Io(error)),
        }
        sqlx::query("DELETE FROM enterprise_backups WHERE id = ?")
            .bind(id)
            .execute(&self.control_pool)
            .await?;
        Ok(true)
    }

    pub async fn restore_backup(&self, id: &str) -> Result<Value, OpsError> {
        let row: Option<(String, String, String, String)> = sqlx::query_as(
            "SELECT tenant_id, path, sha256, status FROM enterprise_backups WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.control_pool)
        .await?;
        let Some((tenant_id, path, expected_sha, status)) = row else {
            return Err(OpsError::NotFound(format!("backup `{id}` does not exist")));
        };
        if status != "ready" {
            return Err(OpsError::Invalid(format!(
                "backup `{id}` is not ready to restore"
            )));
        }
        validate_tenant(&tenant_id)?;
        if !self.inside_backups(&path) {
            return Err(OpsError::Forbidden(
                "backup path is outside the backups directory".to_string(),
            ));
        }
        let target = self.tenant_db_path(&tenant_id)?;
        let bytes = match tokio::fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(OpsError::NotFound(format!(
                    "backup file for `{id}` is missing"
                )));
            }
            Err(error) => return Err(OpsError::Io(error)),
        };
        if sha256_hex(&bytes) != expected_sha {
            return Err(OpsError::Invalid(format!(
                "backup `{id}` failed checksum verification"
            )));
        }
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let parent = target
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let temp = parent.join(format!(".hpa-restore-{}.tmp", Uuid::now_v7()));
        tokio::fs::write(&temp, &bytes).await?;
        if let Err(error) = tokio::fs::rename(&temp, &target).await {
            let _ = tokio::fs::remove_file(&temp).await;
            return Err(OpsError::Io(error));
        }
        for suffix in ["-wal", "-shm"] {
            let mut sibling = target.clone().into_os_string();
            sibling.push(suffix);
            match tokio::fs::remove_file(PathBuf::from(sibling)).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(OpsError::Io(error)),
            }
        }
        Ok(json!({
            "restored": true,
            "requiresRestart": true,
            "message": format!("restored tenant `{tenant_id}` from backup `{id}`")
        }))
    }

    pub async fn purge_backups(
        &self,
        retain_days: i64,
        retain_count: i64,
    ) -> Result<usize, OpsError> {
        let rows: Vec<(String, String, String, i64)> = sqlx::query_as(
            "SELECT id, tenant_id, path, created_at FROM enterprise_backups
             ORDER BY tenant_id ASC, created_at DESC, id DESC",
        )
        .fetch_all(&self.control_pool)
        .await?;
        let now = now_ms();
        let cutoff = now - retain_days.max(0).saturating_mul(86_400_000);
        let mut rank: HashMap<String, i64> = HashMap::new();
        let mut stale = Vec::new();
        for (id, tenant_id, path, created_at) in rows {
            let position = rank.entry(tenant_id).or_insert(0);
            *position += 1;
            let keep_by_age = retain_days > 0 && created_at >= cutoff;
            let keep_by_count = retain_count > 0 && *position <= retain_count;
            if !keep_by_age && !keep_by_count {
                stale.push((id, path));
            }
        }
        let mut removed = 0usize;
        for (id, path) in stale {
            if self.inside_backups(&path) {
                match tokio::fs::remove_file(&path).await {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(OpsError::Io(error)),
                }
            }
            removed += sqlx::query("DELETE FROM enterprise_backups WHERE id = ?")
                .bind(&id)
                .execute(&self.control_pool)
                .await?
                .rows_affected() as usize;
        }
        Ok(removed)
    }

    #[allow(dead_code)]
    pub async fn register_worker(
        &self,
        tenant_id: &str,
        name: &str,
        kind: &str,
        metadata: Value,
    ) -> Result<Value, OpsError> {
        self.register_worker_with_config(
            tenant_id,
            name,
            kind,
            metadata,
            vec!["workflow".to_string()],
            1,
        )
        .await
    }

    #[allow(dead_code)]
    pub async fn register_worker_config(
        &self,
        tenant_id: &str,
        name: &str,
        kind: &str,
        metadata: Value,
        capabilities: Vec<String>,
        max_concurrency: i64,
    ) -> Result<Value, OpsError> {
        if !(1..=64).contains(&max_concurrency) {
            return Err(OpsError::Invalid(
                "worker max concurrency must be between 1 and 64".to_string(),
            ));
        }
        let mut normalized = Vec::with_capacity(capabilities.len());
        for capability in capabilities {
            let capability = capability.trim();
            if capability.is_empty() {
                return Err(OpsError::Invalid(
                    "worker capabilities must be non-empty strings".to_string(),
                ));
            }
            normalized.push(capability.to_string());
        }
        if normalized.is_empty() {
            return Err(OpsError::Invalid(
                "worker capabilities are required".to_string(),
            ));
        }
        self.register_worker_with_config(
            tenant_id,
            name,
            kind,
            metadata,
            normalized,
            max_concurrency,
        )
        .await
    }

    async fn register_worker_with_config(
        &self,
        tenant_id: &str,
        name: &str,
        kind: &str,
        metadata: Value,
        capabilities: Vec<String>,
        max_concurrency: i64,
    ) -> Result<Value, OpsError> {
        validate_tenant(tenant_id)?;
        if name.trim().is_empty() {
            return Err(OpsError::Invalid("worker name is required".to_string()));
        }
        if kind.trim().is_empty() {
            return Err(OpsError::Invalid("worker kind is required".to_string()));
        }
        if !metadata.is_null() && !metadata.is_object() {
            return Err(OpsError::Invalid(
                "worker metadata must be an object or null".to_string(),
            ));
        }
        let now = now_ms();
        let id = format!("worker_{}", Uuid::now_v7());
        let metadata_json = metadata.to_string();
        let capabilities_json = serde_json::to_string(&capabilities)?;
        sqlx::query(
            "INSERT INTO enterprise_workers
             (id, tenant_id, name, kind, status, capabilities_json, max_concurrency,
              runtime_version, last_heartbeat_at, registered_at, metadata_json)
             VALUES (?, ?, ?, ?, 'active', ?, ?, NULL, ?, ?, ?)",
        )
        .bind(&id)
        .bind(tenant_id)
        .bind(name)
        .bind(kind)
        .bind(&capabilities_json)
        .bind(max_concurrency)
        .bind(now)
        .bind(now)
        .bind(&metadata_json)
        .execute(&self.control_pool)
        .await?;
        Ok(worker_json(
            &id,
            tenant_id,
            name,
            kind,
            "active",
            now,
            now,
            &json!(metadata),
            &capabilities,
            max_concurrency,
            None,
        ))
    }

    pub async fn heartbeat_worker(&self, id: &str) -> Result<Value, OpsError> {
        let row = self.fetch_worker(id).await?;
        let now = now_ms();
        sqlx::query(
            "UPDATE enterprise_workers SET last_heartbeat_at = ?, status = ?
             WHERE id = ?",
        )
        .bind(now)
        .bind(if row.4 == "disabled" {
            "disabled"
        } else {
            "active"
        })
        .bind(id)
        .execute(&self.control_pool)
        .await?;
        let status = if row.4 == "disabled" {
            "disabled"
        } else {
            "active"
        };
        let metadata = serde_json::from_str(&row.7).unwrap_or(Value::Null);
        let capabilities = worker_capabilities(&row.8);
        Ok(worker_json(
            &row.0,
            &row.1,
            &row.2,
            &row.3,
            status,
            now,
            row.6,
            &metadata,
            &capabilities,
            row.9,
            row.10.as_deref(),
        ))
    }

    #[allow(clippy::type_complexity)]
    pub async fn list_workers(&self, tenant_id: Option<&str>) -> Result<Vec<Value>, OpsError> {
        let rows: Vec<(
            String,
            String,
            String,
            String,
            String,
            i64,
            i64,
            String,
            String,
            i64,
            Option<String>,
        )> = match tenant_id {
            Some(tenant) => {
                validate_tenant(tenant)?;
                sqlx::query_as(
                    "SELECT id, tenant_id, name, kind, status, last_heartbeat_at,
                            registered_at, metadata_json, capabilities_json, max_concurrency,
                            runtime_version
                     FROM enterprise_workers WHERE tenant_id = ?
                     ORDER BY registered_at DESC, id DESC",
                )
                .bind(tenant)
                .fetch_all(&self.control_pool)
                .await?
            }
            None => {
                sqlx::query_as(
                    "SELECT id, tenant_id, name, kind, status, last_heartbeat_at,
                            registered_at, metadata_json, capabilities_json, max_concurrency,
                            runtime_version
                     FROM enterprise_workers
                     ORDER BY registered_at DESC, id DESC",
                )
                .fetch_all(&self.control_pool)
                .await?
            }
        };
        let now = now_ms();
        Ok(rows
            .into_iter()
            .map(|row| {
                let status = derived_status(&row.4, row.5, now);
                let metadata = serde_json::from_str(&row.7).unwrap_or(Value::Null);
                let capabilities = worker_capabilities(&row.8);
                worker_json(
                    &row.0,
                    &row.1,
                    &row.2,
                    &row.3,
                    status,
                    row.5,
                    row.6,
                    &metadata,
                    &capabilities,
                    row.9,
                    row.10.as_deref(),
                )
            })
            .collect())
    }

    pub async fn disable_worker(&self, id: &str) -> Result<Value, OpsError> {
        let row = self.fetch_worker(id).await?;
        sqlx::query("UPDATE enterprise_workers SET status = 'disabled' WHERE id = ?")
            .bind(id)
            .execute(&self.control_pool)
            .await?;
        let metadata = serde_json::from_str(&row.7).unwrap_or(Value::Null);
        let capabilities = worker_capabilities(&row.8);
        Ok(worker_json(
            &row.0,
            &row.1,
            &row.2,
            &row.3,
            "disabled",
            row.5,
            row.6,
            &metadata,
            &capabilities,
            row.9,
            row.10.as_deref(),
        ))
    }

    #[allow(dead_code)]
    #[allow(clippy::type_complexity)]
    pub async fn set_oidc_config(
        &self,
        issuer: &str,
        audience: &str,
        jwks_url: &str,
        network: Vec<String>,
    ) -> Result<Value, OpsError> {
        let row: Option<(String, String, String, String, String, String, i64, i64)> =
            sqlx::query_as(
                "SELECT client_id, authorization_endpoint, token_endpoint, redirect_uri,
                        scopes, admin_subjects_json, cookie_secure, session_ttl_seconds
                 FROM enterprise_oidc WHERE id = 1",
            )
            .fetch_optional(&self.control_pool)
            .await?;
        let (
            client_id,
            authorization_endpoint,
            token_endpoint,
            redirect_uri,
            scopes,
            admin_subjects_json,
            cookie_secure,
            session_ttl_seconds,
        ) = row.unwrap_or_else(|| {
            (
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                DEFAULT_SCOPES.to_string(),
                "[]".to_string(),
                0,
                DEFAULT_SESSION_TTL,
            )
        });
        let admin_subjects: Vec<String> = serde_json::from_str(&admin_subjects_json)
            .map_err(|_| OpsError::Invalid("stored OIDC admin subjects are invalid".to_string()))?;
        self.set_browser_oidc_config(
            issuer,
            audience,
            jwks_url,
            network,
            &client_id,
            &authorization_endpoint,
            &token_endpoint,
            &redirect_uri,
            &scopes,
            admin_subjects,
            cookie_secure != 0,
            session_ttl_seconds,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn set_browser_oidc_config(
        &self,
        issuer: &str,
        audience: &str,
        jwks_url: &str,
        network: Vec<String>,
        client_id: &str,
        authorization_endpoint: &str,
        token_endpoint: &str,
        redirect_uri: &str,
        scopes: &str,
        admin_subjects: Vec<String>,
        cookie_secure: bool,
        session_ttl_seconds: i64,
    ) -> Result<Value, OpsError> {
        if issuer.trim().is_empty() {
            return Err(OpsError::Invalid("OIDC issuer is required".to_string()));
        }
        if audience.trim().is_empty() {
            return Err(OpsError::Invalid("OIDC audience is required".to_string()));
        }
        if network.iter().any(|rule| rule.trim().is_empty()) {
            return Err(OpsError::Invalid(
                "OIDC network entries must not be empty".to_string(),
            ));
        }
        validate_oidc_url(jwks_url, &network, "JWKS", false).await?;
        let client_id = client_id.trim();
        let authorization_endpoint = authorization_endpoint.trim();
        let token_endpoint = token_endpoint.trim();
        let redirect_uri = redirect_uri.trim();
        let browser_fields = [
            client_id,
            authorization_endpoint,
            token_endpoint,
            redirect_uri,
        ];
        let browser_configured = browser_fields.iter().any(|value| !value.is_empty());
        if browser_configured {
            if browser_fields.iter().any(|value| value.is_empty()) {
                return Err(OpsError::Invalid(
                    "clientId, authorizationEndpoint, tokenEndpoint and redirectUri must all be configured"
                        .to_string(),
                ));
            }
            validate_oidc_url(jwks_url, &network, "JWKS", true).await?;
            self.validate_oidc_endpoint(authorization_endpoint, &network, "authorization endpoint")
                .await?;
            self.validate_oidc_endpoint(token_endpoint, &network, "token endpoint")
                .await?;
            let redirect = validate_oidc_url(redirect_uri, &[], "redirect URI", false).await?;
            if !redirect.path().ends_with("/auth/oidc/callback") {
                return Err(OpsError::Invalid(
                    "redirectUri must end with /auth/oidc/callback".to_string(),
                ));
            }
        }
        if !(SESSION_TTL_MIN..=SESSION_TTL_MAX).contains(&session_ttl_seconds) {
            return Err(OpsError::Invalid(format!(
                "sessionTtlSeconds must be between {SESSION_TTL_MIN} and {SESSION_TTL_MAX}"
            )));
        }
        let mut seen = HashSet::new();
        let mut normalized_subjects = Vec::new();
        for subject in admin_subjects {
            let subject = subject.trim();
            if subject.is_empty() {
                return Err(OpsError::Invalid(
                    "adminSubjects entries must be non-empty strings".to_string(),
                ));
            }
            if seen.insert(subject.to_string()) {
                normalized_subjects.push(subject.to_string());
            }
        }
        let scopes = scopes.trim();
        let scopes = if scopes.is_empty() {
            DEFAULT_SCOPES
        } else {
            scopes
        };
        let admin_subjects_json = serde_json::to_string(&normalized_subjects)?;
        let network_json = serde_json::to_string(&network)?;
        sqlx::query(
            "INSERT INTO enterprise_oidc
             (id, issuer, audience, jwks_url, network_json, updated_at, client_id,
              authorization_endpoint, token_endpoint, redirect_uri, scopes,
              admin_subjects_json, cookie_secure, session_ttl_seconds)
             VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                issuer = excluded.issuer,
                audience = excluded.audience,
                jwks_url = excluded.jwks_url,
                network_json = excluded.network_json,
                updated_at = excluded.updated_at,
                client_id = excluded.client_id,
                authorization_endpoint = excluded.authorization_endpoint,
                token_endpoint = excluded.token_endpoint,
                redirect_uri = excluded.redirect_uri,
                scopes = excluded.scopes,
                admin_subjects_json = excluded.admin_subjects_json,
                cookie_secure = excluded.cookie_secure,
                session_ttl_seconds = excluded.session_ttl_seconds",
        )
        .bind(issuer)
        .bind(audience)
        .bind(jwks_url)
        .bind(&network_json)
        .bind(now_ms())
        .bind(client_id)
        .bind(authorization_endpoint)
        .bind(token_endpoint)
        .bind(redirect_uri)
        .bind(scopes)
        .bind(&admin_subjects_json)
        .bind(cookie_secure as i64)
        .bind(session_ttl_seconds)
        .execute(&self.control_pool)
        .await?;
        if let Ok(mut cache) = jwks_cache().lock() {
            cache.clear();
        }
        self.get_oidc_config().await
    }

    pub(crate) async fn validate_oidc_endpoint(
        &self,
        raw: &str,
        network: &[String],
        label: &str,
    ) -> Result<reqwest::Url, OpsError> {
        validate_oidc_url(raw, network, label, true).await
    }

    #[allow(clippy::type_complexity)]
    pub async fn get_oidc_config(&self) -> Result<Value, OpsError> {
        let row: Option<(
            String,
            String,
            String,
            String,
            i64,
            String,
            String,
            String,
            String,
            String,
            String,
            i64,
            i64,
        )> = sqlx::query_as(
            "SELECT issuer, audience, jwks_url, network_json, updated_at, client_id,
                    authorization_endpoint, token_endpoint, redirect_uri, scopes,
                    admin_subjects_json, cookie_secure, session_ttl_seconds
             FROM enterprise_oidc WHERE id = 1",
        )
        .fetch_optional(&self.control_pool)
        .await?;
        let Some((
            issuer,
            audience,
            jwks_url,
            network_json,
            updated_at,
            client_id,
            authorization_endpoint,
            token_endpoint,
            redirect_uri,
            scopes,
            admin_subjects_json,
            cookie_secure,
            session_ttl_seconds,
        )) = row
        else {
            return Ok(json!({
                "issuer": Value::Null,
                "audience": Value::Null,
                "jwksUrl": Value::Null,
                "network": Vec::<String>::new(),
                "clientId": "",
                "authorizationEndpoint": "",
                "tokenEndpoint": "",
                "redirectUri": "",
                "scopes": DEFAULT_SCOPES,
                "adminSubjects": Vec::<String>::new(),
                "cookieSecure": false,
                "sessionTtlSeconds": DEFAULT_SESSION_TTL,
                "updatedAt": Value::Null
            }));
        };
        let network: Vec<String> = serde_json::from_str(&network_json)
            .map_err(|_| OpsError::Invalid("stored OIDC network is invalid".to_string()))?;
        let admin_subjects: Vec<String> = serde_json::from_str(&admin_subjects_json)
            .map_err(|_| OpsError::Invalid("stored OIDC admin subjects are invalid".to_string()))?;
        Ok(json!({
            "issuer": issuer,
            "audience": audience,
            "jwksUrl": jwks_url,
            "network": network,
            "clientId": client_id,
            "authorizationEndpoint": authorization_endpoint,
            "tokenEndpoint": token_endpoint,
            "redirectUri": redirect_uri,
            "scopes": scopes,
            "adminSubjects": admin_subjects,
            "cookieSecure": cookie_secure != 0,
            "sessionTtlSeconds": session_ttl_seconds,
            "updatedAt": updated_at
        }))
    }

    pub async fn verify_oidc_token(&self, token: &str) -> Result<OidcPrincipal, OpsError> {
        self.verify_token(token, None).await
    }

    pub async fn verify_id_token(
        &self,
        token: &str,
        expected_nonce: &str,
    ) -> Result<OidcPrincipal, OpsError> {
        if expected_nonce.is_empty() {
            return Err(OpsError::Invalid(
                "expected nonce must not be empty".to_string(),
            ));
        }
        self.verify_token(token, Some(expected_nonce)).await
    }

    async fn verify_token(
        &self,
        token: &str,
        expected_nonce: Option<&str>,
    ) -> Result<OidcPrincipal, OpsError> {
        let row: Option<(String, String, String, String)> = sqlx::query_as(
            "SELECT issuer, audience, jwks_url, network_json FROM enterprise_oidc WHERE id = 1",
        )
        .fetch_optional(&self.control_pool)
        .await?;
        let Some((issuer, audience, jwks_url, network_json)) = row else {
            return Err(OpsError::Invalid(
                "OIDC is not configured for this control plane".to_string(),
            ));
        };
        let network: Vec<String> = serde_json::from_str(&network_json)
            .map_err(|_| OpsError::Invalid("stored OIDC network is invalid".to_string()))?;
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 3 || parts.iter().any(|part| part.is_empty()) {
            return Err(OpsError::Invalid("token is not a compact JWT".to_string()));
        }
        validate_oidc_url(&jwks_url, &network, "JWKS", true).await?;
        let keys = self.jwks_keys(&jwks_url).await?;
        oidc_verify::verify_jwt(token, &keys, &issuer, &audience, expected_nonce)
    }

    async fn fetch_worker(
        &self,
        id: &str,
    ) -> Result<
        (
            String,
            String,
            String,
            String,
            String,
            i64,
            i64,
            String,
            String,
            i64,
            Option<String>,
        ),
        OpsError,
    > {
        let row = sqlx::query_as(
            "SELECT id, tenant_id, name, kind, status, last_heartbeat_at, registered_at,
                    metadata_json, capabilities_json, max_concurrency, runtime_version
             FROM enterprise_workers WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.control_pool)
        .await?;
        row.ok_or_else(|| OpsError::NotFound(format!("worker `{id}` does not exist")))
    }

    async fn jwks_keys(&self, jwks_url: &str) -> Result<Vec<Jwk>, OpsError> {
        if let Ok(cache) = jwks_cache().lock()
            && let Some(entry) = cache.get(jwks_url)
            && entry.fetched_at.elapsed() < OIDC_CACHE_TTL
        {
            return Ok(entry.keys.clone());
        }
        let keys = fetch_jwks(jwks_url).await?;
        if let Ok(mut cache) = jwks_cache().lock() {
            cache.insert(
                jwks_url.to_string(),
                CachedJwks {
                    fetched_at: Instant::now(),
                    keys: keys.clone(),
                },
            );
        }
        Ok(keys)
    }

    fn tenant_db_path(&self, tenant_id: &str) -> Result<PathBuf, OpsError> {
        validate_tenant(tenant_id)?;
        if tenant_id == "local" {
            Ok(self.data_dir.join("hpa.db"))
        } else {
            Ok(self.data_dir.join("tenants").join(tenant_id).join("hpa.db"))
        }
    }

    fn inside_backups(&self, raw: &str) -> bool {
        Path::new(raw).starts_with(self.data_dir.join("backups"))
    }
}

fn validate_tenant(tenant_id: &str) -> Result<(), OpsError> {
    if tenant_id.is_empty()
        || !tenant_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err(OpsError::Invalid(
            "tenant id must match [A-Za-z0-9_-]+".to_string(),
        ));
    }
    Ok(())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn backup_json(
    id: &str,
    tenant_id: &str,
    path: &str,
    size: i64,
    sha256: &str,
    created_at: i64,
) -> Value {
    json!({
        "id": id,
        "tenantId": tenant_id,
        "path": path,
        "size": size,
        "sha256": sha256,
        "createdAt": created_at,
        "status": "ready"
    })
}

fn derived_status(status: &str, last_heartbeat_at: i64, now: i64) -> &'static str {
    if status == "active" && now - last_heartbeat_at > WORKER_OFFLINE_MS {
        "offline"
    } else if status == "disabled" {
        "disabled"
    } else if status == "offline" {
        "offline"
    } else {
        "active"
    }
}

#[allow(clippy::too_many_arguments)]
fn worker_json(
    id: &str,
    tenant_id: &str,
    name: &str,
    kind: &str,
    status: &str,
    last_heartbeat_at: i64,
    registered_at: i64,
    metadata: &Value,
    capabilities: &[String],
    max_concurrency: i64,
    runtime_version: Option<&str>,
) -> Value {
    json!({
        "id": id,
        "tenantId": tenant_id,
        "name": name,
        "kind": kind,
        "status": status,
        "lastHeartbeatAt": last_heartbeat_at,
        "registeredAt": registered_at,
        "metadata": metadata,
        "capabilities": capabilities,
        "maxConcurrency": max_concurrency,
        "runtimeVersion": runtime_version
    })
}

fn worker_capabilities(raw: &str) -> Vec<String> {
    serde_json::from_str(raw).unwrap_or_else(|_| vec!["workflow".to_string()])
}

async fn ensure_worker_columns(pool: &SqlitePool) -> Result<(), OpsError> {
    let exists: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'enterprise_workers'",
    )
    .fetch_optional(pool)
    .await?;
    if exists.is_none() {
        return Ok(());
    }
    let columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('enterprise_workers')")
            .fetch_all(pool)
            .await?;
    let additions = [
        (
            "capabilities_json",
            "ALTER TABLE enterprise_workers ADD COLUMN capabilities_json TEXT NOT NULL DEFAULT '[\"workflow\"]'",
        ),
        (
            "max_concurrency",
            "ALTER TABLE enterprise_workers ADD COLUMN max_concurrency INTEGER NOT NULL DEFAULT 1",
        ),
        (
            "runtime_version",
            "ALTER TABLE enterprise_workers ADD COLUMN runtime_version TEXT",
        ),
    ];
    for (name, statement) in additions {
        if !columns.iter().any(|column| column == name) {
            sqlx::query(statement).execute(pool).await?;
        }
    }
    Ok(())
}

async fn ensure_oidc_columns(pool: &SqlitePool) -> Result<(), OpsError> {
    let exists: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'enterprise_oidc'",
    )
    .fetch_optional(pool)
    .await?;
    if exists.is_none() {
        return Ok(());
    }
    let columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('enterprise_oidc')")
            .fetch_all(pool)
            .await?;
    let additions = [
        (
            "client_id",
            "ALTER TABLE enterprise_oidc ADD COLUMN client_id TEXT NOT NULL DEFAULT ''",
        ),
        (
            "authorization_endpoint",
            "ALTER TABLE enterprise_oidc ADD COLUMN authorization_endpoint TEXT NOT NULL DEFAULT ''",
        ),
        (
            "token_endpoint",
            "ALTER TABLE enterprise_oidc ADD COLUMN token_endpoint TEXT NOT NULL DEFAULT ''",
        ),
        (
            "redirect_uri",
            "ALTER TABLE enterprise_oidc ADD COLUMN redirect_uri TEXT NOT NULL DEFAULT ''",
        ),
        (
            "scopes",
            "ALTER TABLE enterprise_oidc ADD COLUMN scopes TEXT NOT NULL DEFAULT 'openid email profile'",
        ),
        (
            "admin_subjects_json",
            "ALTER TABLE enterprise_oidc ADD COLUMN admin_subjects_json TEXT NOT NULL DEFAULT '[]'",
        ),
        (
            "cookie_secure",
            "ALTER TABLE enterprise_oidc ADD COLUMN cookie_secure INTEGER NOT NULL DEFAULT 0",
        ),
        (
            "session_ttl_seconds",
            "ALTER TABLE enterprise_oidc ADD COLUMN session_ttl_seconds INTEGER NOT NULL DEFAULT 28800",
        ),
    ];
    for (name, statement) in additions {
        if !columns.iter().any(|column| column == name) {
            sqlx::query(statement).execute(pool).await?;
        }
    }
    Ok(())
}

async fn validate_oidc_url(
    raw: &str,
    network: &[String],
    label: &str,
    enforce_network: bool,
) -> Result<reqwest::Url, OpsError> {
    let url = reqwest::Url::parse(raw)
        .map_err(|error| OpsError::Invalid(format!("invalid {label} URL: {error}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(OpsError::Invalid(format!(
            "{label} URL must use http or https"
        )));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(OpsError::Forbidden(format!(
            "{label} URL must not contain userinfo"
        )));
    }
    let host = url
        .host_str()
        .ok_or_else(|| OpsError::Invalid(format!("{label} URL has no host")))?;
    let port = url.port_or_known_default().unwrap_or(443);
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let literal = bare.parse::<IpAddr>().ok();
    let loopback = host.eq_ignore_ascii_case("localhost")
        || literal.is_some_and(|address| address.is_loopback());
    if loopback {
        return Ok(url);
    }
    if url.scheme() != "https" {
        return Err(OpsError::Forbidden(format!(
            "non-loopback {label} URLs must use https"
        )));
    }
    if !enforce_network {
        return Ok(url);
    }
    if !network
        .iter()
        .any(|rule| host_rule_matches(rule, host, port))
    {
        return Err(OpsError::Forbidden(format!(
            "{label} host `{host}:{port}` is not in the configured network allowlist"
        )));
    }
    if let Some(address) = literal {
        if is_private_address(address) {
            return Err(OpsError::Forbidden(format!(
                "{label} host resolves to a private or link-local address"
            )));
        }
        return Ok(url);
    }
    let addresses = tokio::net::lookup_host((host, port))
        .await
        .map_err(|error| OpsError::Upstream(format!("cannot resolve {label} host: {error}")))?;
    let mut resolved = false;
    for address in addresses {
        resolved = true;
        if is_private_address(address.ip()) {
            return Err(OpsError::Forbidden(format!(
                "{label} host resolves to a private or link-local address"
            )));
        }
    }
    if !resolved {
        return Err(OpsError::Upstream(format!(
            "{label} host did not resolve to an address"
        )));
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

fn is_private_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            address.is_private()
                || address.is_loopback()
                || address.is_link_local()
                || address.is_broadcast()
                || address.is_unspecified()
                || address.is_documentation()
        }
        IpAddr::V6(address) => {
            address.is_loopback()
                || address.is_unspecified()
                || address.is_unique_local()
                || address.is_unicast_link_local()
                || address.is_multicast()
        }
    }
}

async fn fetch_jwks(jwks_url: &str) -> Result<Vec<Jwk>, OpsError> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| OpsError::Upstream(format!("cannot build JWKS client: {error}")))?;
    let response = client
        .get(jwks_url)
        .send()
        .await
        .map_err(|error| OpsError::Upstream(format!("JWKS request failed: {error}")))?;
    if !response.status().is_success() {
        return Err(OpsError::Upstream(format!(
            "JWKS endpoint returned {}",
            response.status()
        )));
    }
    if response
        .content_length()
        .is_some_and(|length| length as usize > JWKS_MAX_BYTES)
    {
        return Err(OpsError::Upstream("JWKS response is too large".to_string()));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| OpsError::Upstream(format!("cannot read JWKS response: {error}")))?;
    if bytes.len() > JWKS_MAX_BYTES {
        return Err(OpsError::Upstream("JWKS response is too large".to_string()));
    }
    let document: Value = serde_json::from_slice(&bytes)?;
    oidc_verify::parse_jwks(&document)
}

#[cfg(test)]
mod browser_sso_config_tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn pool() -> SqlitePool {
        SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap()
    }

    fn ops(pool: SqlitePool) -> OpsService {
        OpsService::new(pool, std::env::temp_dir().join("hpa-oidc-config-tests"))
    }

    #[tokio::test]
    async fn defaults_and_legacy_setter_preserve_browser_fields() {
        let pool = pool().await;
        let ops = ops(pool);
        ops.ensure_schema().await.unwrap();
        let empty = ops.get_oidc_config().await.unwrap();
        assert_eq!(empty["clientId"], "");
        assert_eq!(empty["authorizationEndpoint"], "");
        assert_eq!(empty["scopes"], "openid email profile");
        assert_eq!(empty["adminSubjects"], json!([]));
        assert_eq!(empty["cookieSecure"], false);
        assert_eq!(empty["sessionTtlSeconds"], 28800);

        let configured = ops
            .set_browser_oidc_config(
                "https://issuer.example/",
                "hpa",
                "http://127.0.0.1:9/jwks",
                vec!["*".to_string()],
                "client-1",
                "http://127.0.0.1:9/authorize",
                "http://127.0.0.1:9/token",
                "http://127.0.0.1:9/auth/oidc/callback",
                "openid email",
                vec![
                    "admin@example.com".to_string(),
                    " admin@example.com ".to_string(),
                    "subject-admin".to_string(),
                ],
                true,
                7200,
            )
            .await
            .unwrap();
        assert_eq!(configured["clientId"], "client-1");
        assert_eq!(
            configured["adminSubjects"],
            json!(["admin@example.com", "subject-admin"])
        );
        assert_eq!(configured["cookieSecure"], true);
        assert_eq!(configured["sessionTtlSeconds"], 7200);

        let updated = ops
            .set_oidc_config(
                "https://issuer.example/v2",
                "hpa",
                "http://127.0.0.1:9/jwks",
                vec!["*".to_string()],
            )
            .await
            .unwrap();
        assert_eq!(updated["issuer"], "https://issuer.example/v2");
        assert_eq!(updated["clientId"], "client-1");
        assert_eq!(
            updated["authorizationEndpoint"],
            "http://127.0.0.1:9/authorize"
        );
        assert_eq!(
            updated["redirectUri"],
            "http://127.0.0.1:9/auth/oidc/callback"
        );
        assert_eq!(
            updated["adminSubjects"],
            json!(["admin@example.com", "subject-admin"])
        );
        assert_eq!(updated["cookieSecure"], true);
    }

    #[tokio::test]
    async fn browser_config_validation() {
        let pool = pool().await;
        let ops = ops(pool);
        ops.ensure_schema().await.unwrap();
        let base = |client: &str, authorization: &str, token: &str, redirect: &str, ttl: i64| {
            (
                client.to_string(),
                authorization.to_string(),
                token.to_string(),
                redirect.to_string(),
                ttl,
            )
        };
        let all_empty = base("", "", "", "", 28800);
        let invalid = [
            base("client", "", "", "", 28800),
            base(
                "client",
                "http://idp.example/authorize",
                "https://idp.example/token",
                "https://hpa.example/auth/oidc/callback",
                28800,
            ),
            base(
                "client",
                "https://idp.example/authorize",
                "https://idp.example/token",
                "https://hpa.example/callback",
                28800,
            ),
            base(
                "client",
                "https://idp.example/authorize",
                "https://idp.example/token",
                "https://hpa.example/auth/oidc/callback",
                299,
            ),
            base(
                "client",
                "https://idp.example/authorize",
                "https://idp.example/token",
                "https://hpa.example/auth/oidc/callback",
                604_801,
            ),
        ];
        for (client, authorization, token, redirect, ttl) in invalid {
            assert!(
                ops.set_browser_oidc_config(
                    "https://issuer.example/",
                    "hpa",
                    "http://127.0.0.1:9/jwks",
                    vec!["*".to_string()],
                    &client,
                    &authorization,
                    &token,
                    &redirect,
                    "openid",
                    Vec::new(),
                    false,
                    ttl,
                )
                .await
                .is_err()
            );
        }
        let (client, authorization, token, redirect, ttl) = all_empty;
        assert!(
            ops.set_browser_oidc_config(
                "https://issuer.example/",
                "hpa",
                "http://127.0.0.1:9/jwks",
                vec!["*".to_string()],
                &client,
                &authorization,
                &token,
                &redirect,
                "openid",
                Vec::new(),
                false,
                ttl,
            )
            .await
            .is_ok()
        );
        assert!(
            ops.set_browser_oidc_config(
                "https://issuer.example/",
                "hpa",
                "https://idp.example/jwks",
                vec!["other.example".to_string()],
                "client",
                "https://idp.example/authorize",
                "https://idp.example/token",
                "https://hpa.example/auth/oidc/callback",
                "openid",
                Vec::new(),
                false,
                28800,
            )
            .await
            .is_err()
        );
        assert!(
            ops.set_browser_oidc_config(
                "https://issuer.example/",
                "hpa",
                "https://idp.example/jwks",
                vec!["other.example".to_string()],
                "",
                "",
                "",
                "",
                "openid",
                Vec::new(),
                false,
                28800,
            )
            .await
            .is_ok()
        );
        assert!(
            ops.set_browser_oidc_config(
                "https://issuer.example/",
                "hpa",
                "http://idp.example/jwks",
                vec!["*".to_string()],
                "",
                "",
                "",
                "",
                "openid",
                Vec::new(),
                false,
                28800,
            )
            .await
            .is_err()
        );
        assert!(
            ops.set_browser_oidc_config(
                "https://issuer.example/",
                "hpa",
                "https://user:pass@idp.example/jwks",
                vec!["*".to_string()],
                "",
                "",
                "",
                "",
                "openid",
                Vec::new(),
                false,
                28800,
            )
            .await
            .is_err()
        );
        assert!(
            ops.set_browser_oidc_config(
                "https://issuer.example/",
                "hpa",
                "http://127.0.0.1:9/jwks",
                vec!["*".to_string()],
                "",
                "",
                "",
                "",
                "openid",
                vec!["  ".to_string()],
                false,
                28800,
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn old_schema_is_migrated() {
        let pool = pool().await;
        sqlx::query(
            "CREATE TABLE enterprise_oidc (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                issuer TEXT NOT NULL,
                audience TEXT NOT NULL,
                jwks_url TEXT NOT NULL,
                network_json TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO enterprise_oidc VALUES (1, 'https://issuer.example/', 'hpa',
             'http://127.0.0.1:9/jwks', '[]', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let ops = ops(pool);
        ops.ensure_schema().await.unwrap();
        let config = ops.get_oidc_config().await.unwrap();
        assert_eq!(config["clientId"], "");
        assert_eq!(config["sessionTtlSeconds"], 28800);
        assert_eq!(config["adminSubjects"], json!([]));
    }
}
