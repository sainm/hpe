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
use std::collections::BTreeSet;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
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

#[derive(Debug, Clone)]
pub struct TenantRecord {
    pub id: String,
    pub name: String,
    pub status: String,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaRecord {
    pub max_workflows: i64,
    pub max_executions_per_day: i64,
    pub max_skills: i64,
    pub max_agents: i64,
    pub max_plugins: i64,
    pub max_connections: i64,
    pub max_concurrent_runs: i64,
}

#[derive(Debug, Clone)]
pub struct MemberRecord {
    pub id: String,
    pub tenant_id: String,
    pub subject: String,
    pub display_name: String,
    pub email: Option<String>,
    pub role: String,
    pub status: String,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone)]
pub struct ScimMemberRecord {
    pub id: String,
    pub tenant_id: String,
    pub subject: String,
    pub user_name: Option<String>,
    pub external_id: Option<String>,
    pub display_name: String,
    pub email: Option<String>,
    pub role: String,
    pub base_role: String,
    pub scim_json: Value,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct ScimGroupRecord {
    pub id: String,
    pub tenant_id: String,
    pub display_name: String,
    pub external_id: Option<String>,
    pub role: Option<String>,
    pub scim_json: Value,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub id: String,
    pub token_hash: String,
    pub csrf_token: String,
    pub tenant_id: String,
    pub subject: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub role: String,
    pub platform_admin: bool,
    pub created_at: i64,
    pub expires_at: i64,
    pub revoked_at: Option<i64>,
    pub last_seen_at: Option<i64>,
    pub refresh_token_ciphertext: Option<String>,
    pub refresh_token_version: i32,
    pub refresh_token_expires_at: Option<i64>,
    pub id_token_ciphertext: Option<String>,
    pub access_token_expires_at: Option<i64>,
    pub last_refresh_at: Option<i64>,
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

    #[allow(clippy::too_many_arguments)]
    pub async fn create_session(&self, session: &SessionRecord) -> Result<(), ControlError> {
        let mut transaction = self.begin_admin().await?;
        sqlx::query(
            "INSERT INTO hpa_control.sessions
             (id, token_hash, csrf_token, tenant_id, subject, email, display_name, role,
              platform_admin, created_at, expires_at, revoked_at, last_seen_at,
              refresh_token_ciphertext, refresh_token_version, refresh_token_expires_at,
              id_token_ciphertext, access_token_expires_at, last_refresh_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
                     $15, $16, $17, $18, $19)",
        )
        .bind(&session.id)
        .bind(&session.token_hash)
        .bind(&session.csrf_token)
        .bind(&session.tenant_id)
        .bind(&session.subject)
        .bind(session.email.as_deref())
        .bind(session.display_name.as_deref())
        .bind(&session.role)
        .bind(session.platform_admin)
        .bind(session.created_at)
        .bind(session.expires_at)
        .bind(session.revoked_at)
        .bind(session.last_seen_at)
        .bind(session.refresh_token_ciphertext.as_deref())
        .bind(session.refresh_token_version)
        .bind(session.refresh_token_expires_at)
        .bind(session.id_token_ciphertext.as_deref())
        .bind(session.access_token_expires_at)
        .bind(session.last_refresh_at)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn session_by_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<SessionRecord>, ControlError> {
        let mut transaction = self.begin_admin().await?;
        let row = sqlx::query("SELECT * FROM hpa_control.sessions WHERE token_hash = $1")
            .bind(token_hash)
            .fetch_optional(&mut *transaction)
            .await?;
        transaction.commit().await?;
        match row {
            Some(row) => Ok(Some(SessionRecord {
                id: row.try_get("id")?,
                token_hash: row.try_get("token_hash")?,
                csrf_token: row.try_get("csrf_token")?,
                tenant_id: row.try_get("tenant_id")?,
                subject: row.try_get("subject")?,
                email: row.try_get("email")?,
                display_name: row.try_get("display_name")?,
                role: row.try_get("role")?,
                platform_admin: row.try_get("platform_admin")?,
                created_at: row.try_get("created_at")?,
                expires_at: row.try_get("expires_at")?,
                revoked_at: row.try_get("revoked_at")?,
                last_seen_at: row.try_get("last_seen_at")?,
                refresh_token_ciphertext: row.try_get("refresh_token_ciphertext")?,
                refresh_token_version: row.try_get("refresh_token_version")?,
                refresh_token_expires_at: row.try_get("refresh_token_expires_at")?,
                id_token_ciphertext: row.try_get("id_token_ciphertext")?,
                access_token_expires_at: row.try_get("access_token_expires_at")?,
                last_refresh_at: row.try_get("last_refresh_at")?,
            })),
            None => Ok(None),
        }
    }

    pub async fn touch_session(&self, session_id: &str, at: i64) -> Result<(), ControlError> {
        let mut transaction = self.begin_admin().await?;
        sqlx::query("UPDATE hpa_control.sessions SET last_seen_at = $2 WHERE id = $1")
            .bind(session_id)
            .bind(at)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn revoke_session_by_hash(
        &self,
        token_hash: &str,
        at: i64,
    ) -> Result<bool, ControlError> {
        let mut transaction = self.begin_admin().await?;
        let revoked = sqlx::query(
            "UPDATE hpa_control.sessions SET revoked_at = $2
             WHERE token_hash = $1 AND revoked_at IS NULL",
        )
        .bind(token_hash)
        .bind(at)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(revoked.rows_affected() > 0)
    }

    pub async fn revoke_session_by_id(
        &self,
        session_id: &str,
        at: i64,
    ) -> Result<(), ControlError> {
        let mut transaction = self.begin_admin().await?;
        sqlx::query(
            "UPDATE hpa_control.sessions SET revoked_at = $2
             WHERE id = $1 AND revoked_at IS NULL",
        )
        .bind(session_id)
        .bind(at)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn revoke_tenant_sessions(
        &self,
        tenant_id: &str,
        at: i64,
    ) -> Result<usize, ControlError> {
        let mut transaction = self.begin_admin().await?;
        let revoked = sqlx::query(
            "UPDATE hpa_control.sessions SET revoked_at = $2
             WHERE tenant_id = $1 AND revoked_at IS NULL",
        )
        .bind(tenant_id)
        .bind(at)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(revoked.rows_affected() as usize)
    }

    /// Mirrors the SQLite refresh rotation, including its fencing conditions.
    #[allow(clippy::too_many_arguments)]
    pub async fn rotate_session_tokens(
        &self,
        session_id: &str,
        expected_hash: &str,
        expected_version: i32,
        new_hash: &str,
        refresh_token_ciphertext: Option<&str>,
        refresh_token_expires_at: Option<i64>,
        id_token_ciphertext: Option<&str>,
        access_token_expires_at: Option<i64>,
        expires_at: i64,
        refreshed_at: i64,
    ) -> Result<bool, ControlError> {
        let mut transaction = self.begin_admin().await?;
        let updated = sqlx::query(
            "UPDATE hpa_control.sessions
             SET token_hash = $3, refresh_token_ciphertext = $4,
                 refresh_token_version = refresh_token_version + 1,
                 refresh_token_expires_at = $5, id_token_ciphertext = $6,
                 access_token_expires_at = $7, expires_at = $8,
                 last_refresh_at = $9, last_seen_at = $9
             WHERE id = $1 AND token_hash = $2 AND refresh_token_version = $10
               AND revoked_at IS NULL",
        )
        .bind(session_id)
        .bind(expected_hash)
        .bind(new_hash)
        .bind(refresh_token_ciphertext)
        .bind(refresh_token_expires_at)
        .bind(id_token_ciphertext)
        .bind(access_token_expires_at)
        .bind(expires_at)
        .bind(refreshed_at)
        .bind(expected_version)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn session_tokens(
        &self,
        session_id: &str,
    ) -> Result<Option<(Option<String>, Option<String>)>, ControlError> {
        let mut transaction = self.begin_admin().await?;
        let row = sqlx::query(
            "SELECT refresh_token_ciphertext, id_token_ciphertext
             FROM hpa_control.sessions WHERE id = $1",
        )
        .bind(session_id)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        match row {
            Some(row) => Ok(Some((row.try_get(0)?, row.try_get(1)?))),
            None => Ok(None),
        }
    }

    pub async fn upsert_tenant(&self, tenant: &TenantRecord) -> Result<(), ControlError> {
        let mut transaction = self.begin_admin().await?;
        sqlx::query(
            "INSERT INTO hpa_control.tenants (id, name, status, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (id) DO UPDATE
             SET name = EXCLUDED.name,
                 status = EXCLUDED.status,
                 updated_at = EXCLUDED.updated_at",
        )
        .bind(&tenant.id)
        .bind(&tenant.name)
        .bind(&tenant.status)
        .bind(tenant.created_at)
        .bind(tenant.updated_at)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn tenant(&self, tenant_id: &str) -> Result<Option<TenantRecord>, ControlError> {
        let mut transaction = self.begin_admin().await?;
        let row = sqlx::query(
            "SELECT id, name, status, created_at, updated_at FROM hpa_control.tenants WHERE id = $1",
        )
        .bind(tenant_id)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        match row {
            Some(row) => Ok(Some(TenantRecord {
                id: row.try_get("id")?,
                name: row.try_get("name")?,
                status: row.try_get("status")?,
                created_at: row.try_get("created_at")?,
                updated_at: row.try_get("updated_at")?,
            })),
            None => Ok(None),
        }
    }

    pub async fn list_tenants(&self) -> Result<Vec<TenantRecord>, ControlError> {
        let mut transaction = self.begin_admin().await?;
        let rows = sqlx::query(
            "SELECT id, name, status, created_at, updated_at FROM hpa_control.tenants
             ORDER BY created_at ASC",
        )
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.into_iter()
            .map(|row| {
                Ok(TenantRecord {
                    id: row.try_get("id")?,
                    name: row.try_get("name")?,
                    status: row.try_get("status")?,
                    created_at: row.try_get("created_at")?,
                    updated_at: row.try_get("updated_at")?,
                })
            })
            .collect()
    }

    pub async fn upsert_quota(
        &self,
        tenant_id: &str,
        quota: &QuotaRecord,
    ) -> Result<(), ControlError> {
        let mut transaction = self.begin_admin().await?;
        sqlx::query(
            "INSERT INTO hpa_control.quotas
             (tenant_id, max_workflows, max_executions_per_day, max_skills, max_agents,
              max_plugins, max_connections, max_concurrent_runs)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             ON CONFLICT (tenant_id) DO UPDATE
             SET max_workflows = EXCLUDED.max_workflows,
                 max_executions_per_day = EXCLUDED.max_executions_per_day,
                 max_skills = EXCLUDED.max_skills,
                 max_agents = EXCLUDED.max_agents,
                 max_plugins = EXCLUDED.max_plugins,
                 max_connections = EXCLUDED.max_connections,
                 max_concurrent_runs = EXCLUDED.max_concurrent_runs",
        )
        .bind(tenant_id)
        .bind(quota.max_workflows)
        .bind(quota.max_executions_per_day)
        .bind(quota.max_skills)
        .bind(quota.max_agents)
        .bind(quota.max_plugins)
        .bind(quota.max_connections)
        .bind(quota.max_concurrent_runs)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn quota(&self, tenant_id: &str) -> Result<Option<QuotaRecord>, ControlError> {
        let mut transaction = self.begin_admin().await?;
        let row = sqlx::query(
            "SELECT max_workflows, max_executions_per_day, max_skills, max_agents,
                    max_plugins, max_connections, max_concurrent_runs
             FROM hpa_control.quotas WHERE tenant_id = $1",
        )
        .bind(tenant_id)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        match row {
            Some(row) => Ok(Some(QuotaRecord {
                max_workflows: row.try_get("max_workflows")?,
                max_executions_per_day: row.try_get("max_executions_per_day")?,
                max_skills: row.try_get("max_skills")?,
                max_agents: row.try_get("max_agents")?,
                max_plugins: row.try_get("max_plugins")?,
                max_connections: row.try_get("max_connections")?,
                max_concurrent_runs: row.try_get("max_concurrent_runs")?,
            })),
            None => Ok(None),
        }
    }

    pub async fn upsert_member(&self, member: &MemberRecord) -> Result<(), ControlError> {
        let mut transaction = self.begin_admin().await?;
        sqlx::query(
            "INSERT INTO hpa_control.members
             (id, tenant_id, subject, display_name, email, role, status, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             ON CONFLICT (id) DO UPDATE
             SET subject = EXCLUDED.subject,
                 display_name = EXCLUDED.display_name,
                 email = EXCLUDED.email,
                 role = EXCLUDED.role,
                 status = EXCLUDED.status,
                 updated_at = EXCLUDED.updated_at",
        )
        .bind(&member.id)
        .bind(&member.tenant_id)
        .bind(&member.subject)
        .bind(&member.display_name)
        .bind(member.email.as_deref())
        .bind(&member.role)
        .bind(&member.status)
        .bind(member.created_at)
        .bind(member.updated_at)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn delete_member(&self, member_id: &str) -> Result<bool, ControlError> {
        let mut transaction = self.begin_admin().await?;
        let deleted = sqlx::query("DELETE FROM hpa_control.members WHERE id = $1")
            .bind(member_id)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(deleted.rows_affected() > 0)
    }

    pub async fn list_members(
        &self,
        tenant_id: Option<&str>,
    ) -> Result<Vec<MemberRecord>, ControlError> {
        let mut transaction = self.begin_admin().await?;
        let rows = sqlx::query(
            "SELECT id, tenant_id, subject, display_name, email, role, status, created_at, updated_at
             FROM hpa_control.members
             WHERE ($1::text IS NULL OR tenant_id = $1)
             ORDER BY created_at ASC",
        )
        .bind(tenant_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.into_iter()
            .map(|row| {
                Ok(MemberRecord {
                    id: row.try_get("id")?,
                    tenant_id: row.try_get("tenant_id")?,
                    subject: row.try_get("subject")?,
                    display_name: row.try_get("display_name")?,
                    email: row.try_get("email")?,
                    role: row.try_get("role")?,
                    status: row.try_get("status")?,
                    created_at: row.try_get("created_at")?,
                    updated_at: row.try_get("updated_at")?,
                })
            })
            .collect()
    }

    pub async fn scim_member_by_id(
        &self,
        tenant_id: &str,
        id: &str,
    ) -> Result<Option<ScimMemberRecord>, ControlError> {
        let mut transaction = self.begin_admin().await?;
        let row = sqlx::query(
            "SELECT id, tenant_id, subject, user_name, external_id, display_name, email,
                    role, base_role, scim_json, status, created_at, updated_at
             FROM hpa_control.members WHERE tenant_id = $1 AND id = $2",
        )
        .bind(tenant_id)
        .bind(id)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.as_ref().map(scim_member_from_row).transpose()
    }

    pub async fn scim_member_for_identity(
        &self,
        tenant_id: &str,
        subject: &str,
        email: Option<&str>,
    ) -> Result<Option<ScimMemberRecord>, ControlError> {
        let mut transaction = self.begin_admin().await?;
        let row = sqlx::query(
            "SELECT id, tenant_id, subject, user_name, external_id, display_name, email,
                    role, base_role, scim_json, status, created_at, updated_at
             FROM hpa_control.members
             WHERE tenant_id = $1
               AND (subject = $2
                    OR ($3::text IS NOT NULL AND email IS NOT NULL
                        AND lower(email) = lower($3)))
             ORDER BY CASE WHEN subject = $2 THEN 0 ELSE 1 END
             LIMIT 1",
        )
        .bind(tenant_id)
        .bind(subject)
        .bind(email)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.as_ref().map(scim_member_from_row).transpose()
    }

    pub async fn scim_list_members(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<ScimMemberRecord>, ControlError> {
        let mut transaction = self.begin_admin().await?;
        let rows = sqlx::query(
            "SELECT id, tenant_id, subject, user_name, external_id, display_name, email,
                    role, base_role, scim_json, status, created_at, updated_at
             FROM hpa_control.members WHERE tenant_id = $1
             ORDER BY created_at DESC",
        )
        .bind(tenant_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.iter().map(scim_member_from_row).collect()
    }

    pub async fn scim_create_member(&self, member: &ScimMemberRecord) -> Result<(), ControlError> {
        let mut transaction = self.begin_admin().await?;
        sqlx::query(
            "INSERT INTO hpa_control.members
             (id, tenant_id, subject, user_name, external_id, display_name, email, role,
              base_role, scim_json, status, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
        )
        .bind(&member.id)
        .bind(&member.tenant_id)
        .bind(&member.subject)
        .bind(member.user_name.as_deref())
        .bind(member.external_id.as_deref())
        .bind(&member.display_name)
        .bind(member.email.as_deref())
        .bind(&member.role)
        .bind(&member.base_role)
        .bind(Json(member.scim_json.clone()))
        .bind(&member.status)
        .bind(parse_scim_time(&member.created_at)?)
        .bind(parse_scim_time(&member.updated_at)?)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn scim_update_member(&self, member: &ScimMemberRecord) -> Result<(), ControlError> {
        let mut transaction = self.begin_admin().await?;
        sqlx::query(
            "UPDATE hpa_control.members
             SET subject = $1, user_name = $2, external_id = $3, display_name = $4,
                 email = $5, role = $6, base_role = $7, scim_json = $8, status = $9,
                 updated_at = $10
             WHERE tenant_id = $11 AND id = $12",
        )
        .bind(&member.subject)
        .bind(member.user_name.as_deref())
        .bind(member.external_id.as_deref())
        .bind(&member.display_name)
        .bind(member.email.as_deref())
        .bind(&member.role)
        .bind(&member.base_role)
        .bind(Json(member.scim_json.clone()))
        .bind(&member.status)
        .bind(parse_scim_time(&member.updated_at)?)
        .bind(&member.tenant_id)
        .bind(&member.id)
        .execute(&mut *transaction)
        .await?;
        sync_member_role(&mut transaction, &member.tenant_id, &member.id).await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn scim_delete_member(
        &self,
        tenant_id: &str,
        id: &str,
    ) -> Result<bool, ControlError> {
        let mut transaction = self.begin_admin().await?;
        let deleted =
            sqlx::query("DELETE FROM hpa_control.members WHERE tenant_id = $1 AND id = $2")
                .bind(tenant_id)
                .bind(id)
                .execute(&mut *transaction)
                .await?;
        transaction.commit().await?;
        Ok(deleted.rows_affected() > 0)
    }

    pub async fn revoke_tenant_tokens(&self, tenant_id: &str) -> Result<usize, ControlError> {
        let mut transaction = self.begin_admin().await?;
        let revoked = sqlx::query(
            "UPDATE hpa_control.tokens SET revoked_at = now(), updated_at = now()
             WHERE tenant_id = $1 AND revoked_at IS NULL",
        )
        .bind(tenant_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(revoked.rows_affected() as usize)
    }

    pub async fn scim_member_any_id(
        &self,
        id: &str,
    ) -> Result<Option<ScimMemberRecord>, ControlError> {
        let mut transaction = self.begin_admin().await?;
        let row = sqlx::query(
            "SELECT id, tenant_id, subject, user_name, external_id, display_name, email,
                    role, base_role, scim_json, status, created_at, updated_at
             FROM hpa_control.members WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.as_ref().map(scim_member_from_row).transpose()
    }

    pub async fn scim_delete_member_any_id(&self, id: &str) -> Result<bool, ControlError> {
        let mut transaction = self.begin_admin().await?;
        let deleted = sqlx::query("DELETE FROM hpa_control.members WHERE id = $1")
            .bind(id)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(deleted.rows_affected() > 0)
    }

    pub async fn scim_member_groups(
        &self,
        tenant_id: &str,
        member_id: &str,
    ) -> Result<Vec<(String, String)>, ControlError> {
        let mut transaction = self.begin_admin().await?;
        let rows = sqlx::query(
            "SELECT g.id, g.display_name
             FROM hpa_control.scim_groups g
             JOIN hpa_control.scim_group_members gm
               ON gm.group_id = g.id AND gm.tenant_id = g.tenant_id
             WHERE gm.tenant_id = $1 AND gm.member_id = $2
             ORDER BY lower(g.display_name)",
        )
        .bind(tenant_id)
        .bind(member_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.iter()
            .map(|row| Ok((row.try_get("id")?, row.try_get("display_name")?)))
            .collect()
    }

    pub async fn scim_group_roles_for_member(
        &self,
        tenant_id: &str,
        member_id: &str,
    ) -> Result<Vec<String>, ControlError> {
        let mut transaction = self.begin_admin().await?;
        let roles = sqlx::query_scalar(
            "SELECT DISTINCT g.role
             FROM hpa_control.scim_groups g
             JOIN hpa_control.scim_group_members gm
               ON gm.group_id = g.id AND gm.tenant_id = g.tenant_id
             WHERE gm.tenant_id = $1 AND gm.member_id = $2 AND g.role IS NOT NULL
             ORDER BY g.role",
        )
        .bind(tenant_id)
        .bind(member_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(roles)
    }

    pub async fn scim_group_by_id(
        &self,
        tenant_id: &str,
        id: &str,
    ) -> Result<Option<ScimGroupRecord>, ControlError> {
        let mut transaction = self.begin_admin().await?;
        let row = sqlx::query(
            "SELECT id, tenant_id, display_name, external_id, role, scim_json,
                    created_at, updated_at
             FROM hpa_control.scim_groups WHERE tenant_id = $1 AND id = $2",
        )
        .bind(tenant_id)
        .bind(id)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.as_ref().map(scim_group_from_row).transpose()
    }

    pub async fn scim_list_groups(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<ScimGroupRecord>, ControlError> {
        let mut transaction = self.begin_admin().await?;
        let rows = sqlx::query(
            "SELECT id, tenant_id, display_name, external_id, role, scim_json,
                    created_at, updated_at
             FROM hpa_control.scim_groups WHERE tenant_id = $1
             ORDER BY created_at DESC",
        )
        .bind(tenant_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.iter().map(scim_group_from_row).collect()
    }

    pub async fn scim_group_members(
        &self,
        tenant_id: &str,
        group_id: &str,
    ) -> Result<Vec<ScimMemberRecord>, ControlError> {
        let mut transaction = self.begin_admin().await?;
        let rows = sqlx::query(
            "SELECT m.id, m.tenant_id, m.subject, m.user_name, m.external_id,
                    m.display_name, m.email, m.role, m.base_role, m.scim_json,
                    m.status, m.created_at, m.updated_at
             FROM hpa_control.members m
             JOIN hpa_control.scim_group_members gm
               ON gm.member_id = m.id AND gm.tenant_id = m.tenant_id
             WHERE gm.tenant_id = $1 AND gm.group_id = $2
             ORDER BY lower(m.display_name)",
        )
        .bind(tenant_id)
        .bind(group_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.iter().map(scim_member_from_row).collect()
    }

    pub async fn scim_create_group(
        &self,
        group: &ScimGroupRecord,
        members: &BTreeSet<String>,
    ) -> Result<(), ControlError> {
        let mut transaction = self.begin_admin().await?;
        sqlx::query(
            "INSERT INTO hpa_control.scim_groups
             (id, tenant_id, display_name, external_id, role, scim_json, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(&group.id)
        .bind(&group.tenant_id)
        .bind(&group.display_name)
        .bind(group.external_id.as_deref())
        .bind(group.role.as_deref())
        .bind(Json(group.scim_json.clone()))
        .bind(parse_scim_time(&group.created_at)?)
        .bind(parse_scim_time(&group.updated_at)?)
        .execute(&mut *transaction)
        .await?;
        replace_group_members(&mut transaction, &group.tenant_id, &group.id, members).await?;
        for member_id in members {
            sync_member_role(&mut transaction, &group.tenant_id, member_id).await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn scim_update_group(
        &self,
        group: &ScimGroupRecord,
        members: &BTreeSet<String>,
    ) -> Result<(), ControlError> {
        let mut transaction = self.begin_admin().await?;
        let before: Vec<String> = sqlx::query_scalar(
            "SELECT member_id FROM hpa_control.scim_group_members
             WHERE tenant_id = $1 AND group_id = $2",
        )
        .bind(&group.tenant_id)
        .bind(&group.id)
        .fetch_all(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE hpa_control.scim_groups
             SET display_name = $1, external_id = $2, role = $3, scim_json = $4,
                 updated_at = $5
             WHERE tenant_id = $6 AND id = $7",
        )
        .bind(&group.display_name)
        .bind(group.external_id.as_deref())
        .bind(group.role.as_deref())
        .bind(Json(group.scim_json.clone()))
        .bind(parse_scim_time(&group.updated_at)?)
        .bind(&group.tenant_id)
        .bind(&group.id)
        .execute(&mut *transaction)
        .await?;
        replace_group_members(&mut transaction, &group.tenant_id, &group.id, members).await?;
        let mut affected: BTreeSet<String> = before.into_iter().collect();
        affected.extend(members.iter().cloned());
        for member_id in affected {
            sync_member_role(&mut transaction, &group.tenant_id, &member_id).await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn scim_delete_group(
        &self,
        tenant_id: &str,
        group_id: &str,
    ) -> Result<(), ControlError> {
        let mut transaction = self.begin_admin().await?;
        let affected: Vec<String> = sqlx::query_scalar(
            "SELECT member_id FROM hpa_control.scim_group_members
             WHERE tenant_id = $1 AND group_id = $2",
        )
        .bind(tenant_id)
        .bind(group_id)
        .fetch_all(&mut *transaction)
        .await?;
        sqlx::query("DELETE FROM hpa_control.scim_groups WHERE tenant_id = $1 AND id = $2")
            .bind(tenant_id)
            .bind(group_id)
            .execute(&mut *transaction)
            .await?;
        for member_id in affected {
            sync_member_role(&mut transaction, tenant_id, &member_id).await?;
        }
        transaction.commit().await?;
        Ok(())
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

fn scim_member_from_row(row: &PgRow) -> Result<ScimMemberRecord, ControlError> {
    Ok(ScimMemberRecord {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        subject: row.try_get("subject")?,
        user_name: row.try_get("user_name")?,
        external_id: row.try_get("external_id")?,
        display_name: row.try_get("display_name")?,
        email: row.try_get("email")?,
        role: row.try_get("role")?,
        base_role: row.try_get("base_role")?,
        scim_json: row.try_get("scim_json")?,
        status: row.try_get("status")?,
        created_at: format_scim_time(row.try_get("created_at")?),
        updated_at: format_scim_time(row.try_get("updated_at")?),
    })
}

fn scim_group_from_row(row: &PgRow) -> Result<ScimGroupRecord, ControlError> {
    Ok(ScimGroupRecord {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        display_name: row.try_get("display_name")?,
        external_id: row.try_get("external_id")?,
        role: row.try_get("role")?,
        scim_json: row.try_get("scim_json")?,
        created_at: format_scim_time(row.try_get("created_at")?),
        updated_at: format_scim_time(row.try_get("updated_at")?),
    })
}

fn parse_scim_time(value: &str) -> Result<OffsetDateTime, ControlError> {
    OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|error| ControlError::Invalid(format!("invalid timestamp `{value}`: {error}")))
}

fn format_scim_time(value: OffsetDateTime) -> String {
    value.format(&Rfc3339).unwrap_or_default()
}

async fn replace_group_members(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: &str,
    group_id: &str,
    members: &BTreeSet<String>,
) -> Result<(), ControlError> {
    for member_id in members {
        let exists: Option<String> = sqlx::query_scalar(
            "SELECT id FROM hpa_control.members WHERE tenant_id = $1 AND id = $2",
        )
        .bind(tenant_id)
        .bind(member_id)
        .fetch_optional(&mut **transaction)
        .await?;
        if exists.is_none() {
            return Err(ControlError::Invalid(format!(
                "group member `{member_id}` does not exist in this tenant"
            )));
        }
    }
    sqlx::query(
        "DELETE FROM hpa_control.scim_group_members WHERE tenant_id = $1 AND group_id = $2",
    )
    .bind(tenant_id)
    .bind(group_id)
    .execute(&mut **transaction)
    .await?;
    let now = OffsetDateTime::now_utc();
    for member_id in members {
        sqlx::query(
            "INSERT INTO hpa_control.scim_group_members
             (tenant_id, group_id, member_id, created_at)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(tenant_id)
        .bind(group_id)
        .bind(member_id)
        .bind(now)
        .execute(&mut **transaction)
        .await?;
    }
    Ok(())
}

async fn sync_member_role(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: &str,
    member_id: &str,
) -> Result<(), ControlError> {
    let base_role: Option<String> = sqlx::query_scalar(
        "SELECT base_role FROM hpa_control.members WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id)
    .bind(member_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some(base_role) = base_role else {
        return Err(ControlError::Invalid(format!(
            "member `{member_id}` does not exist"
        )));
    };
    let roles: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT g.role
         FROM hpa_control.scim_groups g
         JOIN hpa_control.scim_group_members gm
           ON gm.group_id = g.id AND gm.tenant_id = g.tenant_id
         WHERE gm.tenant_id = $1 AND gm.member_id = $2 AND g.role IS NOT NULL
         ORDER BY g.role",
    )
    .bind(tenant_id)
    .bind(member_id)
    .fetch_all(&mut **transaction)
    .await?;
    if roles.len() > 1 {
        return Err(ControlError::Invalid(
            "member belongs to SCIM Groups mapped to different roles".to_string(),
        ));
    }
    let effective_role = roles.first().cloned().unwrap_or(base_role);
    sqlx::query(
        "UPDATE hpa_control.members SET role = $1, updated_at = $2
         WHERE tenant_id = $3 AND id = $4",
    )
    .bind(effective_role)
    .bind(OffsetDateTime::now_utc())
    .bind(tenant_id)
    .bind(member_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
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
