use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path as AxumPath, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{any, delete, get, post, put};
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hpa_core::compiler::compile_workflow;
use hpa_core::ir::Workflow;
use hpa_core::runtime::{
    ExecutionEvent, ExecutionState, cancel_execution as cancel_execution_state, new_state,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tower::ServiceExt;
use uuid::Uuid;

use crate::agent::AgentRuntime;
use crate::api::{ApiError, ApiState};
use crate::connector::ConnectorRuntime;
use crate::db::{Db, ExecutionRow, parse_state, status_text};
use crate::driver::RuntimeManager;
use crate::enterprise_ops::browser_auth::{self, BrowserAuth};
use crate::enterprise_ops::{OpsError, OpsService};
use crate::oauth::{OauthOwner, OauthService};
use crate::plugin::PluginService;
use crate::skills::SkillService;
use crate::worker::{WorkerAuth, WorkerError, WorkerErrorKind, WorkerJob};
use hpa_local::postgres_control::{
    AuditEntry, ControlError, MemberRecord, PostgresControlStore, QuotaRecord, ScimMemberRecord,
    TenantRecord, TokenRecord,
};

#[path = "scim.rs"]
mod scim;

const LOCAL_TENANT: &str = "local";
const DEFAULT_ROLE: &str = "owner";

#[derive(Clone)]
pub struct EnterpriseState {
    control: SqlitePool,
    data_dir: PathBuf,
    mcp_base_url: String,
    require_auth: bool,
    ops: OpsService,
    browser_auth: BrowserAuth,
    oauth: OauthService,
    workers: crate::worker::WorkerService,
    postgres_execution: Option<sqlx::postgres::PgPool>,
    postgres_replica: Option<sqlx::postgres::PgPool>,
    postgres_event_bus: Option<sqlx::postgres::PgPool>,
    cache: Arc<tokio::sync::Mutex<HashMap<String, TenantEntry>>>,
}

#[derive(Clone)]
struct TenantEntry {
    router: Router,
    state: ApiState,
}

#[derive(Clone, Debug)]
pub struct Principal {
    token_id: String,
    tenant_id: String,
    role: String,
    platform_admin: bool,
}

#[derive(Debug, Clone)]
struct Quota {
    max_workflows: i64,
    max_executions_per_day: i64,
    max_skills: i64,
    max_agents: i64,
    max_plugins: i64,
    max_connections: i64,
    max_concurrent_runs: i64,
}

impl EnterpriseState {
    pub async fn new(
        data_dir: PathBuf,
        mcp_base_url: String,
        require_auth: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        tokio::fs::create_dir_all(&data_dir).await?;
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(data_dir.join("control.db"))
                    .create_if_missing(true)
                    .foreign_keys(true),
            )
            .await?;
        ensure_control_schema(&pool).await?;
        let ops = OpsService::new(pool.clone(), data_dir.clone());
        ops.ensure_schema().await?;
        let browser_auth = BrowserAuth::new(pool.clone(), ops.clone());
        browser_auth.ensure_schema().await?;
        let oauth = OauthService::new(pool.clone(), mcp_base_url.clone());
        oauth.ensure_schema().await?;
        let _ = oauth.purge_expired().await;
        let workers = crate::worker::WorkerService::new(pool.clone());
        workers.ensure_schema().await?;
        let now = now_rfc3339();
        sqlx::query(
            "INSERT INTO enterprise_tenants (id, name, status, created_at, updated_at)
             VALUES (?, 'Local', 'active', ?, ?)
             ON CONFLICT(id) DO NOTHING",
        )
        .bind(LOCAL_TENANT)
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await?;
        sqlx::query(
            "INSERT INTO enterprise_quotas
             (tenant_id, max_workflows, max_executions_per_day, max_skills, max_agents,
              max_plugins, max_connections, max_concurrent_runs)
             VALUES (?, 100, 1000, 100, 100, 50, 100, 8)
             ON CONFLICT(tenant_id) DO NOTHING",
        )
        .bind(LOCAL_TENANT)
        .execute(&pool)
        .await?;
        Ok(Self {
            control: pool,
            data_dir,
            mcp_base_url,
            require_auth,
            ops,
            browser_auth,
            oauth,
            workers,
            postgres_execution: None,
            postgres_replica: None,
            postgres_event_bus: None,
            cache: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        })
    }

    pub fn with_postgres_execution(mut self, pool: sqlx::postgres::PgPool) -> Self {
        self.workers = self.workers.clone().with_postgres(pool.clone());
        self.browser_auth = self.browser_auth.clone().with_postgres(pool.clone());
        self.oauth = self.oauth.clone().with_postgres(pool.clone());
        self.postgres_execution = Some(pool);
        self.cache = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        self
    }

    /// Attaches an optional read replica used by console listings.
    pub fn with_postgres_replica(mut self, pool: sqlx::postgres::PgPool) -> Self {
        self.postgres_replica = Some(pool);
        self.cache = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        self
    }

    pub fn with_postgres_event_bus(mut self, pool: sqlx::postgres::PgPool) -> Self {
        self.postgres_event_bus = Some(pool);
        self.cache = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        self
    }

    async fn ensure_postgres_tenant(&self, tenant_id: &str) -> Result<(), ApiError> {
        let Some(pool) = self.postgres_execution.as_ref() else {
            return Ok(());
        };
        // A node that never saw this tenant must not fail here: if PostgreSQL
        // already knows it, the mirror has nothing to do.
        if let Some(control) = self.control_store()
            && control
                .tenant(tenant_id)
                .await
                .map_err(|error| ApiError::internal(error.to_string()))?
                .is_some()
        {
            return Ok(());
        }
        let row = sqlx::query(
            "SELECT name, status, created_at, updated_at
             FROM enterprise_tenants WHERE id = ?",
        )
        .bind(tenant_id)
        .fetch_optional(&self.control)
        .await
        .map_err(internal)?
        .ok_or_else(|| ApiError::not_found(format!("tenant `{tenant_id}` not found")))?;
        let created_at =
            OffsetDateTime::parse(row.get::<String, _>("created_at").as_str(), &Rfc3339).map_err(
                |error| ApiError::internal(format!("invalid tenant created_at: {error}")),
            )?;
        let updated_at =
            OffsetDateTime::parse(row.get::<String, _>("updated_at").as_str(), &Rfc3339).map_err(
                |error| ApiError::internal(format!("invalid tenant updated_at: {error}")),
            )?;
        sqlx::query(
            "INSERT INTO hpa_control.tenants (
                 id, name, status, created_at, updated_at
             )
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (id) DO UPDATE
             SET name = EXCLUDED.name,
                 status = EXCLUDED.status,
                 updated_at = EXCLUDED.updated_at",
        )
        .bind(tenant_id)
        .bind(row.get::<String, _>("name"))
        .bind(row.get::<String, _>("status"))
        .bind(created_at)
        .bind(updated_at)
        .execute(pool)
        .await
        .map_err(internal)?;
        Ok(())
    }

    pub async fn local_state(&self) -> Result<ApiState, ApiError> {
        Ok(self.tenant_entry(LOCAL_TENANT).await?.state)
    }

    /// Tenant ids that the scheduler and hooks may serve.
    pub(crate) async fn active_tenants(&self) -> Result<Vec<String>, ApiError> {
        if let Some(control) = self.control_store() {
            let tenants = control
                .list_tenants()
                .await
                .map_err(|error| ApiError::internal(error.to_string()))?;
            if !tenants.is_empty() {
                return Ok(tenants
                    .into_iter()
                    .filter(|tenant| tenant.status == "active")
                    .map(|tenant| tenant.id)
                    .collect());
            }
        }
        sqlx::query_scalar(
            "SELECT id FROM enterprise_tenants WHERE status = 'active' ORDER BY created_at ASC",
        )
        .fetch_all(&self.control)
        .await
        .map_err(internal)
    }

    pub(crate) fn auth_required(&self) -> bool {
        self.require_auth
    }

    pub(crate) fn uses_postgres(&self) -> bool {
        self.postgres_execution.is_some()
    }

    /// PostgreSQL control-plane store, present only in `HPA_POSTGRES_RUNTIME=1`.
    fn control_store(&self) -> Option<PostgresControlStore> {
        self.postgres_execution
            .as_ref()
            .cloned()
            .map(PostgresControlStore::new)
    }

    pub(crate) async fn expire_worker_approvals(&self) -> Result<usize, ApiError> {
        let jobs = self
            .workers
            .waiting_approval_jobs()
            .await
            .map_err(worker_error)?;
        let mut expired = 0usize;
        for job in jobs {
            let mut state: ExecutionState = serde_json::from_str(&job.state_json)
                .map_err(|error| ApiError::internal(error.to_string()))?;
            let Some(request) = state.waiting.clone() else {
                if state.status != hpa_core::runtime::ExecutionStatus::WaitingApproval {
                    self.workers
                        .requeue_execution(&job.execution_id, &job.state_json)
                        .await
                        .map_err(worker_error)?;
                    expired += 1;
                }
                continue;
            };
            if !request.is_overdue(OffsetDateTime::now_utc()) {
                continue;
            }
            let workflow: Workflow = serde_json::from_str(&job.workflow_json)
                .map_err(|error| ApiError::internal(error.to_string()))?;
            let compilation = compile_workflow(&workflow)
                .map_err(|issues| ApiError::internal(format!("{issues:?}")))?;
            let events = hpa_core::runtime::expire_approval(
                &mut state,
                &compilation.plan,
                &request.node_id,
                json!({"automatic": true, "reason": "sla", "remote": true}),
            )
            .map_err(|error| ApiError::internal(error.to_string()))?;
            let state_json = serde_json::to_string(&state)
                .map_err(|error| ApiError::internal(error.to_string()))?;
            let entry = self.tenant_entry(&job.tenant_id).await?;
            entry.state.db.save_execution(&state, &events).await?;
            self.workers
                .requeue_execution(&job.execution_id, &state_json)
                .await
                .map_err(worker_error)?;
            expired += 1;
            self.audit(
                Some(&job.tenant_id),
                None,
                "system",
                "worker.approval.timeout",
                "worker_job",
                "allow",
                None,
                json!({
                    "jobId": job.id,
                    "executionId": job.execution_id,
                    "nodeId": request.node_id,
                }),
            )
            .await;
        }
        Ok(expired)
    }

    /// Resolves tenant state, preferring PostgreSQL in runtime mode and falling
    /// back to SQLite while the mirror is incomplete.
    pub(crate) async fn tenant_is_active(&self, tenant_id: &str) -> Result<bool, ApiError> {
        if let Some(control) = self.control_store()
            && let Some(record) = control
                .tenant(tenant_id)
                .await
                .map_err(|error| ApiError::internal(error.to_string()))?
        {
            return Ok(record.status == "active");
        }
        let found: Option<String> = sqlx::query_scalar(
            "SELECT id FROM enterprise_tenants WHERE id = $1 AND status = 'active'",
        )
        .bind(tenant_id)
        .fetch_optional(&self.control)
        .await
        .map_err(internal)?;
        if found.is_none() {
            return Ok(false);
        }
        // Self-heal: mirror missing tenants so the next read hits PostgreSQL.
        let _ = self.ensure_postgres_tenant(tenant_id).await;
        Ok(true)
    }

    pub fn spawn_control_mirror(
        &self,
        interval: std::time::Duration,
    ) -> tokio::task::JoinHandle<()> {
        let state = self.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                match state.sync_postgres_control().await {
                    Ok(count) => {
                        tracing::debug!(rows = count, "control plane cache refreshed");
                    }
                    Err(error) => {
                        tracing::warn!(error = %error.message, "control plane cache refresh failed");
                    }
                }
            }
        })
    }

    pub(crate) async fn sync_postgres_control(&self) -> Result<usize, ApiError> {
        let Some(control) = self.control_store() else {
            return Ok(0);
        };
        let tenants = control.list_tenants().await.map_err(control_error)?;
        if tenants.is_empty() {
            return Ok(0);
        }
        let mut quotas = Vec::new();
        let mut members = Vec::new();
        for tenant in &tenants {
            if let Some(quota) = control.quota(&tenant.id).await.map_err(control_error)? {
                quotas.push((tenant.id.clone(), quota));
            }
            members.extend(
                control
                    .scim_list_members(&tenant.id)
                    .await
                    .map_err(control_error)?,
            );
        }
        let tenant_ids = tenants
            .iter()
            .map(|tenant| tenant.id.clone())
            .collect::<Vec<_>>();
        let quota_ids = quotas
            .iter()
            .map(|(tenant_id, _)| tenant_id.clone())
            .collect::<Vec<_>>();
        let member_ids = members
            .iter()
            .map(|member| member.id.clone())
            .collect::<Vec<_>>();
        let mut transaction = self.control.begin().await.map_err(internal)?;
        for tenant in &tenants {
            sqlx::query(
                "INSERT INTO enterprise_tenants (id, name, status, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?)
                 ON CONFLICT(id) DO UPDATE SET
                    name = excluded.name,
                    status = excluded.status,
                    updated_at = excluded.updated_at",
            )
            .bind(&tenant.id)
            .bind(&tenant.name)
            .bind(&tenant.status)
            .bind(tenant.created_at.format(&Rfc3339).unwrap_or_default())
            .bind(tenant.updated_at.format(&Rfc3339).unwrap_or_default())
            .execute(&mut *transaction)
            .await
            .map_err(internal)?;
        }
        for (tenant_id, quota) in &quotas {
            sqlx::query(
                "INSERT INTO enterprise_quotas
                 (tenant_id, max_workflows, max_executions_per_day, max_skills, max_agents,
                  max_plugins, max_connections, max_concurrent_runs)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(tenant_id) DO UPDATE SET
                    max_workflows = excluded.max_workflows,
                    max_executions_per_day = excluded.max_executions_per_day,
                    max_skills = excluded.max_skills,
                    max_agents = excluded.max_agents,
                    max_plugins = excluded.max_plugins,
                    max_connections = excluded.max_connections,
                    max_concurrent_runs = excluded.max_concurrent_runs",
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
            .await
            .map_err(internal)?;
        }
        for member in &members {
            sqlx::query(
                "INSERT INTO enterprise_members
                 (id, tenant_id, subject, user_name, external_id, display_name, email,
                  role, base_role, scim_json, status, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(id) DO UPDATE SET
                    tenant_id = excluded.tenant_id,
                    subject = excluded.subject,
                    user_name = excluded.user_name,
                    external_id = excluded.external_id,
                    display_name = excluded.display_name,
                    email = excluded.email,
                    role = excluded.role,
                    base_role = excluded.base_role,
                    scim_json = excluded.scim_json,
                    status = excluded.status,
                    updated_at = excluded.updated_at",
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
            .bind(serde_json::to_string(&member.scim_json).unwrap_or_else(|_| "{}".to_string()))
            .bind(&member.status)
            .bind(&member.created_at)
            .bind(&member.updated_at)
            .execute(&mut *transaction)
            .await
            .map_err(internal)?;
        }
        let sqlite_tenants: Vec<String> = sqlx::query_scalar("SELECT id FROM enterprise_tenants")
            .fetch_all(&mut *transaction)
            .await
            .map_err(internal)?;
        for tenant_id in sqlite_tenants {
            if tenant_id != LOCAL_TENANT && !tenant_ids.contains(&tenant_id) {
                sqlx::query("DELETE FROM enterprise_tenants WHERE id = ?")
                    .bind(&tenant_id)
                    .execute(&mut *transaction)
                    .await
                    .map_err(internal)?;
            }
        }
        let sqlite_quotas: Vec<String> =
            sqlx::query_scalar("SELECT tenant_id FROM enterprise_quotas")
                .fetch_all(&mut *transaction)
                .await
                .map_err(internal)?;
        for tenant_id in sqlite_quotas {
            if !quota_ids.contains(&tenant_id) {
                sqlx::query("DELETE FROM enterprise_quotas WHERE tenant_id = ?")
                    .bind(&tenant_id)
                    .execute(&mut *transaction)
                    .await
                    .map_err(internal)?;
            }
        }
        let sqlite_members: Vec<String> = sqlx::query_scalar("SELECT id FROM enterprise_members")
            .fetch_all(&mut *transaction)
            .await
            .map_err(internal)?;
        for member_id in sqlite_members {
            if !member_ids.contains(&member_id) {
                sqlx::query("DELETE FROM enterprise_members WHERE id = ?")
                    .bind(&member_id)
                    .execute(&mut *transaction)
                    .await
                    .map_err(internal)?;
            }
        }
        transaction.commit().await.map_err(internal)?;
        Ok(tenants.len() + quotas.len() + members.len())
    }

    pub(crate) async fn control_ping(&self) -> Result<(), ApiError> {
        sqlx::query("SELECT 1")
            .execute(&self.control)
            .await
            .map(|_| ())
            .map_err(internal)
    }

    pub(crate) async fn tenant_ping(&self, tenant_id: &str) -> Result<(), ApiError> {
        let state = self.tenant_entry(tenant_id).await?;
        sqlx::query("SELECT 1")
            .execute(&state.state.db.pool)
            .await
            .map(|_| ())
            .map_err(internal)
    }

    pub(crate) async fn tenant_active(&self, tenant_id: &str) -> Result<bool, ApiError> {
        let found: Option<String> = sqlx::query_scalar(
            "SELECT id FROM enterprise_tenants WHERE id = ? AND status = 'active'",
        )
        .bind(tenant_id)
        .fetch_optional(&self.control)
        .await
        .map_err(internal)?;
        Ok(found.is_some())
    }

    pub(crate) async fn tenant_api_state(&self, tenant_id: &str) -> Result<ApiState, ApiError> {
        Ok(self.tenant_entry(tenant_id).await?.state)
    }

    pub(crate) fn oauth(&self) -> &OauthService {
        &self.oauth
    }

    pub(crate) fn oauth_service(&self) -> OauthService {
        self.oauth.clone()
    }

    /// Resolves the resource owner for an authorization request.
    ///
    /// A browser session always wins. Without a session the local anonymous
    /// owner is used only when the runtime runs in local mode.
    pub(crate) async fn oauth_owner(
        &self,
        headers: &HeaderMap,
    ) -> Result<Option<OauthOwner>, ApiError> {
        if let Some(cookie) = headers
            .get(header::COOKIE)
            .and_then(|value| value.to_str().ok())
        {
            match self.browser_auth.session(Some(cookie)).await {
                Ok(Some(session)) => {
                    return Ok(Some(OauthOwner {
                        tenant_id: session.tenant_id,
                        subject: session.subject,
                        role: session.role,
                        platform_admin: session.platform_admin,
                        authenticated: true,
                    }));
                }
                Ok(None) => {}
                Err(browser_auth::BrowserAuthError::Forbidden(message))
                | Err(browser_auth::BrowserAuthError::Invalid(message)) => {
                    return Err(unauthorized(&message));
                }
                Err(error) => return Err(internal(error)),
            }
        }
        if self.require_auth {
            return Ok(None);
        }
        Ok(Some(OauthOwner {
            tenant_id: LOCAL_TENANT.to_string(),
            subject: "local-owner".to_string(),
            role: DEFAULT_ROLE.to_string(),
            platform_admin: true,
            authenticated: false,
        }))
    }

    pub(crate) async fn oauth_admin(&self, headers: &HeaderMap) -> Result<(), ApiError> {
        self.require_admin(headers).await.map(|_| ())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn record_audit(
        &self,
        tenant_id: Option<&str>,
        actor_id: Option<&str>,
        actor_type: &str,
        action: &str,
        resource: &str,
        decision: &str,
        reason: Option<&str>,
        detail: Value,
    ) {
        self.audit(
            tenant_id, actor_id, actor_type, action, resource, decision, reason, detail,
        )
        .await;
    }

    pub async fn create_bootstrap_token(&self, name: &str) -> Result<Value, ApiError> {
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT id FROM enterprise_tokens WHERE platform_admin = 1 AND revoked_at IS NULL LIMIT 1",
        )
        .fetch_optional(&self.control)
        .await
        .map_err(internal)?;
        if existing.is_some() {
            return Err(ApiError::conflict(
                "ADMIN_TOKEN_EXISTS",
                "a platform admin token already exists",
            ));
        }
        self.create_token(LOCAL_TENANT, name, "owner", true, None)
            .await
    }

    pub async fn create_token(
        &self,
        tenant_id: &str,
        name: &str,
        role: &str,
        platform_admin: bool,
        expires_in_days: Option<i64>,
    ) -> Result<Value, ApiError> {
        validate_tenant_id(tenant_id)?;
        validate_role(role)?;
        if let Some(control) = self.control_store() {
            let active = control
                .tenant(tenant_id)
                .await
                .map_err(control_error)?
                .is_some_and(|tenant| tenant.status == "active");
            if !active {
                return Err(ApiError::not_found(format!(
                    "tenant `{tenant_id}` not found"
                )));
            }
        } else {
            let tenant: Option<String> = sqlx::query_scalar(
                "SELECT id FROM enterprise_tenants WHERE id = ? AND status = 'active'",
            )
            .bind(tenant_id)
            .fetch_optional(&self.control)
            .await
            .map_err(internal)?;
            if tenant.is_none() {
                return Err(ApiError::not_found(format!(
                    "tenant `{tenant_id}` not found"
                )));
            }
        }
        let mut bytes = [0u8; 32];
        getrandom::getrandom(&mut bytes)
            .map_err(|error| ApiError::internal(format!("cannot generate token: {error}")))?;
        let token = format!("hpa_ent_{}", URL_SAFE_NO_PAD.encode(bytes));
        let hash = hash_token(&token);
        let now = OffsetDateTime::now_utc();
        if let Some(control) = self.control_store() {
            let expires_at =
                expires_in_days.map(|days| now + time::Duration::days(days.clamp(1, 3650)));
            let record = control
                .create_token(tenant_id, name, &hash, role, platform_admin, expires_at)
                .await
                .map_err(|error| ApiError::internal(error.to_string()))?;
            self.audit(
                Some(tenant_id),
                Some(&record.id),
                "token",
                "token.create",
                "enterprise_token",
                "allow",
                None,
                json!({"role": role, "platformAdmin": platform_admin}),
            )
            .await;
            let mut value = token_value_from_record(&record);
            if let Some(object) = value.as_object_mut() {
                object.insert("token".to_string(), Value::String(token));
            }
            return Ok(value);
        }
        let created_at = now.format(&Rfc3339).unwrap_or_default();
        let expires_at = expires_in_days.map(|days| {
            (now + time::Duration::days(days.clamp(1, 3650)))
                .format(&Rfc3339)
                .unwrap_or_default()
        });
        let id = format!("enttok_{}", Uuid::now_v7());
        sqlx::query(
            "INSERT INTO enterprise_tokens
             (id, name, token_hash, tenant_id, role, platform_admin, created_at, last_used_at, expires_at, revoked_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, NULL, ?, NULL)",
        )
        .bind(&id)
        .bind(name)
        .bind(&hash)
        .bind(tenant_id)
        .bind(role)
        .bind(platform_admin as i64)
        .bind(&created_at)
        .bind(&expires_at)
        .execute(&self.control)
        .await
        .map_err(internal)?;
        self.audit(
            Some(tenant_id),
            Some(&id),
            "token",
            "token.create",
            "enterprise_token",
            "allow",
            None,
            json!({"role": role, "platformAdmin": platform_admin}),
        )
        .await;
        Ok(json!({
            "token": token,
            "tokenInfo": {
                "id": id,
                "name": name,
                "tenantId": tenant_id,
                "role": role,
                "platformAdmin": platform_admin,
                "createdAt": created_at,
                "lastUsedAt": null,
                "expiresAt": expires_at,
                "revokedAt": null,
            }
        }))
    }

    async fn authenticate(&self, headers: &HeaderMap) -> Result<Option<Principal>, ApiError> {
        let Some(value) = headers.get(header::AUTHORIZATION) else {
            return self.authenticate_session(headers).await;
        };
        let value = value
            .to_str()
            .map_err(|_| unauthorized("invalid Authorization header"))?;
        let Some(token) = value.strip_prefix("Bearer ") else {
            return Err(unauthorized("Authorization must use Bearer"));
        };
        let token = token.trim();
        if token.is_empty() {
            return Err(unauthorized("bearer token is empty"));
        }
        if !token.starts_with("hpa_ent_") {
            if token.matches('.').count() == 2 {
                if self.oauth.is_own_access_token(token) {
                    return Err(unauthorized(
                        "OAuth access tokens are scoped to the MCP resource; use a platform token or session",
                    ));
                }
                let principal = self
                    .ops
                    .verify_oidc_token(token)
                    .await
                    .map_err(|error| unauthorized(&format!("OIDC token rejected: {error}")))?;
                let active: Option<String> = sqlx::query_scalar(
                    "SELECT id FROM enterprise_tenants WHERE id = ? AND status = 'active'",
                )
                .bind(&principal.tenant_id)
                .fetch_optional(&self.control)
                .await
                .map_err(internal)?;
                if active.is_none() {
                    return Err(unauthorized("OIDC tenant is not active"));
                }
                let identity = principal
                    .email
                    .clone()
                    .unwrap_or_else(|| principal.subject.clone());
                let member = sqlx::query(
                    "SELECT id, role, status FROM enterprise_members
                     WHERE tenant_id = ?
                       AND (subject = ? OR (email IS NOT NULL AND lower(email) = lower(?)))
                     ORDER BY CASE WHEN subject = ? THEN 0 ELSE 1 END
                     LIMIT 1",
                )
                .bind(&principal.tenant_id)
                .bind(&principal.subject)
                .bind(principal.email.as_deref().unwrap_or(""))
                .bind(&principal.subject)
                .fetch_optional(&self.control)
                .await
                .map_err(internal)?;
                if let Some(row) = member {
                    let status: String = row.get("status");
                    if status != "active" {
                        self.audit(
                            Some(&principal.tenant_id),
                            Some(&format!("oidc:{identity}")),
                            "oidc",
                            "member.authenticate",
                            "enterprise_member",
                            "deny",
                            Some("member is disabled"),
                            json!({}),
                        )
                        .await;
                        return Err(unauthorized("OIDC member is disabled"));
                    }
                    let member_id: String = row.get("id");
                    return Ok(Some(Principal {
                        token_id: format!("member:{member_id}"),
                        tenant_id: principal.tenant_id,
                        role: row.get("role"),
                        platform_admin: false,
                    }));
                }
                return Ok(Some(Principal {
                    token_id: format!("oidc:{identity}"),
                    tenant_id: principal.tenant_id,
                    role: principal.role,
                    platform_admin: false,
                }));
            }
            return Err(unauthorized("unsupported bearer token"));
        }
        let hash = hash_token(token);
        if let Some(control) = self.control_store() {
            let record = control
                .find_token_by_hash(&hash)
                .await
                .map_err(|error| ApiError::internal(error.to_string()))?
                .ok_or_else(|| unauthorized("token is not recognized"))?;
            if record.is_revoked() {
                return Err(unauthorized("token is revoked"));
            }
            let now = OffsetDateTime::now_utc();
            if record.is_expired(now) {
                return Err(unauthorized("token is expired"));
            }
            let _ = control.touch_token(&record.id, now).await;
            return Ok(Some(Principal {
                token_id: record.id,
                tenant_id: record.tenant_id,
                role: record.role,
                platform_admin: record.platform_admin,
            }));
        }
        let row = sqlx::query(
            "SELECT id, tenant_id, role, platform_admin, expires_at, revoked_at
             FROM enterprise_tokens WHERE token_hash = ?",
        )
        .bind(&hash)
        .fetch_optional(&self.control)
        .await
        .map_err(internal)?;
        let Some(row) = row else {
            return Err(unauthorized("token is not recognized"));
        };
        let revoked: Option<String> = row.get("revoked_at");
        if revoked.is_some() {
            return Err(unauthorized("token is revoked"));
        }
        let expires_at: Option<String> = row.get("expires_at");
        if expires_at
            .as_deref()
            .is_some_and(|expires| expires <= now_rfc3339().as_str())
        {
            return Err(unauthorized("token is expired"));
        }
        let tenant_id: String = row.get("tenant_id");
        let platform_admin = row.get::<i64, _>("platform_admin") != 0;
        if !platform_admin {
            let active: Option<String> = sqlx::query_scalar(
                "SELECT id FROM enterprise_tenants WHERE id = ? AND status = 'active'",
            )
            .bind(&tenant_id)
            .fetch_optional(&self.control)
            .await
            .map_err(internal)?;
            if active.is_none() {
                return Err(unauthorized("tenant is not active"));
            }
        }
        let token_id: String = row.get("id");
        let _ = sqlx::query("UPDATE enterprise_tokens SET last_used_at = ? WHERE id = ?")
            .bind(now_rfc3339())
            .bind(&token_id)
            .execute(&self.control)
            .await;
        Ok(Some(Principal {
            token_id,
            tenant_id,
            role: row.get("role"),
            platform_admin,
        }))
    }

    async fn authenticate_session(
        &self,
        headers: &HeaderMap,
    ) -> Result<Option<Principal>, ApiError> {
        let Some(cookie) = headers
            .get(header::COOKIE)
            .and_then(|value| value.to_str().ok())
        else {
            return Ok(None);
        };
        match self.browser_auth.session(Some(cookie)).await {
            Ok(Some(principal)) => Ok(Some(Principal {
                token_id: format!("session:{}", principal.session_id),
                tenant_id: principal.tenant_id,
                role: principal.role,
                platform_admin: principal.platform_admin,
            })),
            Ok(None) => Ok(None),
            Err(browser_auth::BrowserAuthError::Forbidden(message))
            | Err(browser_auth::BrowserAuthError::Invalid(message)) => Err(unauthorized(&message)),
            Err(error) => Err(internal(error)),
        }
    }

    async fn require_admin(&self, headers: &HeaderMap) -> Result<Principal, ApiError> {
        let principal = match self.authenticate(headers).await? {
            Some(principal) => principal,
            None if !self.require_auth => Principal {
                token_id: "local".to_string(),
                tenant_id: LOCAL_TENANT.to_string(),
                role: DEFAULT_ROLE.to_string(),
                platform_admin: true,
            },
            None => return Err(unauthorized("platform admin token required")),
        };
        if !principal.platform_admin {
            self.audit(
                Some(&principal.tenant_id),
                Some(&principal.token_id),
                "token",
                "enterprise.admin",
                "control_plane",
                "deny",
                Some("platform admin required"),
                json!({}),
            )
            .await;
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "PLATFORM_ADMIN_REQUIRED",
                "platform admin token required",
            ));
        }
        Ok(principal)
    }

    async fn tenant_entry(&self, tenant_id: &str) -> Result<TenantEntry, ApiError> {
        if let Some(entry) = self.cache.lock().await.get(tenant_id).cloned() {
            return Ok(entry);
        }
        let entry = self.build_tenant(tenant_id).await?;
        self.cache
            .lock()
            .await
            .insert(tenant_id.to_string(), entry.clone());
        Ok(entry)
    }

    async fn build_tenant(&self, tenant_id: &str) -> Result<TenantEntry, ApiError> {
        self.ensure_postgres_tenant(tenant_id).await?;
        let tenant_dir = self.tenant_dir(tenant_id);
        tokio::fs::create_dir_all(&tenant_dir)
            .await
            .map_err(|error| ApiError::internal(error.to_string()))?;
        let mut db = Db::open(&tenant_dir.join("hpa.db"))
            .await
            .map_err(|error| ApiError::internal(error.to_string()))?;
        if let Some(pool) = self.postgres_execution.as_ref() {
            db = db.with_postgres_execution(pool.clone(), tenant_id);
        }
        if let Some(replica) = self.postgres_replica.as_ref() {
            db = db.with_postgres_replica(replica.clone(), tenant_id);
        }
        let event_bus = self
            .postgres_event_bus
            .as_ref()
            .map(|pool| crate::event_bus::EventBusPublisher::new(pool.clone(), tenant_id));
        let runtime = RuntimeManager::new(db.clone());
        runtime
            .recover()
            .await
            .map_err(|error| ApiError::internal(error.to_string()))?;
        runtime.start_approval_monitor();
        runtime.start_recovery_monitor();
        let skills = SkillService::new(db.clone(), tenant_dir.clone());
        let connectors = ConnectorRuntime::new(db.clone());
        connectors
            .initialize()
            .await
            .map_err(|error| ApiError::internal(error.to_string()))?;
        let plugins = PluginService::new(
            db.clone(),
            tenant_dir.clone(),
            skills.clone(),
            connectors.clone(),
        );
        let mut agents = AgentRuntime::new(skills, runtime.clone(), connectors.clone())
            .with_tenant_id(tenant_id);
        if let Some(config) = crate::agent::PythonWorkerConfig::from_environment(
            &self.mcp_base_url,
            tenant_dir.join("agent-checkpoints"),
        ) {
            agents = agents.with_python_worker(config);
        }
        agents
            .recover()
            .await
            .map_err(|error| ApiError::internal(error.to_string()))?;
        agents.start_approval_monitor();
        let state = ApiState {
            tenant_id: tenant_id.to_string(),
            db,
            runtime,
            agents,
            connectors,
            plugins,
            data_dir: tenant_dir,
            mcp_base_url: self.mcp_base_url.clone(),
            worker_queue: Some(self.workers.clone()),
            event_bus,
        };
        let router = crate::api::router(state.clone());
        Ok(TenantEntry { router, state })
    }

    fn tenant_dir(&self, tenant_id: &str) -> PathBuf {
        if tenant_id == LOCAL_TENANT {
            self.data_dir.clone()
        } else {
            self.data_dir.join("tenants").join(tenant_id)
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn audit(
        &self,
        tenant_id: Option<&str>,
        actor_id: Option<&str>,
        actor_type: &str,
        action: &str,
        resource: &str,
        decision: &str,
        reason: Option<&str>,
        detail: Value,
    ) {
        if let Some(control) = self.control_store() {
            let entry = AuditEntry {
                tenant_id,
                actor_id,
                actor_type,
                action,
                resource,
                decision,
                reason,
                detail: &detail,
            };
            if let Err(error) = control.append_audit(entry).await {
                tracing::warn!(error = %error, "cannot persist audit event in PostgreSQL");
            }
            return;
        }
        let _ = sqlx::query(
            "INSERT INTO enterprise_audit
             (id, at, tenant_id, actor_id, actor_type, action, resource, decision, reason, detail_json)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(format!("entaudit_{}", Uuid::now_v7()))
        .bind(now_rfc3339())
        .bind(tenant_id)
        .bind(actor_id)
        .bind(actor_type)
        .bind(action)
        .bind(resource)
        .bind(decision)
        .bind(reason)
        .bind(serde_json::to_string(&detail).unwrap_or_else(|_| "{}".to_string()))
        .execute(&self.control)
        .await;
    }

    async fn quota(&self, tenant_id: &str) -> Result<Quota, ApiError> {
        if let Some(control) = self.control_store()
            && let Some(record) = control
                .quota(tenant_id)
                .await
                .map_err(|error| ApiError::internal(error.to_string()))?
        {
            return Ok(Quota {
                max_workflows: record.max_workflows,
                max_executions_per_day: record.max_executions_per_day,
                max_skills: record.max_skills,
                max_agents: record.max_agents,
                max_plugins: record.max_plugins,
                max_connections: record.max_connections,
                max_concurrent_runs: record.max_concurrent_runs,
            });
        }
        let row = sqlx::query(
            "SELECT max_workflows, max_executions_per_day, max_skills, max_agents,
                    max_plugins, max_connections, max_concurrent_runs
             FROM enterprise_quotas WHERE tenant_id = ?",
        )
        .bind(tenant_id)
        .fetch_optional(&self.control)
        .await
        .map_err(internal)?
        .ok_or_else(|| ApiError::not_found("tenant quotas not found"))?;
        Ok(Quota {
            max_workflows: row.get("max_workflows"),
            max_executions_per_day: row.get("max_executions_per_day"),
            max_skills: row.get("max_skills"),
            max_agents: row.get("max_agents"),
            max_plugins: row.get("max_plugins"),
            max_connections: row.get("max_connections"),
            max_concurrent_runs: row.get("max_concurrent_runs"),
        })
    }

    async fn check_quota(
        &self,
        entry: &TenantEntry,
        tenant_id: &str,
        method: &str,
        path: &str,
        idempotency_key: Option<&str>,
    ) -> Result<(), ApiError> {
        if !matches!(method, "POST" | "PUT") {
            return Ok(());
        }
        let quota = self.quota(tenant_id).await?;
        let db = &entry.state.db;
        if is_execution_create_request(method, path)
            && let Some(idempotency_key) = idempotency_key
            && db.execution_idempotency_exists(idempotency_key).await?
        {
            return Ok(());
        }
        let (label, count, limit) = if method == "POST" && path == "/workflows" {
            ("workflows", db.workflow_count().await?, quota.max_workflows)
        } else if method == "POST" && path == "/skills/import" {
            ("skills", db.skill_count().await?, quota.max_skills)
        } else if method == "POST" && path == "/agents" {
            ("agents", db.agent_count().await?, quota.max_agents)
        } else if is_plugin_create_request(method, path) {
            ("plugins", db.plugin_count().await?, quota.max_plugins)
        } else if is_connection_create_request(method, path) {
            (
                "connections",
                db.connector_connection_count().await?,
                quota.max_connections,
            )
        } else if is_execution_create_request(method, path) {
            (
                "executionsToday",
                db.execution_count_since(&today_prefix()).await?,
                quota.max_executions_per_day,
            )
        } else if method == "POST" && path.starts_with("/agents/") && path.ends_with("/run") {
            (
                "agentSessionsToday",
                db.agent_sessions_on(&today_prefix()).await?,
                quota.max_executions_per_day,
            )
        } else {
            return Ok(());
        };
        if count >= limit {
            self.audit(
                Some(tenant_id),
                None,
                "api",
                "quota.check",
                label,
                "deny",
                Some("tenant quota exceeded"),
                json!({"count": count, "limit": limit}),
            )
            .await;
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "TENANT_QUOTA",
                format!("tenant quota `{label}` exceeded ({count}/{limit})"),
            ));
        }
        if is_execution_create_request(method, path) {
            let running = db.active_execution_count().await?;
            if running >= quota.max_concurrent_runs {
                return Err(ApiError::new(
                    StatusCode::TOO_MANY_REQUESTS,
                    "TENANT_CONCURRENCY",
                    format!(
                        "tenant concurrent runs exceeded ({running}/{})",
                        quota.max_concurrent_runs
                    ),
                ));
            }
        }
        Ok(())
    }
}

pub fn spawn_remote_approval_monitor(state: EnterpriseState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let interval = std::env::var("HPA_APPROVAL_TICK_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(1_000);
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(interval)).await;
            if let Err(error) = state.expire_worker_approvals().await {
                tracing::warn!(error = %error.message, "remote approval SLA scan failed");
            }
        }
    })
}

