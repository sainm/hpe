//! PostgreSQL-authoritative platform tokens and audit events.
//!
//! The PostgreSQL schema already mirrors the SQLite control plane; this module
//! is the code seam that makes the shared stores real so that tokens survive a
//! node losing its local `control.db`, and audit rows are visible to every
//! control-plane node.

use serde_json::Value;
use sqlx::Row;
use sqlx::postgres::{PgPool, PgRow};
use sqlx::types::Json;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid request: {0}")]
    Invalid(String),
    #[error("not found: {0}")]
    NotFound(String),
}

#[derive(Debug, Clone)]
pub struct TokenRecord {
    pub id: String,
    pub tenant_id: String,
    pub name: String,
    pub role: String,
    pub platform_admin: bool,
    pub created_at: OffsetDateTime,
    pub last_used_at: Option<OffsetDateTime>,
    pub expires_at: Option<OffsetDateTime>,
    pub revoked_at: Option<OffsetDateTime>,
}

impl TokenRecord {
    pub fn is_revoked(&self) -> bool {
        self.revoked_at.is_some()
    }

    pub fn is_expired(&self, now: OffsetDateTime) -> bool {
        self.expires_at.is_some_and(|expires| expires <= now)
    }
}

pub struct AuditEntry<'a> {
    pub tenant_id: Option<&'a str>,
    pub actor_id: Option<&'a str>,
    pub actor_type: &'a str,
    pub action: &'a str,
    pub resource: &'a str,
    pub decision: &'a str,
    pub reason: Option<&'a str>,
    pub detail: &'a Value,
}

#[derive(Clone)]
pub struct PostgresControlStore {
    pool: PgPool,
}

impl PostgresControlStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn begin_admin(&self) -> Result<sqlx::Transaction<'_, sqlx::Postgres>, ControlError> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query("SET LOCAL ROLE hpa_platform_admin")
            .execute(&mut *transaction)
            .await?;
        Ok(transaction)
    }

    pub async fn tenant_exists(&self, tenant_id: &str) -> Result<bool, ControlError> {
        let mut transaction = self.begin_admin().await?;
        let found: Option<String> =
            sqlx::query_scalar("SELECT id FROM hpa_control.tenants WHERE id = $1")
                .bind(tenant_id)
                .fetch_optional(&mut *transaction)
                .await?;
        transaction.commit().await?;
        Ok(found.is_some())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_token(
        &self,
        tenant_id: &str,
        name: &str,
        token_hash: &str,
        role: &str,
        platform_admin: bool,
        expires_at: Option<OffsetDateTime>,
    ) -> Result<TokenRecord, ControlError> {
        let mut transaction = self.begin_admin().await?;
        let row = sqlx::query(
            "INSERT INTO hpa_control.tokens
             (id, tenant_id, name, role, platform_admin, token_hash, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             RETURNING *",
        )
        .bind(format!("enttok_{}", Uuid::now_v7()))
        .bind(tenant_id)
        .bind(name)
        .bind(role)
        .bind(platform_admin)
        .bind(token_hash)
        .bind(expires_at)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        token_from_row(row)
    }

    pub async fn find_token_by_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<TokenRecord>, ControlError> {
        let mut transaction = self.begin_admin().await?;
        let row = sqlx::query("SELECT * FROM hpa_control.tokens WHERE token_hash = $1")
            .bind(token_hash)
            .fetch_optional(&mut *transaction)
            .await?;
        transaction.commit().await?;
        match row {
            Some(row) => Ok(Some(token_from_row(row)?)),
            None => Ok(None),
        }
    }

    pub async fn touch_token(
        &self,
        token_id: &str,
        at: OffsetDateTime,
    ) -> Result<(), ControlError> {
        let mut transaction = self.begin_admin().await?;
        sqlx::query("UPDATE hpa_control.tokens SET last_used_at = $2 WHERE id = $1")
            .bind(token_id)
            .bind(at)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn list_tokens(
        &self,
        tenant_id: Option<&str>,
    ) -> Result<Vec<TokenRecord>, ControlError> {
        let mut transaction = self.begin_admin().await?;
        let rows = sqlx::query(
            "SELECT * FROM hpa_control.tokens
             WHERE ($1::text IS NULL OR tenant_id = $1)
             ORDER BY created_at DESC",
        )
        .bind(tenant_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.into_iter().map(token_from_row).collect()
    }

    pub async fn revoke_token(
        &self,
        tenant_id: Option<&str>,
        token_id: &str,
        at: OffsetDateTime,
    ) -> Result<bool, ControlError> {
        let mut transaction = self.begin_admin().await?;
        let updated = sqlx::query(
            "UPDATE hpa_control.tokens SET revoked_at = $3
             WHERE id = $1 AND ($2::text IS NULL OR tenant_id = $2) AND revoked_at IS NULL",
        )
        .bind(token_id)
        .bind(tenant_id)
        .bind(at)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(updated.rows_affected() > 0)
    }

    /// Audit must never fail the request it describes. If the tenant is not yet
    /// known to PostgreSQL the row is retried with a NULL tenant instead of
    /// letting the foreign key reject it.
    pub async fn append_audit(&self, entry: AuditEntry<'_>) -> Result<(), ControlError> {
        match self.insert_audit(&entry, entry.tenant_id).await {
            Ok(()) => Ok(()),
            Err(ControlError::Db(error)) if is_foreign_key_violation(&error) => {
                self.insert_audit(&entry, None).await
            }
            Err(error) => Err(error),
        }
    }

    async fn insert_audit(
        &self,
        entry: &AuditEntry<'_>,
        tenant_id: Option<&str>,
    ) -> Result<(), ControlError> {
        let mut transaction = self.begin_admin().await?;
        sqlx::query(
            "INSERT INTO hpa_control.audit
             (id, tenant_id, actor_id, actor_type, action, resource, decision, reason, detail_json)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(format!("entaudit_{}", Uuid::now_v7()))
        .bind(tenant_id)
        .bind(entry.actor_id)
        .bind(entry.actor_type)
        .bind(entry.action)
        .bind(entry.resource)
        .bind(entry.decision)
        .bind(entry.reason)
        .bind(Json(entry.detail.clone()))
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }
}

fn token_from_row(row: PgRow) -> Result<TokenRecord, ControlError> {
    Ok(TokenRecord {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        name: row.try_get("name")?,
        role: row.try_get("role")?,
        platform_admin: row.try_get("platform_admin")?,
        created_at: row.try_get("created_at")?,
        last_used_at: row.try_get("last_used_at")?,
        expires_at: row.try_get("expires_at")?,
        revoked_at: row.try_get("revoked_at")?,
    })
}

fn is_foreign_key_violation(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(database) if database.code().as_deref() == Some("23503"))
}