pub fn router(state: EnterpriseState) -> Router {
    Router::new()
        .route(
            "/api/enterprise/tenants",
            get(list_tenants).post(create_tenant),
        )
        .route(
            "/api/enterprise/tenants/{id}",
            get(get_tenant)
                .put(update_tenant_status)
                .delete(delete_tenant),
        )
        .route(
            "/api/enterprise/tokens",
            get(list_tokens).post(create_token_handler),
        )
        .route("/api/enterprise/tokens/{id}", delete(revoke_token))
        .route(
            "/api/enterprise/members",
            get(list_members).post(create_member),
        )
        .route(
            "/api/enterprise/members/{id}",
            put(update_member).delete(delete_member),
        )
        .route(
            "/api/enterprise/quotas/{tenant_id}",
            get(get_quota_handler).put(put_quota_handler),
        )
        .route("/api/enterprise/audit", get(list_audit))
        .route("/api/enterprise/audit/export", get(export_audit))
        .route(
            "/api/enterprise/workers",
            get(list_workers).post(register_worker),
        )
        .route(
            "/api/enterprise/workers/{id}/heartbeat",
            post(worker_heartbeat),
        )
        .route(
            "/api/enterprise/workers/{id}/token",
            post(rotate_worker_token),
        )
        .route("/api/enterprise/workers/{id}/disable", post(disable_worker))
        .route(
            "/api/enterprise/worker-jobs",
            get(list_worker_jobs).post(create_worker_job),
        )
        .route(
            "/api/enterprise/worker-jobs/{id}/cancel",
            post(cancel_worker_job),
        )
        .route(
            "/api/enterprise/tenants/{id}/workflows",
            get(list_tenant_workflows),
        )
        .route("/api/worker/heartbeat", post(worker_agent_heartbeat))
        .route("/api/worker/lease", post(worker_agent_lease))
        .route("/api/worker/jobs/{id}/start", post(worker_agent_start))
        .route("/api/worker/jobs/{id}/events", post(worker_agent_events))
        .route(
            "/api/worker/jobs/{id}/complete",
            post(worker_agent_complete),
        )
        .route("/api/enterprise/backups", get(list_backups))
        .route("/api/enterprise/backups/purge", post(purge_backups))
        .route("/api/enterprise/backups/{id}", delete(delete_backup))
        .route("/api/enterprise/backups/{id}/restore", post(restore_backup))
        .route("/api/enterprise/tenants/{id}/backups", post(create_backup))
        .route("/api/enterprise/oidc", get(get_oidc).put(set_oidc))
        .route("/auth/oidc/login", get(oidc_login))
        .route("/auth/oidc/callback", get(oidc_callback))
        .route("/auth/oidc/logout/callback", get(oidc_logout_callback))
        .route("/auth/session", get(auth_session))
        .route("/auth/refresh", post(auth_refresh))
        .route("/auth/logout", post(auth_logout))
        .route("/api/auth/session", get(auth_session))
        .route("/api/auth/refresh", post(auth_refresh))
        .route("/api/auth/logout", post(auth_logout))
        .merge(scim::router())
        .merge(crate::oauth_http::router())
        .merge(crate::hooks::router())
        .merge(crate::observe::router())
        .route("/api/{*rest}", any(tenant_gate))
        .route_layer(middleware::from_fn(crate::metrics::track_http))
        .layer(middleware::from_fn_with_state(state.clone(), csrf_guard))
        .with_state(state)
}

async fn list_tenants(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    state.require_admin(&headers).await?;
    if let Some(control) = state.control_store() {
        let tenants = control.list_tenants().await.map_err(control_error)?;
        return Ok(Json(json!({
            "tenants": tenants
                .iter()
                .filter(|tenant| tenant.status != "deleted")
                .map(tenant_record_value)
                .collect::<Vec<_>>(),
        })));
    }
    let rows = sqlx::query(
        "SELECT id, name, status, created_at, updated_at FROM enterprise_tenants
         WHERE status != 'deleted' ORDER BY created_at ASC",
    )
    .fetch_all(&state.control)
    .await
    .map_err(internal)?;
    Ok(Json(json!({
        "tenants": rows.iter().map(tenant_value).collect::<Vec<_>>(),
    })))
}

#[derive(Debug, Deserialize)]
struct CreateTenantRequest {
    name: String,
}

async fn create_tenant(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
    Json(request): Json<CreateTenantRequest>,
) -> Result<Json<Value>, ApiError> {
    let admin = state.require_admin(&headers).await?;
    if request.name.trim().is_empty() {
        return Err(ApiError::bad_request(
            "TENANT_NAME_REQUIRED",
            "tenant name is required",
        ));
    }
    let id = format!("ten_{}", Uuid::now_v7().simple());
    let now = now_rfc3339();
    if let Some(control) = state.control_store() {
        let tenant = TenantRecord {
            id: id.clone(),
            name: request.name.trim().to_string(),
            status: "active".to_string(),
            created_at: parse_rfc3339(&now)?,
            updated_at: parse_rfc3339(&now)?,
        };
        control
            .upsert_tenant(&tenant)
            .await
            .map_err(control_error)?;
        control
            .upsert_quota(&id, &default_quota())
            .await
            .map_err(control_error)?;
        tokio::fs::create_dir_all(state.tenant_dir(&id))
            .await
            .map_err(|error| ApiError::internal(error.to_string()))?;
        state
            .audit(
                Some(&id),
                Some(&admin.token_id),
                "token",
                "tenant.create",
                "tenant",
                "allow",
                None,
                json!({"name": request.name}),
            )
            .await;
        return Ok(Json(tenant_record_value(&tenant)));
    }
    sqlx::query(
        "INSERT INTO enterprise_tenants (id, name, status, created_at, updated_at)
         VALUES (?, ?, 'active', ?, ?)",
    )
    .bind(&id)
    .bind(request.name.trim())
    .bind(&now)
    .bind(&now)
    .execute(&state.control)
    .await
    .map_err(internal)?;
    sqlx::query(
        "INSERT INTO enterprise_quotas
         (tenant_id, max_workflows, max_executions_per_day, max_skills, max_agents,
          max_plugins, max_connections, max_concurrent_runs)
         VALUES (?, 100, 1000, 100, 100, 50, 100, 8)",
    )
    .bind(&id)
    .execute(&state.control)
    .await
    .map_err(internal)?;
    tokio::fs::create_dir_all(state.tenant_dir(&id))
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?;
    // PostgreSQL-mode control rows are provisioned with the tenant (and its
    // default quota) so worker registration and FK-bound writes do not fail.
    state.ensure_postgres_tenant(&id).await?;
    if let Some(control) = state.control_store() {
        let quota = QuotaRecord {
            max_workflows: 100,
            max_executions_per_day: 1000,
            max_skills: 100,
            max_agents: 100,
            max_plugins: 50,
            max_connections: 100,
            max_concurrent_runs: 8,
        };
        control
            .upsert_quota(&id, &quota)
            .await
            .map_err(|error| ApiError::internal(error.to_string()))?;
    }
    state
        .audit(
            Some(&id),
            Some(&admin.token_id),
            "token",
            "tenant.create",
            "tenant",
            "allow",
            None,
            json!({"name": request.name}),
        )
        .await;
    let row = sqlx::query(
        "SELECT id, name, status, created_at, updated_at FROM enterprise_tenants WHERE id = ?",
    )
    .bind(&id)
    .fetch_one(&state.control)
    .await
    .map_err(internal)?;
    Ok(Json(tenant_value(&row)))
}

async fn get_tenant(
    State(state): State<EnterpriseState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    state.require_admin(&headers).await?;
    if let Some(control) = state.control_store() {
        let tenant = control
            .tenant(&id)
            .await
            .map_err(control_error)?
            .filter(|tenant| tenant.status != "deleted")
            .ok_or_else(|| ApiError::not_found(format!("tenant `{id}` not found")))?;
        let quota = state.quota(&id).await?;
        let workers = state
            .ops
            .list_workers(Some(&id))
            .await
            .map_err(ops_error)?
            .into_iter()
            .map(normalize_worker)
            .collect::<Vec<_>>();
        let backups = state
            .ops
            .list_backups(Some(&id))
            .await
            .map_err(ops_error)?
            .into_iter()
            .map(normalize_backup)
            .collect::<Vec<_>>();
        return Ok(Json(json!({
            "tenant": tenant_record_value(&tenant),
            "quotas": quota_value(&id, &quota),
            "workers": workers,
            "backups": backups,
        })));
    }
    let row = sqlx::query(
        "SELECT id, name, status, created_at, updated_at FROM enterprise_tenants WHERE id = ?",
    )
    .bind(&id)
    .fetch_optional(&state.control)
    .await
    .map_err(internal)?
    .ok_or_else(|| ApiError::not_found(format!("tenant `{id}` not found")))?;
    let quota = state.quota(&id).await?;
    let workers = state
        .ops
        .list_workers(Some(&id))
        .await
        .map_err(ops_error)?
        .into_iter()
        .map(normalize_worker)
        .collect::<Vec<_>>();
    let backups = state
        .ops
        .list_backups(Some(&id))
        .await
        .map_err(ops_error)?
        .into_iter()
        .map(normalize_backup)
        .collect::<Vec<_>>();
    Ok(Json(json!({
        "tenant": tenant_value(&row),
        "quotas": quota_value(&id, &quota),
        "workers": workers,
        "backups": backups,
    })))
}

#[derive(Debug, Deserialize, Default)]
struct DeleteTenantRequest {
    #[serde(default)]
    confirm: bool,
}

#[derive(Debug, Deserialize)]
struct UpdateTenantStatusRequest {
    status: String,
}

async fn update_tenant_status(
    State(state): State<EnterpriseState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
    Json(request): Json<UpdateTenantStatusRequest>,
) -> Result<Json<Value>, ApiError> {
    let admin = state.require_admin(&headers).await?;
    if id == LOCAL_TENANT {
        return Err(ApiError::conflict(
            "TENANT_PROTECTED",
            "the local tenant status cannot be changed",
        ));
    }
    if !matches!(request.status.as_str(), "active" | "suspended") {
        return Err(ApiError::bad_request(
            "TENANT_STATUS_INVALID",
            "status must be active or suspended",
        ));
    }
    let now = now_rfc3339();
    if let Some(control) = state.control_store() {
        let tenant = control
            .tenant(&id)
            .await
            .map_err(control_error)?
            .filter(|tenant| tenant.status != "deleted")
            .ok_or_else(|| ApiError::not_found(format!("tenant `{id}` not found")))?;
        let updated = TenantRecord {
            status: request.status.clone(),
            updated_at: parse_rfc3339(&now)?,
            ..tenant
        };
        control
            .upsert_tenant(&updated)
            .await
            .map_err(control_error)?;
        if request.status == "suspended" {
            let _ = state.browser_auth.revoke_tenant_sessions(&id).await;
        }
        state
            .audit(
                Some(&id),
                Some(&admin.token_id),
                "token",
                "tenant.status",
                "tenant",
                "allow",
                None,
                json!({"status": request.status}),
            )
            .await;
        return Ok(Json(tenant_record_value(&updated)));
    }
    let updated = sqlx::query(
        "UPDATE enterprise_tenants SET status = ?, updated_at = ?
         WHERE id = ? AND status != 'deleted'",
    )
    .bind(&request.status)
    .bind(&now)
    .bind(&id)
    .execute(&state.control)
    .await
    .map_err(internal)?;
    if updated.rows_affected() == 0 {
        return Err(ApiError::not_found(format!("tenant `{id}` not found")));
    }
    if request.status == "suspended" {
        let _ = state.browser_auth.revoke_tenant_sessions(&id).await;
    }
    // Keep the PostgreSQL mirror in step with suspend/resume.
    let _ = state.ensure_postgres_tenant(&id).await;
    state
        .audit(
            Some(&id),
            Some(&admin.token_id),
            "token",
            "tenant.status",
            "tenant",
            "allow",
            None,
            json!({"status": request.status}),
        )
        .await;
    let row = sqlx::query(
        "SELECT id, name, status, created_at, updated_at FROM enterprise_tenants WHERE id = ?",
    )
    .bind(&id)
    .fetch_one(&state.control)
    .await
    .map_err(internal)?;
    Ok(Json(tenant_value(&row)))
}

async fn delete_tenant(
    State(state): State<EnterpriseState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
    Json(request): Json<DeleteTenantRequest>,
) -> Result<Json<Value>, ApiError> {
    let admin = state.require_admin(&headers).await?;
    if id == LOCAL_TENANT {
        return Err(ApiError::conflict(
            "TENANT_PROTECTED",
            "the local tenant cannot be deleted",
        ));
    }
    if !request.confirm {
        return Err(ApiError::bad_request(
            "CONFIRM_REQUIRED",
            "pass confirm=true to delete a tenant",
        ));
    }
    let now = now_rfc3339();
    if let Some(control) = state.control_store() {
        let tenant = control
            .tenant(&id)
            .await
            .map_err(control_error)?
            .filter(|tenant| tenant.status != "deleted")
            .ok_or_else(|| ApiError::not_found(format!("tenant `{id}` not found")))?;
        let updated = TenantRecord {
            status: "deleted".to_string(),
            updated_at: parse_rfc3339(&now)?,
            ..tenant
        };
        control
            .upsert_tenant(&updated)
            .await
            .map_err(control_error)?;
        let _ = control.revoke_tenant_tokens(&id).await;
        let _ = state.browser_auth.revoke_tenant_sessions(&id).await;
        let dir = state.tenant_dir(&id);
        if dir.exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        state.cache.lock().await.remove(&id);
        state
            .audit(
                Some(&id),
                Some(&admin.token_id),
                "token",
                "tenant.delete",
                "tenant",
                "allow",
                None,
                json!({}),
            )
            .await;
        return Ok(Json(json!({"deleted": true})));
    }
    let updated = sqlx::query(
        "UPDATE enterprise_tenants SET status = 'deleted', updated_at = ? WHERE id = ?",
    )
    .bind(&now)
    .bind(&id)
    .execute(&state.control)
    .await
    .map_err(internal)?;
    if updated.rows_affected() == 0 {
        return Err(ApiError::not_found(format!("tenant `{id}` not found")));
    }
    sqlx::query(
        "UPDATE enterprise_tokens SET revoked_at = ? WHERE tenant_id = ? AND revoked_at IS NULL",
    )
    .bind(&now)
    .bind(&id)
    .execute(&state.control)
    .await
    .map_err(internal)?;
    sqlx::query("DELETE FROM enterprise_scim_groups WHERE tenant_id = ?")
        .bind(&id)
        .execute(&state.control)
        .await
        .map_err(internal)?;
    sqlx::query("DELETE FROM enterprise_members WHERE tenant_id = ?")
        .bind(&id)
        .execute(&state.control)
        .await
        .map_err(internal)?;
    let backups = state.ops.list_backups(Some(&id)).await.map_err(ops_error)?;
    for backup in backups {
        if let Some(backup_id) = backup.get("id").and_then(Value::as_str) {
            let _ = state.ops.delete_backup(backup_id).await;
        }
    }
    let workers = state.ops.list_workers(Some(&id)).await.map_err(ops_error)?;
    for worker in workers {
        if let Some(worker_id) = worker.get("id").and_then(Value::as_str) {
            let _ = state.workers.revoke_worker_tokens(worker_id).await;
        }
    }
    if let Ok(jobs) = state.workers.list_jobs(Some(&id), 200).await {
        for job in jobs {
            if !matches!(job.status.as_str(), "succeeded" | "failed" | "cancelled") {
                let _ = state.workers.cancel_job(&id, &job.id).await;
            }
        }
    }
    sqlx::query("DELETE FROM enterprise_workers WHERE tenant_id = ?")
        .bind(&id)
        .execute(&state.control)
        .await
        .map_err(internal)?;
    sqlx::query("DELETE FROM enterprise_quotas WHERE tenant_id = ?")
        .bind(&id)
        .execute(&state.control)
        .await
        .map_err(internal)?;
    let dir = state.tenant_dir(&id);
    if dir.exists() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    state.cache.lock().await.remove(&id);
    state
        .audit(
            Some(&id),
            Some(&admin.token_id),
            "token",
            "tenant.delete",
            "tenant",
            "allow",
            None,
            json!({}),
        )
        .await;
    Ok(Json(json!({"deleted": true})))
}

async fn list_tokens(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
    Query(query): Query<TenantQuery>,
) -> Result<Json<Value>, ApiError> {
    state.require_admin(&headers).await?;
    if let Some(control) = state.control_store() {
        let records = control
            .list_tokens(query.tenant_id.as_deref())
            .await
            .map_err(|error| ApiError::internal(error.to_string()))?;
        return Ok(Json(json!({
            "tokens": records.iter().map(token_value_from_record).collect::<Vec<_>>(),
        })));
    }
    let rows = match query.tenant_id.as_deref() {
        Some(tenant_id) => {
            sqlx::query(
                "SELECT id, name, tenant_id, role, platform_admin, created_at, last_used_at,
                        expires_at, revoked_at FROM enterprise_tokens WHERE tenant_id = ?
                 ORDER BY created_at DESC",
            )
            .bind(tenant_id)
            .fetch_all(&state.control)
            .await
        }
        None => {
            sqlx::query(
                "SELECT id, name, tenant_id, role, platform_admin, created_at, last_used_at,
                        expires_at, revoked_at FROM enterprise_tokens
                 ORDER BY created_at DESC",
            )
            .fetch_all(&state.control)
            .await
        }
    }
    .map_err(internal)?;
    Ok(Json(json!({
        "tokens": rows.iter().map(token_value).collect::<Vec<_>>(),
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateTokenRequest {
    tenant_id: String,
    name: String,
    role: String,
    #[serde(default)]
    platform_admin: bool,
    expires_in_days: Option<i64>,
}

async fn create_token_handler(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
    Json(request): Json<CreateTokenRequest>,
) -> Result<Json<Value>, ApiError> {
    state.require_admin(&headers).await?;
    if request.name.trim().is_empty() {
        return Err(ApiError::bad_request(
            "TOKEN_NAME_REQUIRED",
            "token name is required",
        ));
    }
    Ok(Json(
        state
            .create_token(
                &request.tenant_id,
                request.name.trim(),
                &request.role,
                request.platform_admin,
                request.expires_in_days,
            )
            .await?,
    ))
}

async fn revoke_token(
    State(state): State<EnterpriseState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let admin = state.require_admin(&headers).await?;
    let now = now_rfc3339();
    let revoked = if let Some(control) = state.control_store() {
        control
            .revoke_token(None, &id, OffsetDateTime::now_utc())
            .await
            .map_err(|error| ApiError::internal(error.to_string()))?
    } else {
        sqlx::query(
            "UPDATE enterprise_tokens SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL",
        )
        .bind(&now)
        .bind(&id)
        .execute(&state.control)
        .await
        .map_err(internal)?
        .rows_affected()
            > 0
    };
    state
        .audit(
            None,
            Some(&admin.token_id),
            "token",
            "token.revoke",
            "enterprise_token",
            "allow",
            None,
            json!({"tokenId": id}),
        )
        .await;
    Ok(Json(json!({"revoked": revoked})))
}

async fn list_members(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
    Query(query): Query<TenantQuery>,
) -> Result<Json<Value>, ApiError> {
    state.require_admin(&headers).await?;
    if let Some(control) = state.control_store() {
        let mut members = control
            .list_members(query.tenant_id.as_deref())
            .await
            .map_err(control_error)?;
        members.reverse();
        return Ok(Json(json!({
            "members": members.iter().map(member_record_value).collect::<Vec<_>>(),
        })));
    }
    let rows = match query.tenant_id.as_deref() {
        Some(tenant_id) => {
            sqlx::query(
                "SELECT id, tenant_id, subject, display_name, email, role, status,
                        created_at, updated_at FROM enterprise_members WHERE tenant_id = ?
                 ORDER BY created_at DESC",
            )
            .bind(tenant_id)
            .fetch_all(&state.control)
            .await
        }
        None => {
            sqlx::query(
                "SELECT id, tenant_id, subject, display_name, email, role, status,
                        created_at, updated_at FROM enterprise_members
                 ORDER BY created_at DESC",
            )
            .fetch_all(&state.control)
            .await
        }
    }
    .map_err(internal)?;
    Ok(Json(json!({
        "members": rows.iter().map(member_value).collect::<Vec<_>>(),
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateMemberRequest {
    tenant_id: String,
    subject: String,
    display_name: Option<String>,
    email: Option<String>,
    role: String,
}

async fn create_member(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
    Json(request): Json<CreateMemberRequest>,
) -> Result<Json<Value>, ApiError> {
    let admin = state.require_admin(&headers).await?;
    validate_tenant_id(&request.tenant_id)?;
    validate_role(&request.role)?;
    if state.control_store().is_none() {
        let tenant: Option<String> = sqlx::query_scalar(
            "SELECT id FROM enterprise_tenants WHERE id = ? AND status = 'active'",
        )
        .bind(&request.tenant_id)
        .fetch_optional(&state.control)
        .await
        .map_err(internal)?;
        if tenant.is_none() {
            return Err(ApiError::not_found(format!(
                "tenant `{}` not found",
                request.tenant_id
            )));
        }
    }
    let subject = request.subject.trim();
    if subject.is_empty() || subject.len() > 200 {
        return Err(ApiError::bad_request(
            "MEMBER_SUBJECT_INVALID",
            "member subject must be 1-200 characters",
        ));
    }
    let display_name = request
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(subject);
    if display_name.len() > 200 {
        return Err(ApiError::bad_request(
            "MEMBER_NAME_INVALID",
            "member display name must be at most 200 characters",
        ));
    }
    let email = normalize_member_email(request.email.as_deref())?;
    let user_name = email.clone().unwrap_or_else(|| subject.to_string());
    let now = now_rfc3339();
    let id = format!("entmem_{}", Uuid::now_v7());
    if let Some(control) = state.control_store() {
        let tenant = control
            .tenant(&request.tenant_id)
            .await
            .map_err(control_error)?
            .filter(|tenant| tenant.status == "active")
            .ok_or_else(|| {
                ApiError::not_found(format!("tenant `{}` not found", request.tenant_id))
            })?;
        let member = ScimMemberRecord {
            id: id.clone(),
            tenant_id: tenant.id,
            subject: subject.to_string(),
            user_name: Some(user_name),
            external_id: None,
            display_name: display_name.to_string(),
            email,
            role: request.role.clone(),
            base_role: request.role.clone(),
            scim_json: json!({}),
            status: "active".to_string(),
            created_at: now.clone(),
            updated_at: now.clone(),
        };
        control
            .scim_create_member(&member)
            .await
            .map_err(control_error)?;
        state
            .audit(
                Some(&request.tenant_id),
                Some(&admin.token_id),
                "token",
                "member.create",
                "enterprise_member",
                "allow",
                None,
                json!({"memberId": id, "role": request.role}),
            )
            .await;
        let stored = control
            .scim_member_by_id(&request.tenant_id, &id)
            .await
            .map_err(control_error)?
            .ok_or_else(|| ApiError::internal("member was not persisted"))?;
        return Ok(Json(scim_member_value(&stored)));
    }
    let inserted = sqlx::query(
        "INSERT INTO enterprise_members
         (id, tenant_id, subject, user_name, display_name, email, role, base_role,
          scim_json, status, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, '{}', 'active', ?, ?)
         ON CONFLICT(tenant_id, subject) DO NOTHING",
    )
    .bind(&id)
    .bind(&request.tenant_id)
    .bind(subject)
    .bind(&user_name)
    .bind(display_name)
    .bind(&email)
    .bind(&request.role)
    .bind(&request.role)
    .bind(&now)
    .bind(&now)
    .execute(&state.control)
    .await
    .map_err(|error| {
        if error
            .as_database_error()
            .is_some_and(|database| database.is_unique_violation())
        {
            ApiError::conflict(
                "MEMBER_IDENTITY_EXISTS",
                "member subject, userName or externalId already exists in this tenant",
            )
        } else {
            internal(error)
        }
    })?;
    if inserted.rows_affected() == 0 {
        return Err(ApiError::conflict(
            "MEMBER_EXISTS",
            "member subject already exists in this tenant",
        ));
    }
    state
        .audit(
            Some(&request.tenant_id),
            Some(&admin.token_id),
            "token",
            "member.create",
            "enterprise_member",
            "allow",
            None,
            json!({"memberId": id, "role": request.role}),
        )
        .await;
    let row = sqlx::query(
        "SELECT id, tenant_id, subject, display_name, email, role, status,
                created_at, updated_at FROM enterprise_members WHERE id = ?",
    )
    .bind(&id)
    .fetch_one(&state.control)
    .await
    .map_err(internal)?;
    mirror_member_row(&state, &row).await;
    Ok(Json(member_value(&row)))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateMemberRequest {
    display_name: Option<String>,
    email: Option<String>,
    role: Option<String>,
    status: Option<String>,
}

async fn update_member(
    State(state): State<EnterpriseState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
    Json(request): Json<UpdateMemberRequest>,
) -> Result<Json<Value>, ApiError> {
    let admin = state.require_admin(&headers).await?;
    if let Some(control) = state.control_store() {
        return update_member_postgres(&state, &control, &admin, &id, request).await;
    }
    let row = sqlx::query(
        "SELECT tenant_id, display_name, email, role, base_role, status
         FROM enterprise_members WHERE id = ?",
    )
    .bind(&id)
    .fetch_optional(&state.control)
    .await
    .map_err(internal)?
    .ok_or_else(|| ApiError::not_found(format!("member `{id}` not found")))?;
    if request.display_name.is_none()
        && request.email.is_none()
        && request.role.is_none()
        && request.status.is_none()
    {
        return Err(ApiError::bad_request(
            "MEMBER_UPDATE_EMPTY",
            "at least one member field must be provided",
        ));
    }
    let tenant_id: String = row.get("tenant_id");
    let display_name = match request.display_name.as_deref().map(str::trim) {
        Some("") => {
            return Err(ApiError::bad_request(
                "MEMBER_NAME_INVALID",
                "member display name cannot be empty",
            ));
        }
        Some(value) if value.len() > 200 => {
            return Err(ApiError::bad_request(
                "MEMBER_NAME_INVALID",
                "member display name must be at most 200 characters",
            ));
        }
        Some(value) => value.to_string(),
        None => row.get("display_name"),
    };
    let email = match request.email.as_deref() {
        Some(value) => normalize_member_email(Some(value))?,
        None => row.get("email"),
    };
    let requested_role = match request.role.as_deref() {
        Some(value) => {
            validate_role(value)?;
            Some(value.to_string())
        }
        None => None,
    };
    let mapped_roles: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT g.role
         FROM enterprise_scim_groups g
         JOIN enterprise_scim_group_members gm
           ON gm.group_id = g.id AND gm.tenant_id = g.tenant_id
         WHERE gm.tenant_id = ? AND gm.member_id = ? AND g.role IS NOT NULL",
    )
    .bind(&tenant_id)
    .bind(&id)
    .fetch_all(&state.control)
    .await
    .map_err(internal)?;
    if requested_role.is_some() && !mapped_roles.is_empty() {
        return Err(ApiError::conflict(
            "MEMBER_ROLE_MANAGED_BY_GROUP",
            "member role is managed by a SCIM Group; remove the group mapping first",
        ));
    }
    let base_role = requested_role
        .clone()
        .unwrap_or_else(|| row.get("base_role"));
    let role = requested_role.unwrap_or_else(|| row.get("role"));
    let status = match request.status.as_deref() {
        Some(value) => {
            validate_member_status(value)?;
            value.to_string()
        }
        None => row.get("status"),
    };
    sqlx::query(
        "UPDATE enterprise_members
         SET display_name = ?, email = ?, role = ?, base_role = ?, status = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(&display_name)
    .bind(&email)
    .bind(&role)
    .bind(&base_role)
    .bind(&status)
    .bind(now_rfc3339())
    .bind(&id)
    .execute(&state.control)
    .await
    .map_err(internal)?;
    state
        .audit(
            Some(&tenant_id),
            Some(&admin.token_id),
            "token",
            "member.update",
            "enterprise_member",
            "allow",
            None,
            json!({"memberId": id, "role": role, "status": status}),
        )
        .await;
    let row = sqlx::query(
        "SELECT id, tenant_id, subject, display_name, email, role, status,
                created_at, updated_at FROM enterprise_members WHERE id = ?",
    )
    .bind(&id)
    .fetch_one(&state.control)
    .await
    .map_err(internal)?;
    mirror_member_row(&state, &row).await;
    Ok(Json(member_value(&row)))
}

async fn update_member_postgres(
    state: &EnterpriseState,
    control: &PostgresControlStore,
    admin: &Principal,
    id: &str,
    request: UpdateMemberRequest,
) -> Result<Json<Value>, ApiError> {
    let member = control
        .scim_member_any_id(id)
        .await
        .map_err(control_error)?
        .ok_or_else(|| ApiError::not_found(format!("member `{id}` not found")))?;
    if request.display_name.is_none()
        && request.email.is_none()
        && request.role.is_none()
        && request.status.is_none()
    {
        return Err(ApiError::bad_request(
            "MEMBER_UPDATE_EMPTY",
            "at least one member field must be provided",
        ));
    }
    let display_name = match request.display_name.as_deref().map(str::trim) {
        Some("") => {
            return Err(ApiError::bad_request(
                "MEMBER_NAME_INVALID",
                "member display name cannot be empty",
            ));
        }
        Some(value) if value.len() > 200 => {
            return Err(ApiError::bad_request(
                "MEMBER_NAME_INVALID",
                "member display name must be at most 200 characters",
            ));
        }
        Some(value) => value.to_string(),
        None => member.display_name,
    };
    let email = match request.email.as_deref() {
        Some(value) => normalize_member_email(Some(value))?,
        None => member.email,
    };
    let requested_role = match request.role.as_deref() {
        Some(value) => {
            validate_role(value)?;
            Some(value.to_string())
        }
        None => None,
    };
    let mapped_roles = control
        .scim_group_roles_for_member(&member.tenant_id, id)
        .await
        .map_err(control_error)?;
    if requested_role.is_some() && !mapped_roles.is_empty() {
        return Err(ApiError::conflict(
            "MEMBER_ROLE_MANAGED_BY_GROUP",
            "member role is managed by a SCIM Group; remove the group mapping first",
        ));
    }
    let base_role = requested_role
        .clone()
        .unwrap_or_else(|| member.base_role.clone());
    let role = requested_role.unwrap_or_else(|| member.role.clone());
    let status = match request.status.as_deref() {
        Some(value) => {
            validate_member_status(value)?;
            value.to_string()
        }
        None => member.status,
    };
    let updated = ScimMemberRecord {
        display_name,
        email,
        role,
        base_role,
        status,
        updated_at: now_rfc3339(),
        ..member
    };
    control
        .scim_update_member(&updated)
        .await
        .map_err(control_error)?;
    state
        .audit(
            Some(&updated.tenant_id),
            Some(&admin.token_id),
            "token",
            "member.update",
            "enterprise_member",
            "allow",
            None,
            json!({"memberId": id, "role": updated.role, "status": updated.status}),
        )
        .await;
    let stored = control
        .scim_member_by_id(&updated.tenant_id, id)
        .await
        .map_err(control_error)?
        .ok_or_else(|| ApiError::not_found(format!("member `{id}` not found")))?;
    Ok(Json(scim_member_value(&stored)))
}

async fn delete_member(
    State(state): State<EnterpriseState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let admin = state.require_admin(&headers).await?;
    if let Some(control) = state.control_store() {
        let member = control
            .scim_member_any_id(&id)
            .await
            .map_err(control_error)?
            .ok_or_else(|| ApiError::not_found(format!("member `{id}` not found")))?;
        control
            .scim_delete_member_any_id(&id)
            .await
            .map_err(control_error)?;
        state
            .audit(
                Some(&member.tenant_id),
                Some(&admin.token_id),
                "token",
                "member.delete",
                "enterprise_member",
                "allow",
                None,
                json!({"memberId": id}),
            )
            .await;
        return Ok(Json(json!({"deleted": true})));
    }
    let tenant_id: Option<String> =
        sqlx::query_scalar("SELECT tenant_id FROM enterprise_members WHERE id = ?")
            .bind(&id)
            .fetch_optional(&state.control)
            .await
            .map_err(internal)?;
    let Some(tenant_id) = tenant_id else {
        return Err(ApiError::not_found(format!("member `{id}` not found")));
    };
    if let Some(control) = state.control_store()
        && let Err(error) = control.delete_member(&id).await
    {
        tracing::warn!(error = %error, member = %id, "cannot delete the PostgreSQL member mirror");
    }
    sqlx::query("DELETE FROM enterprise_members WHERE id = ?")
        .bind(&id)
        .execute(&state.control)
        .await
        .map_err(internal)?;
    state
        .audit(
            Some(&tenant_id),
            Some(&admin.token_id),
            "token",
            "member.delete",
            "enterprise_member",
            "allow",
            None,
            json!({"memberId": id}),
        )
        .await;
    Ok(Json(json!({"deleted": true})))
}

fn normalize_member_email(value: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if value.len() > 320 || !value.contains('@') {
        return Err(ApiError::bad_request(
            "MEMBER_EMAIL_INVALID",
            "member email must contain @ and be at most 320 characters",
        ));
    }
    Ok(Some(value.to_ascii_lowercase()))
}

fn validate_member_status(value: &str) -> Result<(), ApiError> {
    if matches!(value, "active" | "disabled") {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "MEMBER_STATUS_INVALID",
            "member status must be active or disabled",
        ))
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct TenantQuery {
    tenant_id: Option<String>,
}

async fn get_quota_handler(
    State(state): State<EnterpriseState>,
    AxumPath(tenant_id): AxumPath<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    state.require_admin(&headers).await?;
    Ok(Json(quota_value(
        &tenant_id,
        &state.quota(&tenant_id).await?,
    )))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateQuotaRequest {
    max_workflows: i64,
    max_executions_per_day: i64,
    max_skills: i64,
    max_agents: i64,
    max_plugins: i64,
    max_connections: i64,
    max_concurrent_runs: i64,
}

async fn put_quota_handler(
    State(state): State<EnterpriseState>,
    AxumPath(tenant_id): AxumPath<String>,
    headers: HeaderMap,
    Json(request): Json<UpdateQuotaRequest>,
) -> Result<Json<Value>, ApiError> {
    let admin = state.require_admin(&headers).await?;
    let values = [
        request.max_workflows,
        request.max_executions_per_day,
        request.max_skills,
        request.max_agents,
        request.max_plugins,
        request.max_connections,
        request.max_concurrent_runs,
    ];
    if values.iter().any(|value| *value < 0) {
        return Err(ApiError::bad_request(
            "QUOTA_INVALID",
            "quota values must be non-negative",
        ));
    }
    if let Some(control) = state.control_store() {
        let quota = QuotaRecord {
            max_workflows: request.max_workflows,
            max_executions_per_day: request.max_executions_per_day,
            max_skills: request.max_skills,
            max_agents: request.max_agents,
            max_plugins: request.max_plugins,
            max_connections: request.max_connections,
            max_concurrent_runs: request.max_concurrent_runs,
        };
        // PostgreSQL is authoritative: a failure here fails the request.
        control
            .upsert_quota(&tenant_id, &quota)
            .await
            .map_err(|error| ApiError::internal(error.to_string()))?;
        // The local SQLite row is only a cache for single-node reads; a node
        // that never saw this tenant simply upserts it.
        let _ = sqlx::query(
            "INSERT INTO enterprise_quotas
             (tenant_id, max_workflows, max_executions_per_day, max_skills, max_agents,
              max_plugins, max_connections, max_concurrent_runs)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(tenant_id) DO UPDATE SET
               max_workflows = excluded.max_workflows,
               max_executions_per_day = excluded.max_executions_per_day,
               max_skills = excluded.max_skills,
               max_agents = excluded.max_agents,
               max_plugins = excluded.max_plugins,
               max_connections = excluded.max_connections,
               max_concurrent_runs = excluded.max_concurrent_runs",
        )
        .bind(&tenant_id)
        .bind(request.max_workflows)
        .bind(request.max_executions_per_day)
        .bind(request.max_skills)
        .bind(request.max_agents)
        .bind(request.max_plugins)
        .bind(request.max_connections)
        .bind(request.max_concurrent_runs)
        .execute(&state.control)
        .await;
    } else {
        let result = sqlx::query(
            "UPDATE enterprise_quotas SET max_workflows = ?, max_executions_per_day = ?,
                    max_skills = ?, max_agents = ?, max_plugins = ?, max_connections = ?,
                    max_concurrent_runs = ? WHERE tenant_id = ?",
        )
        .bind(request.max_workflows)
        .bind(request.max_executions_per_day)
        .bind(request.max_skills)
        .bind(request.max_agents)
        .bind(request.max_plugins)
        .bind(request.max_connections)
        .bind(request.max_concurrent_runs)
        .bind(&tenant_id)
        .execute(&state.control)
        .await
        .map_err(internal)?;
        if result.rows_affected() == 0 {
            return Err(ApiError::not_found("tenant quotas not found"));
        }
    }
    state
        .audit(
            Some(&tenant_id),
            Some(&admin.token_id),
            "token",
            "quota.update",
            "quota",
            "allow",
            None,
            json!({
                "maxWorkflows": request.max_workflows,
                "maxExecutionsPerDay": request.max_executions_per_day,
                "maxSkills": request.max_skills,
                "maxAgents": request.max_agents,
                "maxPlugins": request.max_plugins,
                "maxConnections": request.max_connections,
                "maxConcurrentRuns": request.max_concurrent_runs,
            }),
        )
        .await;
    Ok(Json(quota_value(
        &tenant_id,
        &state.quota(&tenant_id).await?,
    )))
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct AuditQuery {
    tenant_id: Option<String>,
    limit: Option<i64>,
}

async fn list_audit(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
    Query(query): Query<AuditQuery>,
) -> Result<Json<Value>, ApiError> {
    state.require_admin(&headers).await?;
    let entries = audit_rows(
        &state.control,
        query.tenant_id.as_deref(),
        query.limit.unwrap_or(50),
    )
    .await?;
    Ok(Json(json!({"entries": entries})))
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct AuditExportQuery {
    tenant_id: Option<String>,
    format: Option<String>,
    limit: Option<i64>,
}

async fn export_audit(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
    Query(query): Query<AuditExportQuery>,
) -> Result<Response, ApiError> {
    state.require_admin(&headers).await?;
    let entries = audit_rows(
        &state.control,
        query.tenant_id.as_deref(),
        query.limit.unwrap_or(1000),
    )
    .await?;
    if query.format.as_deref() == Some("csv") {
        let mut output =
            String::from("at,tenantId,actorId,actorType,action,resource,decision,reason\n");
        for entry in &entries {
            output.push_str(&format!(
                "{},{},{},{},{},{},{},{}\n",
                csv(&entry["at"]),
                csv(&entry["tenantId"]),
                csv(&entry["actorId"]),
                csv(&entry["actorType"]),
                csv(&entry["action"]),
                csv(&entry["resource"]),
                csv(&entry["decision"]),
                csv(&entry["reason"]),
            ));
        }
        return Ok(([(header::CONTENT_TYPE, "text/csv; charset=utf-8")], output).into_response());
    }
    let body = serde_json::to_string_pretty(&json!({"entries": entries}))
        .map_err(|error| ApiError::internal(error.to_string()))?;
    Ok((
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        body,
    )
        .into_response())
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct OpsQuery {
    tenant_id: Option<String>,
}

async fn list_workers(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
    Query(query): Query<OpsQuery>,
) -> Result<Json<Value>, ApiError> {
    state.require_admin(&headers).await?;
    let workers = if state.workers.has_postgres() {
        state
            .workers
            .list_workers_pg(query.tenant_id.as_deref())
            .await
            .map_err(worker_error)?
    } else {
        state
            .ops
            .list_workers(query.tenant_id.as_deref())
            .await
            .map_err(ops_error)?
            .into_iter()
            .map(normalize_worker)
            .collect()
    };
    Ok(Json(json!({
        "workers": workers,
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisterWorkerRequest {
    tenant_id: String,
    name: String,
    kind: String,
    metadata: Option<Value>,
    capabilities: Option<Vec<String>>,
    max_concurrency: Option<i64>,
}

async fn register_worker(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
    Json(request): Json<RegisterWorkerRequest>,
) -> Result<Json<Value>, ApiError> {
    let admin = state.require_admin(&headers).await?;
    ensure_tenant(&state, &request.tenant_id).await?;
    let capabilities = request
        .capabilities
        .unwrap_or_else(|| vec!["workflow".to_string()]);
    let max_concurrency = request.max_concurrency.unwrap_or(1);
    let metadata = request.metadata.unwrap_or(Value::Null);
    let worker = if state.workers.has_postgres() {
        state.ensure_postgres_tenant(&request.tenant_id).await?;
        state
            .workers
            .register_worker_pg(
                &request.tenant_id,
                &request.name,
                &request.kind,
                &metadata,
                &capabilities,
                max_concurrency,
            )
            .await
            .map_err(worker_error)?
    } else {
        let worker = state
            .ops
            .register_worker_config(
                &request.tenant_id,
                &request.name,
                &request.kind,
                metadata,
                capabilities.clone(),
                max_concurrency,
            )
            .await
            .map_err(ops_error)?;
        state
            .workers
            .sync_worker_value(&worker)
            .await
            .map_err(worker_error)?;
        normalize_worker(worker)
    };
    let worker_id = worker
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::internal("worker id missing"))?;
    let (token, token_id) = state
        .workers
        .mint_worker_token(worker_id)
        .await
        .map_err(worker_error)?;
    state
        .audit(
            Some(&request.tenant_id),
            Some(&admin.token_id),
            "token",
            "worker.register",
            "worker",
            "allow",
            None,
            json!({
                "name": request.name,
                "kind": request.kind,
                "capabilities": capabilities,
                "maxConcurrency": max_concurrency,
                "workerTokenId": token_id,
            }),
        )
        .await;
    let mut value = normalize_worker(worker);
    if let Some(object) = value.as_object_mut() {
        object.insert("workerToken".to_string(), Value::String(token));
    }
    Ok(Json(value))
}

async fn worker_heartbeat(
    State(state): State<EnterpriseState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    state.require_admin(&headers).await?;
    if state.workers.has_postgres() {
        let worker = state
            .workers
            .heartbeat_worker_pg(&id)
            .await
            .map_err(worker_error)?;
        return Ok(Json(normalize_worker(worker)));
    }
    let worker = state.ops.heartbeat_worker(&id).await.map_err(ops_error)?;
    state
        .workers
        .sync_worker_value(&worker)
        .await
        .map_err(worker_error)?;
    Ok(Json(normalize_worker(worker)))
}

async fn disable_worker(
    State(state): State<EnterpriseState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let admin = state.require_admin(&headers).await?;
    let worker = if state.workers.has_postgres() {
        state
            .workers
            .disable_worker_pg(&id)
            .await
            .map_err(worker_error)?
    } else {
        let worker = state.ops.disable_worker(&id).await.map_err(ops_error)?;
        state
            .workers
            .sync_worker_value(&worker)
            .await
            .map_err(worker_error)?;
        worker
    };
    let _ = state.workers.revoke_worker_tokens(&id).await;
    state
        .audit(
            worker.get("tenantId").and_then(Value::as_str),
            Some(&admin.token_id),
            "token",
            "worker.disable",
            "worker",
            "allow",
            None,
            json!({"workerId": id}),
        )
        .await;
    Ok(Json(normalize_worker(worker)))
}

async fn rotate_worker_token(
    State(state): State<EnterpriseState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let admin = state.require_admin(&headers).await?;
    let (token, token_id) = state
        .workers
        .mint_worker_token(&id)
        .await
        .map_err(worker_error)?;
    state
        .audit(
            None,
            Some(&admin.token_id),
            "token",
            "worker.token.rotate",
            "worker",
            "allow",
            None,
            json!({"workerId": id, "tokenId": token_id}),
        )
        .await;
    Ok(Json(json!({"workerToken": token, "tokenId": token_id})))
}

async fn list_worker_jobs(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
    Query(query): Query<OpsQuery>,
) -> Result<Json<Value>, ApiError> {
    state.require_admin(&headers).await?;
    let jobs = state
        .workers
        .list_jobs(query.tenant_id.as_deref(), 200)
        .await
        .map_err(worker_error)?;
    Ok(Json(json!({
        "jobs": jobs.iter().map(WorkerJob::public_json).collect::<Vec<_>>(),
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateWorkerJobRequest {
    tenant_id: String,
    workflow_id: String,
    worker_id: Option<String>,
    input: Option<Value>,
    version: Option<String>,
}

async fn create_worker_job(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
    Json(request): Json<CreateWorkerJobRequest>,
) -> Result<Json<Value>, ApiError> {
    let admin = state.require_admin(&headers).await?;
    let entry = state.tenant_entry(&request.tenant_id).await?;
    let (workflow, frozen_registry) = if let Some(version) = request.version.as_deref() {
        let row = entry
            .state
            .db
            .get_version(&request.workflow_id, version)
            .await?
            .ok_or_else(|| {
                ApiError::not_found(format!(
                    "workflow `{}` has no published version `{version}`",
                    request.workflow_id
                ))
            })?;
        (
            serde_json::from_str::<Workflow>(&row.ir_json)?,
            Some(row.registry_version),
        )
    } else {
        let row = entry
            .state
            .db
            .get_workflow(&request.workflow_id)
            .await?
            .ok_or_else(|| {
                ApiError::not_found(format!("workflow `{}` not found", request.workflow_id))
            })?;
        (serde_json::from_str::<Workflow>(&row.ir_json)?, None)
    };
    let compilation = compile_workflow(&workflow).map_err(|issues| {
        ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_FAILED",
            "workflow does not compile",
        )
        .with_details(serde_json::to_value(issues).unwrap_or(Value::Null))
    })?;
    let execution_id = format!("exec_{}", Uuid::now_v7());
    let mut execution_state = new_state(
        &workflow,
        &compilation.plan,
        &execution_id,
        request.input.unwrap_or_else(|| json!({})),
        Default::default(),
        &request.tenant_id,
    )
    .map_err(|error| ApiError::bad_request("INPUT_INVALID", error.to_string()))?;
    if let Some(registry_version) = frozen_registry
        && !registry_version.is_empty()
    {
        execution_state.registry_version = registry_version;
    }
    let workflow_json = workflow
        .to_json_string(false)
        .map_err(|error| ApiError::internal(format!("cannot serialize workflow: {error}")))?;
    let state_json = serde_json::to_string(&execution_state)?;
    let job = state
        .workers
        .enqueue_job(
            &request.tenant_id,
            request.worker_id.as_deref(),
            &execution_id,
            &workflow.id,
            &workflow.workflow_version,
            &workflow_json,
            &state_json,
            env!("CARGO_PKG_VERSION"),
        )
        .await
        .map_err(worker_error)?;
    state
        .audit(
            Some(&request.tenant_id),
            Some(&admin.token_id),
            "token",
            "worker.job.create",
            "worker_job",
            "allow",
            None,
            json!({
                "jobId": job.id,
                "executionId": job.execution_id,
                "workerId": job.worker_id,
                "workflowId": job.workflow_id,
            }),
        )
        .await;
    Ok(Json(job.public_json()))
}

async fn cancel_worker_job(
    State(state): State<EnterpriseState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let admin = state.require_admin(&headers).await?;
    let _ = state.workers.expire_leases().await;
    let jobs = state
        .workers
        .list_jobs(None, 200)
        .await
        .map_err(worker_error)?;
    let job = jobs
        .into_iter()
        .find(|job| job.id == id)
        .ok_or_else(|| ApiError::not_found(format!("worker job `{id}` not found")))?;
    state
        .workers
        .get_job(&job.tenant_id, &id)
        .await
        .map_err(worker_error)?
        .ok_or_else(|| ApiError::not_found(format!("worker job `{id}` not found")))?;
    let cancelled = state
        .workers
        .cancel_job(&job.tenant_id, &id)
        .await
        .map_err(worker_error)?;
    if let Some(row) = state
        .tenant_entry(&cancelled.tenant_id)
        .await?
        .state
        .db
        .get_execution(&cancelled.execution_id)
        .await?
    {
        let mut execution = parse_state(&row)?;
        if !execution.status.is_terminal() {
            let events = cancel_execution_state(&mut execution)
                .map_err(|error| ApiError::conflict("CANCEL_FAILED", error.to_string()))?;
            state
                .tenant_entry(&cancelled.tenant_id)
                .await?
                .state
                .db
                .save_execution(&execution, &events)
                .await?;
        }
    }
    state
        .audit(
            Some(&cancelled.tenant_id),
            Some(&admin.token_id),
            "token",
            "worker.job.cancel",
            "worker_job",
            "allow",
            None,
            json!({"jobId": id}),
        )
        .await;
    Ok(Json(cancelled.public_json()))
}

async fn list_tenant_workflows(
    State(state): State<EnterpriseState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    state.require_admin(&headers).await?;
    let entry = state.tenant_entry(&id).await?;
    let workflows = entry.state.db.list_workflows().await?;
    Ok(Json(json!({
        "workflows": workflows
            .iter()
            .map(|workflow| json!({
                "id": workflow.id,
                "name": workflow.name,
                "version": workflow.version,
                "description": workflow.description,
                "published": workflow.published != 0,
                "createdAt": workflow.created_at,
                "updatedAt": workflow.updated_at,
            }))
            .collect::<Vec<_>>(),
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkerAgentHeartbeat {
    version: String,
    #[serde(default)]
    capabilities: Vec<String>,
    max_concurrency: i64,
    #[serde(default)]
    active_jobs: Vec<String>,
}

async fn worker_agent_heartbeat(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
    Json(request): Json<WorkerAgentHeartbeat>,
) -> Result<Json<Value>, ApiError> {
    let auth = require_worker(&state, &headers).await?;
    let cancel_jobs = state
        .workers
        .heartbeat(
            &auth,
            &request.version,
            &request.capabilities,
            request.max_concurrency,
            &request.active_jobs,
        )
        .await
        .map_err(worker_error)?;
    Ok(Json(json!({
        "workerId": auth.worker_id,
        "tenantId": auth.tenant_id,
        "serverTime": now_millis(),
        "cancelJobs": cancel_jobs,
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkerAgentLease {
    slots: i64,
    version: String,
    #[serde(default)]
    capabilities: Vec<String>,
}

async fn worker_agent_lease(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
    Json(request): Json<WorkerAgentLease>,
) -> Result<Json<Value>, ApiError> {
    let auth = require_worker(&state, &headers).await?;
    let (jobs, cancel_jobs) = state
        .workers
        .lease_jobs(
            &auth,
            request.slots,
            &request.version,
            &request.capabilities,
        )
        .await
        .map_err(worker_error)?;
    Ok(Json(json!({
        "jobs": jobs.iter().map(WorkerJob::lease_json).collect::<Vec<_>>(),
        "cancelJobs": cancel_jobs,
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkerStateReport {
    lease_id: String,
    state: Value,
    #[serde(default)]
    events: Vec<ExecutionEvent>,
}

async fn worker_agent_start(
    State(state): State<EnterpriseState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
    Json(request): Json<WorkerStateReport>,
) -> Result<Json<Value>, ApiError> {
    let auth = require_worker(&state, &headers).await?;
    let job = state
        .workers
        .start_job(&auth, &id, &request.lease_id)
        .await
        .map_err(worker_error)?;
    let execution_state: ExecutionState = serde_json::from_value(request.state.clone())?;
    validate_worker_state(&job, &execution_state)?;
    persist_worker_state(&state, &job, &execution_state, &request.events).await?;
    Ok(Json(json!({"accepted": true})))
}

async fn worker_agent_events(
    State(state): State<EnterpriseState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
    Json(request): Json<WorkerStateReport>,
) -> Result<Json<Value>, ApiError> {
    let auth = require_worker(&state, &headers).await?;
    let execution_state: ExecutionState = serde_json::from_value(request.state)?;
    let status = match execution_state.status {
        hpa_core::runtime::ExecutionStatus::Running => "running",
        hpa_core::runtime::ExecutionStatus::WaitingApproval => "waiting_approval",
        _ => {
            return Err(ApiError::bad_request(
                "WORKER_EVENTS_TERMINAL",
                "terminal execution state must be reported through /complete",
            ));
        }
    };
    let state_json = serde_json::to_string(&execution_state)?;
    let job = state
        .workers
        .report_job(&auth, &id, &request.lease_id, status, &state_json)
        .await
        .map_err(worker_error)?;
    validate_worker_state(&job, &execution_state)?;
    persist_worker_state(&state, &job, &execution_state, &request.events).await?;
    Ok(Json(json!({"accepted": true})))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkerCompleteReport {
    lease_id: String,
    status: String,
    state: Value,
    #[serde(default)]
    events: Vec<ExecutionEvent>,
    error: Option<String>,
}

async fn worker_agent_complete(
    State(state): State<EnterpriseState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
    Json(request): Json<WorkerCompleteReport>,
) -> Result<Json<Value>, ApiError> {
    let auth = require_worker(&state, &headers).await?;
    let execution_state: ExecutionState = serde_json::from_value(request.state)?;
    let state_json = serde_json::to_string(&execution_state)?;
    let job = state
        .workers
        .complete_job(
            &auth,
            &id,
            &request.lease_id,
            &request.status,
            &state_json,
            request.error.as_deref(),
        )
        .await
        .map_err(worker_error)?;
    validate_worker_state(&job, &execution_state)?;
    persist_worker_state(&state, &job, &execution_state, &request.events).await?;
    Ok(Json(json!({"accepted": true})))
}

async fn require_worker(
    state: &EnterpriseState,
    headers: &HeaderMap,
) -> Result<WorkerAuth, ApiError> {
    let value = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| unauthorized("worker bearer token required"))?;
    let token = value
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|token| token.starts_with("hpa_wrk_"))
        .ok_or_else(|| unauthorized("worker bearer token required"))?;
    state
        .workers
        .authenticate_worker(token)
        .await
        .map_err(worker_error)?
        .ok_or_else(|| unauthorized("worker token is not recognized"))
}

fn validate_worker_state(
    job: &WorkerJob,
    execution_state: &ExecutionState,
) -> Result<(), ApiError> {
    validate_worker_state_values(&job.execution_id, execution_state)?;
    if execution_state.workflow_id != job.workflow_id
        || execution_state.workflow_version != job.workflow_version
    {
        return Err(ApiError::conflict(
            "WORKER_STATE_MISMATCH",
            "worker state does not match the leased workflow",
        ));
    }
    Ok(())
}

fn validate_worker_state_values(
    execution_id: &str,
    execution_state: &ExecutionState,
) -> Result<(), ApiError> {
    if execution_state.id != execution_id {
        return Err(ApiError::conflict(
            "WORKER_STATE_MISMATCH",
            "worker state execution id does not match the job",
        ));
    }
    Ok(())
}

async fn persist_worker_state(
    state: &EnterpriseState,
    job: &WorkerJob,
    execution_state: &ExecutionState,
    events: &[ExecutionEvent],
) -> Result<(), ApiError> {
    let entry = state.tenant_entry(&job.tenant_id).await?;
    let db = &entry.state.db;
    let existing = db.get_execution(&job.execution_id).await?;
    if existing.is_none() {
        let start = events
            .iter()
            .find(|event| event.kind == "execution.started")
            .ok_or_else(|| {
                ApiError::conflict(
                    "WORKER_START_REQUIRED",
                    "first worker report must include execution.started",
                )
            })?;
        let row = ExecutionRow {
            id: execution_state.id.clone(),
            workflow_id: execution_state.workflow_id.clone(),
            workflow_version: execution_state.workflow_version.clone(),
            status: status_text(execution_state.status).to_string(),
            state_json: serde_json::to_string(execution_state)?,
            workflow_json: job.workflow_json.clone(),
            output_json: execution_state
                .output
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?,
            error_json: execution_state
                .error
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?,
            started_at: execution_state.started_at.clone(),
            updated_at: execution_state.updated_at.clone(),
        };
        db.insert_execution(&row, start).await?;
        let rest = events
            .iter()
            .filter(|event| event.id != start.id)
            .cloned()
            .collect::<Vec<_>>();
        db.save_execution(execution_state, &rest).await?;
    } else {
        db.save_execution(execution_state, events).await?;
    }
    // Mark the execution as remote only after its row exists: PostgreSQL
    // enforces the (tenant_id, execution_id) foreign key on remote_executions.
    if !db.is_remote_execution(&job.execution_id).await? {
        db.mark_remote_execution(
            &job.execution_id,
            job.worker_id.as_deref().unwrap_or_default(),
        )
        .await?;
    }
    if execution_state.status.is_terminal() {
        db.save_output_artifact(execution_state).await?;
    }
    Ok(())
}

fn worker_error(error: WorkerError) -> ApiError {
    match error.kind {
        WorkerErrorKind::Invalid => ApiError::bad_request("WORKER_INVALID", error.message),
        WorkerErrorKind::NotFound => ApiError::not_found(error.message),
        WorkerErrorKind::Conflict => ApiError::conflict("WORKER_CONFLICT", error.message),
        WorkerErrorKind::Forbidden => {
            ApiError::new(StatusCode::FORBIDDEN, "WORKER_FORBIDDEN", error.message)
        }
        WorkerErrorKind::Internal => ApiError::internal(error.message),
    }
}

fn now_millis() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp_nanos() as i64 / 1_000_000
}

async fn list_backups(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
    Query(query): Query<OpsQuery>,
) -> Result<Json<Value>, ApiError> {
    state.require_admin(&headers).await?;
    let backups = state
        .ops
        .list_backups(query.tenant_id.as_deref())
        .await
        .map_err(ops_error)?;
    Ok(Json(json!({
        "backups": backups.into_iter().map(normalize_backup).collect::<Vec<_>>(),
    })))
}

async fn create_backup(
    State(state): State<EnterpriseState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let admin = state.require_admin(&headers).await?;
    ensure_tenant(&state, &id).await?;
    let backup = state.ops.create_backup(&id).await.map_err(ops_error)?;
    state
        .audit(
            Some(&id),
            Some(&admin.token_id),
            "token",
            "backup.create",
            "backup",
            "allow",
            None,
            json!({"backupId": backup.get("id").cloned().unwrap_or(Value::Null)}),
        )
        .await;
    Ok(Json(normalize_backup(backup)))
}

async fn delete_backup(
    State(state): State<EnterpriseState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let admin = state.require_admin(&headers).await?;
    let deleted = state.ops.delete_backup(&id).await.map_err(ops_error)?;
    state
        .audit(
            None,
            Some(&admin.token_id),
            "token",
            "backup.delete",
            "backup",
            "allow",
            None,
            json!({"backupId": id}),
        )
        .await;
    Ok(Json(json!({"deleted": deleted})))
}

async fn restore_backup(
    State(state): State<EnterpriseState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let admin = state.require_admin(&headers).await?;
    let tenant_id = state
        .ops
        .list_backups(None)
        .await
        .map_err(ops_error)?
        .into_iter()
        .find(|backup| backup.get("id").and_then(Value::as_str) == Some(id.as_str()))
        .and_then(|backup| {
            backup
                .get("tenantId")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    let result = state.ops.restore_backup(&id).await.map_err(ops_error)?;
    if let Some(tenant_id) = tenant_id.as_deref() {
        state.cache.lock().await.remove(tenant_id);
    }
    state
        .audit(
            tenant_id.as_deref(),
            Some(&admin.token_id),
            "token",
            "backup.restore",
            "backup",
            "allow",
            None,
            json!({"backupId": id, "requiresRestart": true}),
        )
        .await;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PurgeBackupsRequest {
    #[serde(default = "default_retain_days")]
    retain_days: i64,
    #[serde(default = "default_retain_count")]
    retain_count: i64,
}

fn default_retain_days() -> i64 {
    30
}

fn default_retain_count() -> i64 {
    10
}

async fn purge_backups(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
    Json(request): Json<PurgeBackupsRequest>,
) -> Result<Json<Value>, ApiError> {
    let admin = state.require_admin(&headers).await?;
    let deleted = state
        .ops
        .purge_backups(
            request.retain_days.clamp(0, 3650),
            request.retain_count.clamp(0, 1000),
        )
        .await
        .map_err(ops_error)?;
    state
        .audit(
            None,
            Some(&admin.token_id),
            "token",
            "backup.purge",
            "backup",
            "allow",
            None,
            json!({"deleted": deleted, "retainDays": request.retain_days, "retainCount": request.retain_count}),
        )
        .await;
    Ok(Json(json!({"deleted": deleted})))
}

async fn get_oidc(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    state.require_admin(&headers).await?;
    let config = state.ops.get_oidc_config().await.map_err(ops_error)?;
    Ok(Json(normalize_oidc_config(config)))
}

fn normalize_oidc_config(mut config: Value) -> Value {
    if let Some(scopes) = config.get("scopes").and_then(Value::as_str) {
        let values = scopes
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>();
        config["scopes"] = json!(values);
    }
    config
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OidcConfigRequest {
    issuer: String,
    audience: String,
    jwks_url: String,
    #[serde(default)]
    network: Vec<String>,
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    authorization_endpoint: Option<String>,
    #[serde(default)]
    token_endpoint: Option<String>,
    #[serde(default)]
    redirect_uri: Option<String>,
    #[serde(default)]
    scopes: Option<ScopeInput>,
    #[serde(default)]
    admin_subjects: Option<Vec<String>>,
    #[serde(default)]
    cookie_secure: Option<bool>,
    #[serde(default)]
    session_ttl_seconds: Option<i64>,
    #[serde(default)]
    end_session_endpoint: Option<String>,
    #[serde(default)]
    post_logout_redirect_uri: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ScopeInput {
    Many(Vec<String>),
    Single(String),
}

impl ScopeInput {
    fn into_list(self) -> Vec<String> {
        match self {
            Self::Many(values) => values,
            Self::Single(value) => value
                .split(|character: char| character.is_whitespace() || character == ',')
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
                .map(str::to_string)
                .collect(),
        }
    }
}

async fn set_oidc(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
    Json(request): Json<OidcConfigRequest>,
) -> Result<Json<Value>, ApiError> {
    let admin = state.require_admin(&headers).await?;
    let scopes = request
        .scopes
        .map(ScopeInput::into_list)
        .unwrap_or_else(|| {
            vec![
                "openid".to_string(),
                "email".to_string(),
                "profile".to_string(),
            ]
        })
        .join(" ");
    let config = state
        .ops
        .set_browser_oidc_config(
            &request.issuer,
            &request.audience,
            &request.jwks_url,
            request.network,
            request.client_id.as_deref().unwrap_or(""),
            request.authorization_endpoint.as_deref().unwrap_or(""),
            request.token_endpoint.as_deref().unwrap_or(""),
            request.redirect_uri.as_deref().unwrap_or(""),
            &scopes,
            request.admin_subjects.unwrap_or_default(),
            request.cookie_secure.unwrap_or(false),
            request.session_ttl_seconds.unwrap_or(28_800),
            request.end_session_endpoint.as_deref().unwrap_or(""),
            request.post_logout_redirect_uri.as_deref().unwrap_or(""),
        )
        .await
        .map_err(ops_error)?;
    state
        .audit(
            None,
            Some(&admin.token_id),
            "token",
            "oidc.update",
            "oidc",
            "allow",
            None,
            json!({
                "issuer": request.issuer,
                "audience": request.audience,
                "browserSso": config["clientId"].as_str().is_some_and(|value| !value.is_empty())
            }),
        )
        .await;
    Ok(Json(normalize_oidc_config(config)))
}

#[derive(Debug, Deserialize)]
struct LoginQuery {
    #[serde(default)]
    redirect: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CallbackQuery {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

async fn oidc_login(
    State(state): State<EnterpriseState>,
    Query(query): Query<LoginQuery>,
) -> Response {
    match state
        .browser_auth
        .begin_login(query.redirect.as_deref())
        .await
    {
        Ok(start) => Redirect::to(&start.location).into_response(),
        Err(error) => login_error_redirect(browser_error_code(&error)),
    }
}

async fn oidc_callback(
    State(state): State<EnterpriseState>,
    Query(query): Query<CallbackQuery>,
) -> Response {
    if let Some(error) = query.error.as_deref() {
        return login_error_redirect(safe_error_code(error));
    }
    let (Some(code), Some(state_value)) = (query.code.as_deref(), query.state.as_deref()) else {
        return login_error_redirect("state_invalid");
    };
    match state.browser_auth.complete_login(code, state_value).await {
        Ok(success) => {
            state
                .audit(
                    Some(&success.principal.tenant_id),
                    Some(&success.principal.session_id),
                    "session",
                    "auth.login",
                    "browser_session",
                    "allow",
                    None,
                    json!({
                        "subject": success.principal.subject,
                        "email": success.principal.email,
                        "role": success.principal.role,
                        "platformAdmin": success.principal.platform_admin
                    }),
                )
                .await;
            let mut response = Redirect::to(&success.redirect).into_response();
            if let Ok(cookie) = HeaderValue::from_str(&success.cookie) {
                response.headers_mut().append(header::SET_COOKIE, cookie);
            }
            response
        }
        Err(error) => {
            state
                .audit(
                    None,
                    None,
                    "session",
                    "auth.login",
                    "browser_session",
                    "deny",
                    Some(browser_error_code(&error)),
                    json!({}),
                )
                .await;
            login_error_redirect(browser_error_code(&error))
        }
    }
}

async fn oidc_logout_callback(
    State(state): State<EnterpriseState>,
    Query(query): Query<CallbackQuery>,
) -> Response {
    let Some(state_value) = query.state.as_deref() else {
        return login_error_redirect("logout_state_invalid");
    };
    let validation = state.browser_auth.consume_logout_state(state_value).await;
    if validation.is_err() {
        return login_error_redirect("logout_state_invalid");
    }
    if let Some(error) = query.error.as_deref() {
        return login_error_redirect(safe_error_code(error));
    }
    Redirect::to("/login?loggedOut=1").into_response()
}

async fn auth_session(State(state): State<EnterpriseState>, headers: HeaderMap) -> Response {
    let sso_configured = state.browser_auth.sso_configured().await.unwrap_or(false);
    let mode = if state.require_auth {
        "enterprise"
    } else {
        "local"
    };
    let cookie = headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok());
    let mut stale = false;
    let session = match state.browser_auth.session(cookie).await {
        Ok(session) => {
            stale = cookie.is_some() && session.is_none();
            session
        }
        Err(browser_auth::BrowserAuthError::Forbidden(_))
        | Err(browser_auth::BrowserAuthError::Invalid(_)) => {
            stale = cookie.is_some();
            None
        }
        Err(_) => None,
    };
    let body = match session {
        Some(session) => json!({
            "authenticated": true,
            "ssoConfigured": sso_configured,
            "mode": mode,
            "principal": {
                "tenantId": session.tenant_id,
                "role": session.role,
                "platformAdmin": session.platform_admin,
                "subject": session.subject,
                "email": session.email,
                "displayName": session.display_name
            },
            "csrfToken": session.csrf_token,
            "expiresAt": session.expires_at,
            "accessTokenExpiresAt": session.access_token_expires_at,
            "refreshAvailable": session.refresh_available
        }),
        None => json!({
            "authenticated": false,
            "ssoConfigured": sso_configured,
            "mode": mode,
            "principal": Value::Null,
            "csrfToken": Value::Null,
            "expiresAt": Value::Null,
            "accessTokenExpiresAt": Value::Null,
            "refreshAvailable": false
        }),
    };
    let mut response = Json(body).into_response();
    if stale
        && let Ok(cookie) = HeaderValue::from_str(&state.browser_auth.clear_session_cookie().await)
    {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    response
}

async fn auth_refresh(State(state): State<EnterpriseState>, headers: HeaderMap) -> Response {
    let Some(cookie) = headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
    else {
        return unauthorized("session is required").into_response();
    };
    let principal = match state.browser_auth.session(Some(cookie)).await {
        Ok(Some(principal)) => principal,
        Ok(None) => return unauthorized("session is not valid").into_response(),
        Err(error) => return browser_api_error(error).into_response(),
    };
    let provided = headers
        .get("x-hpa-csrf")
        .and_then(|value| value.to_str().ok());
    if !csrf_token_matches(&principal.csrf_token, provided) {
        return ApiError::new(
            StatusCode::FORBIDDEN,
            "CSRF_REQUIRED",
            "X-HPA-CSRF header is required",
        )
        .into_response();
    }
    match state.browser_auth.refresh(Some(cookie)).await {
        Ok(refreshed) => {
            state
                .audit(
                    Some(&refreshed.principal.tenant_id),
                    Some(&refreshed.principal.session_id),
                    "session",
                    "auth.refresh",
                    "browser_session",
                    "allow",
                    None,
                    json!({
                        "subject": refreshed.principal.subject,
                        "expiresAt": refreshed.principal.expires_at,
                        "accessTokenExpiresAt": refreshed.principal.access_token_expires_at
                    }),
                )
                .await;
            let mut response = Json(json!({
                "authenticated": true,
                "refreshed": true,
                "expiresAt": refreshed.principal.expires_at,
                "accessTokenExpiresAt": refreshed.principal.access_token_expires_at,
                "refreshAvailable": refreshed.principal.refresh_available
            }))
            .into_response();
            if let Ok(cookie) = HeaderValue::from_str(&refreshed.cookie) {
                response.headers_mut().append(header::SET_COOKIE, cookie);
            }
            response
        }
        Err(error) => {
            let code = browser_error_code(&error).to_string();
            state
                .audit(
                    Some(&principal.tenant_id),
                    Some(&principal.session_id),
                    "session",
                    "auth.refresh",
                    "browser_session",
                    "deny",
                    Some(&code),
                    json!({"subject": principal.subject}),
                )
                .await;
            let mut response = browser_api_error(error).into_response();
            if let Ok(cookie) =
                HeaderValue::from_str(&state.browser_auth.clear_session_cookie().await)
            {
                response.headers_mut().append(header::SET_COOKIE, cookie);
            }
            response
        }
    }
}

async fn auth_logout(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let Some(cookie) = headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
    else {
        return Err(unauthorized("session is required"));
    };
    let principal = state
        .browser_auth
        .session(Some(cookie))
        .await
        .map_err(browser_api_error)?
        .ok_or_else(|| unauthorized("session is not valid"))?;
    let provided = headers
        .get("x-hpa-csrf")
        .and_then(|value| value.to_str().ok());
    if !csrf_token_matches(&principal.csrf_token, provided) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "CSRF_REQUIRED",
            "X-HPA-CSRF header is required",
        ));
    }
    let logged_out = state
        .browser_auth
        .logout(Some(cookie))
        .await
        .map_err(internal)?;
    state
        .audit(
            Some(&principal.tenant_id),
            Some(&principal.session_id),
            "session",
            "auth.logout",
            "browser_session",
            "allow",
            None,
            json!({"subject": principal.subject}),
        )
        .await;
    let mut response = Json(json!({
        "loggedOut": logged_out.revoked,
        "logoutUrl": logged_out.logout_url,
    }))
    .into_response();
    if let Ok(cookie) = HeaderValue::from_str(&logged_out.cookie) {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    Ok(response)
}

async fn csrf_guard(
    State(state): State<EnterpriseState>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_string();
    let method = request.method().clone();
    if !path.starts_with("/api/") || matches!(method, Method::GET | Method::HEAD | Method::OPTIONS)
    {
        return next.run(request).await;
    }
    if request.headers().contains_key(header::AUTHORIZATION) {
        return next.run(request).await;
    }
    let Some(cookie) = request
        .headers()
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
    else {
        return next.run(request).await;
    };
    let session = match state.browser_auth.session(Some(cookie)).await {
        Ok(Some(session)) => session,
        Ok(None) => return next.run(request).await,
        Err(error) => return browser_api_error(error).into_response(),
    };
    let provided = request
        .headers()
        .get("x-hpa-csrf")
        .and_then(|value| value.to_str().ok());
    if !csrf_token_matches(&session.csrf_token, provided) {
        return ApiError::new(
            StatusCode::FORBIDDEN,
            "CSRF_REQUIRED",
            "X-HPA-CSRF header is required",
        )
        .into_response();
    }
    next.run(request).await
}

fn browser_api_error(error: browser_auth::BrowserAuthError) -> ApiError {
    match error {
        browser_auth::BrowserAuthError::Forbidden(message)
        | browser_auth::BrowserAuthError::Invalid(message) => unauthorized(&message),
        other => internal(other),
    }
}

fn csrf_token_matches(expected: &str, provided: Option<&str>) -> bool {
    let Some(provided) = provided.filter(|value| !value.is_empty()) else {
        return false;
    };
    let left = provided.as_bytes();
    let right = expected.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

fn browser_error_code(error: &browser_auth::BrowserAuthError) -> &str {
    match error {
        browser_auth::BrowserAuthError::Invalid(message)
        | browser_auth::BrowserAuthError::Forbidden(message)
            if safe_error_code(message) == message =>
        {
            message.as_str()
        }
        _ => "sso_failed",
    }
}

fn login_error_redirect(code: &str) -> Response {
    Redirect::to(&format!("/login?error={code}")).into_response()
}

fn safe_error_code(raw: &str) -> &str {
    if !raw.is_empty()
        && raw.len() <= 40
        && raw.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
        })
    {
        raw
    } else {
        "access_denied"
    }
}

async fn ensure_tenant(state: &EnterpriseState, tenant_id: &str) -> Result<(), ApiError> {
    validate_tenant_id(tenant_id)?;
    if !state.tenant_is_active(tenant_id).await? {
        return Err(ApiError::not_found(format!(
            "tenant `{tenant_id}` not found"
        )));
    }
    Ok(())
}

fn normalize_worker(mut value: Value) -> Value {
    normalize_timestamp_fields(&mut value, &["lastHeartbeatAt", "registeredAt"]);
    if value.get("status").and_then(Value::as_str) == Some("active")
        && let Some(object) = value.as_object_mut()
    {
        object.insert("status".to_string(), Value::String("online".to_string()));
    }
    value
}

fn normalize_backup(mut value: Value) -> Value {
    normalize_timestamp_fields(&mut value, &["createdAt"]);
    value
}

fn normalize_timestamp_fields(value: &mut Value, fields: &[&str]) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    for field in fields {
        let Some(millis) = object.get(*field).and_then(Value::as_i64) else {
            continue;
        };
        let formatted = OffsetDateTime::from_unix_timestamp_nanos(millis as i128 * 1_000_000)
            .ok()
            .and_then(|time| time.format(&Rfc3339).ok());
        if let Some(formatted) = formatted {
            object.insert(field.to_string(), Value::String(formatted));
        }
    }
}

fn ops_error(error: OpsError) -> ApiError {
    match error {
        OpsError::Db(error) => ApiError::internal(error.to_string()),
        OpsError::Json(error) => ApiError::internal(error.to_string()),
        OpsError::Io(error) => ApiError::internal(error.to_string()),
        OpsError::Invalid(message) => {
            ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "OPS_INVALID", message)
        }
        OpsError::NotFound(message) => ApiError::not_found(message),
        OpsError::Upstream(message) => {
            ApiError::new(StatusCode::BAD_GATEWAY, "OPS_UPSTREAM", message)
        }
        OpsError::Forbidden(message) => {
            ApiError::new(StatusCode::FORBIDDEN, "OPS_FORBIDDEN", message)
        }
    }
}

async fn tenant_gate(
    State(state): State<EnterpriseState>,
    AxumPath(rest): AxumPath<String>,
    request: Request<Body>,
) -> Response {
    let path = format!("/{rest}");
    if path == "/health" {
        let brand: Value = serde_json::from_str(include_str!("../../../branding/brand.json"))
            .unwrap_or(Value::Null);
        crate::metrics::metrics().inc(
            "hpa_http_requests_total",
            &[
                ("route", "/api/health"),
                ("method", "GET"),
                ("status", "200"),
            ],
        );
        return Json(json!({
            "status": "ok",
            "internalId": "hpa",
            "version": env!("CARGO_PKG_VERSION"),
            "brand": brand,
            "runtime": "rust",
            "enterprise": state.require_auth,
        }))
        .into_response();
    }
    if path == "/internal/agent-tool-calls" {
        let tenant_id = request
            .headers()
            .get("x-hpa-tenant-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
            .filter(|value| !value.is_empty() && value.len() <= 128);
        let Some(tenant_id) = tenant_id else {
            return ApiError::new(
                StatusCode::FORBIDDEN,
                "AGENT_TOOL_TENANT",
                "x-hpa-tenant-id header is required",
            )
            .into_response();
        };
        let active: Option<String> = match sqlx::query_scalar(
            "SELECT id FROM enterprise_tenants WHERE id = ? AND status = 'active'",
        )
        .bind(&tenant_id)
        .fetch_optional(&state.control)
        .await
        {
            Ok(active) => active,
            Err(error) => return internal(error).into_response(),
        };
        if active.is_none() {
            return ApiError::new(
                StatusCode::FORBIDDEN,
                "AGENT_TOOL_TENANT",
                "tenant is not active",
            )
            .into_response();
        }
        let entry = match state.tenant_entry(&tenant_id).await {
            Ok(entry) => entry,
            Err(error) => return error.into_response(),
        };
        let query = request
            .uri()
            .query()
            .map(|query| format!("?{query}"))
            .unwrap_or_default();
        let mut request = request;
        let Ok(uri) = format!("/{rest}{query}").parse::<Uri>() else {
            return ApiError::bad_request("INVALID_URI", "cannot rebuild request URI")
                .into_response();
        };
        *request.uri_mut() = uri;
        request.extensions_mut().clear();
        return match entry.router.oneshot(request).await {
            Ok(response) => response,
            Err(error) => ApiError::internal(error.to_string()).into_response(),
        };
    }
    let headers = request.headers().clone();
    let principal = match state.authenticate(&headers).await {
        Ok(Some(principal)) => principal,
        Ok(None) if !state.require_auth => Principal {
            token_id: "local".to_string(),
            tenant_id: LOCAL_TENANT.to_string(),
            role: DEFAULT_ROLE.to_string(),
            platform_admin: true,
        },
        Ok(None) => {
            return unauthorized("bearer token required").into_response();
        }
        Err(error) => return error.into_response(),
    };
    let method = request.method().as_str().to_string();
    let action = required_action(&method, &path);
    if !role_allows(&principal.role, action) && !principal.platform_admin {
        state
            .audit(
                Some(&principal.tenant_id),
                Some(&principal.token_id),
                "token",
                &format!("{method} {path}"),
                "api",
                "deny",
                Some("rbac denied"),
                json!({"role": principal.role, "action": action}),
            )
            .await;
        return ApiError::new(
            StatusCode::FORBIDDEN,
            "RBAC_DENIED",
            format!("role `{}` cannot perform `{action}`", principal.role),
        )
        .into_response();
    }
    let tenant_id = principal.tenant_id.clone();
    let entry = match state.tenant_entry(&tenant_id).await {
        Ok(entry) => entry,
        Err(error) => return error.into_response(),
    };
    let request_idempotency_key = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Err(error) = state
        .check_quota(&entry, &tenant_id, &method, &path, request_idempotency_key)
        .await
    {
        return error.into_response();
    }
    if method != "GET" {
        state
            .audit(
                Some(&tenant_id),
                Some(&principal.token_id),
                "token",
                &format!("{method} {path}"),
                "api",
                "allow",
                None,
                json!({"role": principal.role}),
            )
            .await;
    }
    let query = request
        .uri()
        .query()
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    let mut request = request;
    let Ok(uri) = format!("/{rest}{query}").parse::<Uri>() else {
        return ApiError::bad_request("INVALID_URI", "cannot rebuild request URI").into_response();
    };
    *request.uri_mut() = uri;
    request.extensions_mut().clear();
    request
        .extensions_mut()
        .insert(crate::metrics::TenantApiSurface);
    match entry.router.oneshot(request).await {
        Ok(response) => response,
        Err(error) => ApiError::internal(error.to_string()).into_response(),
    }
}

fn required_action(method: &str, path: &str) -> &'static str {
    if method == "GET" || method == "HEAD" {
        return "read";
    }
    let approval = (path.starts_with("/executions/") && path.ends_with("/approvals"))
        || (path.starts_with("/agents/approvals/") && path.ends_with("/decide"));
    if approval {
        return "approve";
    }
    let execute = path.starts_with("/workflows/") && path.ends_with("/executions")
        || path.starts_with("/executions/") && path.ends_with("/retry")
        || path.starts_with("/executions/") && path.ends_with("/cancel")
        || path.starts_with("/agents/") && path.ends_with("/run")
        || path.starts_with("/agents/sessions/")
            && (path.ends_with("/continue") || path.ends_with("/cancel"));
    if execute {
        return "execute";
    }
    "write"
}

fn is_execution_create_request(method: &str, path: &str) -> bool {
    method == "POST" && (path.ends_with("/executions") || path.ends_with("/retry"))
}

fn is_plugin_create_request(method: &str, path: &str) -> bool {
    method == "POST"
        && (path == "/plugins/install"
            || path == "/plugins/sample"
            || (path.starts_with("/marketplace/") && path.ends_with("/install")))
}

fn is_connection_create_request(method: &str, path: &str) -> bool {
    method == "POST" && matches!(path, "/connections" | "/connectors/sample")
}

fn role_allows(role: &str, action: &str) -> bool {
    match role {
        "owner" | "admin" | "developer" | "publisher" => true,
        "operator" => matches!(action, "read" | "execute"),
        "approver" => matches!(action, "read" | "approve"),
        "auditor" | "viewer" => action == "read",
        _ => false,
    }
}

fn validate_role(role: &str) -> Result<(), ApiError> {
    if matches!(
        role,
        "owner"
            | "admin"
            | "developer"
            | "publisher"
            | "operator"
            | "approver"
            | "auditor"
            | "viewer"
    ) {
        Ok(())
    } else {
        Err(ApiError::bad_request("ROLE_INVALID", "unknown role"))
    }
}

fn validate_tenant_id(tenant_id: &str) -> Result<(), ApiError> {
    if tenant_id.is_empty()
        || !tenant_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        return Err(ApiError::bad_request("TENANT_INVALID", "invalid tenant id"));
    }
    Ok(())
}

fn tenant_record_value(record: &TenantRecord) -> Value {
    json!({
        "id": record.id,
        "name": record.name,
        "status": record.status,
        "createdAt": record.created_at.format(&Rfc3339).unwrap_or_default(),
        "updatedAt": record.updated_at.format(&Rfc3339).unwrap_or_default(),
    })
}

fn default_quota() -> QuotaRecord {
    QuotaRecord {
        max_workflows: 100,
        max_executions_per_day: 1000,
        max_skills: 100,
        max_agents: 100,
        max_plugins: 50,
        max_connections: 100,
        max_concurrent_runs: 8,
    }
}

fn tenant_value(row: &sqlx::sqlite::SqliteRow) -> Value {
    json!({
        "id": row.get::<String, _>("id"),
        "name": row.get::<String, _>("name"),
        "status": row.get::<String, _>("status"),
        "createdAt": row.get::<String, _>("created_at"),
        "updatedAt": row.get::<String, _>("updated_at"),
    })
}

/// Mirrors one SQLite member row into PostgreSQL (best effort, logged on error).
async fn mirror_member_row(state: &EnterpriseState, row: &sqlx::sqlite::SqliteRow) {
    let Some(control) = state.control_store() else {
        return;
    };
    let created_at = row
        .try_get::<String, _>("created_at")
        .ok()
        .and_then(|value| OffsetDateTime::parse(&value, &Rfc3339).ok());
    let updated_at = row
        .try_get::<String, _>("updated_at")
        .ok()
        .and_then(|value| OffsetDateTime::parse(&value, &Rfc3339).ok());
    let (Some(created_at), Some(updated_at)) = (created_at, updated_at) else {
        tracing::warn!("cannot mirror member: invalid timestamps");
        return;
    };
    let member = MemberRecord {
        id: row.get("id"),
        tenant_id: row.get("tenant_id"),
        subject: row.get("subject"),
        display_name: row.get("display_name"),
        email: row.get::<Option<String>, _>("email"),
        role: row.get("role"),
        status: row.get("status"),
        created_at,
        updated_at,
    };
    if let Err(error) = control.upsert_member(&member).await {
        tracing::warn!(error = %error, member = %member.id, "cannot mirror member into PostgreSQL");
    }
}

fn parse_rfc3339(value: &str) -> Result<OffsetDateTime, ApiError> {
    OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|error| ApiError::internal(format!("invalid timestamp `{value}`: {error}")))
}

fn token_value_from_record(record: &TokenRecord) -> Value {
    let format = |value: Option<OffsetDateTime>| {
        value
            .and_then(|value| value.format(&Rfc3339).ok())
            .map(Value::String)
            .unwrap_or(Value::Null)
    };
    json!({
        "id": record.id,
        "name": record.name,
        "tenantId": record.tenant_id,
        "role": record.role,
        "platformAdmin": record.platform_admin,
        "createdAt": record
            .created_at
            .format(&Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string()),
        "lastUsedAt": format(record.last_used_at),
        "expiresAt": format(record.expires_at),
        "revokedAt": format(record.revoked_at),
    })
}

fn token_value(row: &sqlx::sqlite::SqliteRow) -> Value {
    json!({
        "id": row.get::<String, _>("id"),
        "name": row.get::<String, _>("name"),
        "tenantId": row.get::<String, _>("tenant_id"),
        "role": row.get::<String, _>("role"),
        "platformAdmin": row.get::<i64, _>("platform_admin") != 0,
        "createdAt": row.get::<String, _>("created_at"),
        "lastUsedAt": row.get::<Option<String>, _>("last_used_at"),
        "expiresAt": row.get::<Option<String>, _>("expires_at"),
        "revokedAt": row.get::<Option<String>, _>("revoked_at"),
    })
}

fn member_value(row: &sqlx::sqlite::SqliteRow) -> Value {
    json!({
        "id": row.get::<String, _>("id"),
        "tenantId": row.get::<String, _>("tenant_id"),
        "subject": row.get::<String, _>("subject"),
        "displayName": row.get::<String, _>("display_name"),
        "email": row.get::<Option<String>, _>("email"),
        "role": row.get::<String, _>("role"),
        "status": row.get::<String, _>("status"),
        "createdAt": row.get::<String, _>("created_at"),
        "updatedAt": row.get::<String, _>("updated_at"),
    })
}

fn member_record_value(member: &MemberRecord) -> Value {
    json!({
        "id": member.id,
        "tenantId": member.tenant_id,
        "subject": member.subject,
        "displayName": member.display_name,
        "email": member.email,
        "role": member.role,
        "status": member.status,
        "createdAt": member.created_at.format(&Rfc3339).unwrap_or_default(),
        "updatedAt": member.updated_at.format(&Rfc3339).unwrap_or_default(),
    })
}

fn scim_member_value(member: &ScimMemberRecord) -> Value {
    json!({
        "id": member.id,
        "tenantId": member.tenant_id,
        "subject": member.subject,
        "displayName": member.display_name,
        "email": member.email,
        "role": member.role,
        "status": member.status,
        "createdAt": member.created_at,
        "updatedAt": member.updated_at,
    })
}

fn control_error(error: ControlError) -> ApiError {
    match error {
        ControlError::Db(error) => {
            if error
                .as_database_error()
                .is_some_and(|database| database.is_unique_violation())
            {
                ApiError::conflict(
                    "MEMBER_IDENTITY_EXISTS",
                    "member subject, userName or email already exists in this tenant",
                )
            } else {
                internal(error)
            }
        }
        ControlError::Json(error) => internal(error),
        ControlError::Invalid(message) => ApiError::bad_request("MEMBER_INVALID", message),
        ControlError::NotFound(message) => ApiError::not_found(message),
    }
}

fn quota_value(tenant_id: &str, quota: &Quota) -> Value {
    json!({
        "tenantId": tenant_id,
        "maxWorkflows": quota.max_workflows,
        "maxExecutionsPerDay": quota.max_executions_per_day,
        "maxSkills": quota.max_skills,
        "maxAgents": quota.max_agents,
        "maxPlugins": quota.max_plugins,
        "maxConnections": quota.max_connections,
        "maxConcurrentRuns": quota.max_concurrent_runs,
    })
}

async fn audit_rows(
    pool: &SqlitePool,
    tenant_id: Option<&str>,
    limit: i64,
) -> Result<Vec<Value>, ApiError> {
    let limit = limit.clamp(1, 5000);
    let rows = match tenant_id {
        Some(tenant_id) => {
            sqlx::query(
                "SELECT id, at, tenant_id, actor_id, actor_type, action, resource, decision,
                        reason, detail_json FROM enterprise_audit WHERE tenant_id = ?
                 ORDER BY at DESC LIMIT ?",
            )
            .bind(tenant_id)
            .bind(limit)
            .fetch_all(pool)
            .await
        }
        None => {
            sqlx::query(
                "SELECT id, at, tenant_id, actor_id, actor_type, action, resource, decision,
                        reason, detail_json FROM enterprise_audit
                 ORDER BY at DESC LIMIT ?",
            )
            .bind(limit)
            .fetch_all(pool)
            .await
        }
    }
    .map_err(internal)?;
    Ok(rows
        .iter()
        .map(|row| {
            json!({
                "id": row.get::<String, _>("id"),
                "at": row.get::<String, _>("at"),
                "tenantId": row.get::<Option<String>, _>("tenant_id"),
                "actorId": row.get::<Option<String>, _>("actor_id"),
                "actorType": row.get::<String, _>("actor_type"),
                "action": row.get::<String, _>("action"),
                "resource": row.get::<String, _>("resource"),
                "decision": row.get::<String, _>("decision"),
                "reason": row.get::<Option<String>, _>("reason"),
                "detail": serde_json::from_str::<Value>(&row.get::<String, _>("detail_json"))
                    .unwrap_or(Value::Null),
            })
        })
        .collect())
}

async fn ensure_control_schema(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let statements = [
        "CREATE TABLE IF NOT EXISTS enterprise_tenants (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            status TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
        "CREATE TABLE IF NOT EXISTS enterprise_quotas (
            tenant_id TEXT PRIMARY KEY,
            max_workflows INTEGER NOT NULL,
            max_executions_per_day INTEGER NOT NULL,
            max_skills INTEGER NOT NULL,
            max_agents INTEGER NOT NULL,
            max_plugins INTEGER NOT NULL,
            max_connections INTEGER NOT NULL,
            max_concurrent_runs INTEGER NOT NULL
        )",
        "CREATE TABLE IF NOT EXISTS enterprise_tokens (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            token_hash TEXT NOT NULL UNIQUE,
            tenant_id TEXT NOT NULL,
            role TEXT NOT NULL,
            platform_admin INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL,
            last_used_at TEXT,
            expires_at TEXT,
            revoked_at TEXT
        )",
        "CREATE INDEX IF NOT EXISTS idx_enterprise_tokens_tenant
         ON enterprise_tokens(tenant_id, created_at DESC)",
        "CREATE TABLE IF NOT EXISTS enterprise_members (
            id TEXT PRIMARY KEY,
            tenant_id TEXT NOT NULL,
            subject TEXT NOT NULL,
            user_name TEXT,
            external_id TEXT,
            display_name TEXT NOT NULL,
            email TEXT,
            role TEXT NOT NULL,
            base_role TEXT NOT NULL DEFAULT 'viewer',
            scim_json TEXT NOT NULL DEFAULT '{}',
            status TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            UNIQUE (tenant_id, subject)
        )",
        "CREATE INDEX IF NOT EXISTS idx_enterprise_members_tenant
         ON enterprise_members(tenant_id, created_at DESC)",
        "CREATE TABLE IF NOT EXISTS enterprise_scim_groups (
            id TEXT PRIMARY KEY,
            tenant_id TEXT NOT NULL,
            display_name TEXT NOT NULL,
            external_id TEXT,
            role TEXT CHECK (role IS NULL OR role IN (
                'owner', 'admin', 'developer', 'publisher',
                'operator', 'approver', 'auditor', 'viewer'
            )),
            scim_json TEXT NOT NULL DEFAULT '{}',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_enterprise_scim_groups_name
         ON enterprise_scim_groups(tenant_id, lower(display_name))",
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_enterprise_scim_groups_external_id
         ON enterprise_scim_groups(tenant_id, external_id) WHERE external_id IS NOT NULL",
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_enterprise_members_tenant_id
         ON enterprise_members(tenant_id, id)",
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_enterprise_scim_groups_tenant_id
         ON enterprise_scim_groups(tenant_id, id)",
        "CREATE TABLE IF NOT EXISTS enterprise_scim_group_members (
            tenant_id TEXT NOT NULL,
            group_id TEXT NOT NULL REFERENCES enterprise_scim_groups(id) ON DELETE CASCADE,
            member_id TEXT NOT NULL REFERENCES enterprise_members(id) ON DELETE CASCADE,
            created_at TEXT NOT NULL,
            PRIMARY KEY (group_id, member_id)
        )",
        "CREATE INDEX IF NOT EXISTS idx_enterprise_scim_group_members_member
         ON enterprise_scim_group_members(tenant_id, member_id)",
        "CREATE TRIGGER IF NOT EXISTS enterprise_scim_group_members_tenant_insert
         BEFORE INSERT ON enterprise_scim_group_members
         WHEN NOT EXISTS (
             SELECT 1 FROM enterprise_scim_groups
             WHERE tenant_id = NEW.tenant_id AND id = NEW.group_id
         ) OR NOT EXISTS (
             SELECT 1 FROM enterprise_members
             WHERE tenant_id = NEW.tenant_id AND id = NEW.member_id
         )
         BEGIN
             SELECT RAISE(ABORT, 'scim group member tenant mismatch');
         END",
        "CREATE TRIGGER IF NOT EXISTS enterprise_scim_group_members_tenant_update
         BEFORE UPDATE ON enterprise_scim_group_members
         WHEN NOT EXISTS (
             SELECT 1 FROM enterprise_scim_groups
             WHERE tenant_id = NEW.tenant_id AND id = NEW.group_id
         ) OR NOT EXISTS (
             SELECT 1 FROM enterprise_members
             WHERE tenant_id = NEW.tenant_id AND id = NEW.member_id
         )
         BEGIN
             SELECT RAISE(ABORT, 'scim group member tenant mismatch');
         END",
        "CREATE TABLE IF NOT EXISTS enterprise_audit (
            id TEXT PRIMARY KEY,
            at TEXT NOT NULL,
            tenant_id TEXT,
            actor_id TEXT,
            actor_type TEXT NOT NULL,
            action TEXT NOT NULL,
            resource TEXT NOT NULL,
            decision TEXT NOT NULL,
            reason TEXT,
            detail_json TEXT NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS idx_enterprise_audit_at ON enterprise_audit(at DESC)",
    ];
    for statement in statements {
        sqlx::query(statement).execute(pool).await?;
    }
    ensure_control_columns(pool).await?;
    Ok(())
}

async fn ensure_control_columns(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('enterprise_members')")
            .fetch_all(pool)
            .await?;
    let backfill_base_role = !columns.iter().any(|column| column == "base_role");
    let additions = [
        (
            "user_name",
            "ALTER TABLE enterprise_members ADD COLUMN user_name TEXT",
        ),
        (
            "external_id",
            "ALTER TABLE enterprise_members ADD COLUMN external_id TEXT",
        ),
        (
            "base_role",
            "ALTER TABLE enterprise_members ADD COLUMN base_role TEXT NOT NULL DEFAULT 'viewer'",
        ),
        (
            "scim_json",
            "ALTER TABLE enterprise_members ADD COLUMN scim_json TEXT NOT NULL DEFAULT '{}'",
        ),
    ];
    for (name, statement) in additions {
        if !columns.iter().any(|column| column == name) {
            sqlx::query(statement).execute(pool).await?;
        }
    }
    if backfill_base_role {
        sqlx::query("UPDATE enterprise_members SET base_role = role")
            .execute(pool)
            .await?;
    }
    sqlx::query(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_enterprise_members_user_name
         ON enterprise_members(tenant_id, lower(user_name)) WHERE user_name IS NOT NULL",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_enterprise_members_external_id
         ON enterprise_members(tenant_id, external_id) WHERE external_id IS NOT NULL",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_enterprise_members_email
         ON enterprise_members(tenant_id, lower(email)) WHERE email IS NOT NULL",
    )
    .execute(pool)
    .await?;
    Ok(())
}

fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::internal(error.to_string())
}

fn unauthorized(message: &str) -> ApiError {
    ApiError::new(StatusCode::UNAUTHORIZED, "UNAUTHORIZED", message)
}

fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default()
}

fn today_prefix() -> String {
    now_rfc3339().chars().take(10).collect()
}

fn csv(value: &Value) -> String {
    let text = match value {
        Value::String(value) => value.clone(),
        Value::Null => return String::new(),
        other => other.to_string(),
    };
    let text = if text.starts_with(['=', '+', '-', '@']) {
        format!("'{text}")
    } else {
        text
    };
    format!("\"{}\"", text.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::{is_execution_create_request, required_action, role_allows};

    #[test]
    fn retry_requires_execute_permission() {
        let action = required_action("POST", "/executions/exec_123/retry");
        assert_eq!(action, "execute");
        assert!(role_allows("operator", action));
        assert!(!role_allows("viewer", action));
        assert!(is_execution_create_request(
            "POST",
            "/executions/exec_123/retry"
        ));
        assert!(is_execution_create_request(
            "POST",
            "/workflows/wf_123/executions"
        ));
    }

    #[test]
    fn cancel_and_approval_keep_distinct_permissions() {
        assert_eq!(
            required_action("POST", "/executions/exec_123/cancel"),
            "execute"
        );
        assert_eq!(
            required_action("POST", "/executions/exec_123/approvals"),
            "approve"
        );
    }
}
