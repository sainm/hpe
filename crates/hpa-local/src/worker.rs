use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use uuid::Uuid;

const LEASE_TTL_MS: i64 = 30_000;
const DEFAULT_CAPABILITY: &str = "workflow";
const MAX_CONCURRENCY: i64 = 64;
const JOB_SELECT: &str = "SELECT id, tenant_id, worker_id, execution_id, workflow_id,
    workflow_version, workflow_json, state_json, status, attempts, lease_id,
    lease_expires_at, runtime_version, resumed, cancel_requested, error, created_at,
    updated_at FROM enterprise_worker_jobs";

#[derive(Clone)]
pub struct WorkerService {
    control: SqlitePool,
}

#[derive(Clone, Debug)]
pub struct WorkerAuth {
    pub worker_id: String,
    pub tenant_id: String,
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct WorkerJob {
    pub id: String,
    pub tenant_id: String,
    pub worker_id: Option<String>,
    pub execution_id: String,
    pub workflow_id: String,
    pub workflow_version: String,
    pub workflow_json: String,
    pub state_json: String,
    pub status: String,
    pub attempts: i64,
    pub lease_id: Option<String>,
    pub lease_expires_at: Option<i64>,
    pub runtime_version: String,
    pub resumed: bool,
    pub cancel_requested: bool,
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl WorkerJob {
    pub fn public_json(&self) -> Value {
        json!({
            "id": self.id,
            "tenantId": self.tenant_id,
            "workerId": self.worker_id,
            "executionId": self.execution_id,
            "workflowId": self.workflow_id,
            "workflowVersion": self.workflow_version,
            "status": self.status,
            "attempts": self.attempts,
            "leaseExpiresAt": self.lease_expires_at,
            "cancelRequested": self.cancel_requested,
            "error": self.error,
            "createdAt": self.created_at,
            "updatedAt": self.updated_at
        })
    }

    pub fn lease_json(&self) -> Value {
        let workflow = serde_json::from_str(&self.workflow_json).unwrap_or(Value::Null);
        let state = serde_json::from_str(&self.state_json).unwrap_or(Value::Null);
        json!({
            "jobId": self.id,
            "executionId": self.execution_id,
            "leaseId": self.lease_id,
            "attempt": self.attempts,
            "workflowId": self.workflow_id,
            "workflowVersion": self.workflow_version,
            "resumed": self.resumed,
            "runtimeVersion": self.runtime_version,
            "leaseExpiresAt": self.lease_expires_at,
            "workflow": workflow,
            "state": state
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerErrorKind {
    Invalid,
    NotFound,
    Conflict,
    Forbidden,
    Internal,
}

#[derive(Debug, Clone)]
pub struct WorkerError {
    pub kind: WorkerErrorKind,
    pub message: String,
}

impl WorkerError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self {
            kind: WorkerErrorKind::Invalid,
            message: message.into(),
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            kind: WorkerErrorKind::NotFound,
            message: message.into(),
        }
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self {
            kind: WorkerErrorKind::Conflict,
            message: message.into(),
        }
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self {
            kind: WorkerErrorKind::Forbidden,
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            kind: WorkerErrorKind::Internal,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for WorkerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for WorkerError {}

impl From<sqlx::Error> for WorkerError {
    fn from(error: sqlx::Error) -> Self {
        WorkerError::internal(error.to_string())
    }
}

impl From<serde_json::Error> for WorkerError {
    fn from(error: serde_json::Error) -> Self {
        WorkerError::invalid(format!("invalid JSON payload: {error}"))
    }
}

impl WorkerService {
    pub fn new(control: SqlitePool) -> Self {
        Self { control }
    }

    pub async fn ensure_schema(&self) -> Result<(), WorkerError> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS enterprise_worker_tokens (
                id TEXT PRIMARY KEY,
                worker_id TEXT NOT NULL,
                tenant_id TEXT NOT NULL,
                token_hash TEXT NOT NULL UNIQUE,
                created_at INTEGER NOT NULL,
                revoked_at INTEGER
            )",
        )
        .execute(&self.control)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS enterprise_worker_jobs (
                id TEXT PRIMARY KEY,
                tenant_id TEXT NOT NULL,
                worker_id TEXT,
                execution_id TEXT NOT NULL UNIQUE,
                workflow_id TEXT NOT NULL,
                workflow_version TEXT NOT NULL,
                workflow_json TEXT NOT NULL,
                state_json TEXT NOT NULL,
                status TEXT NOT NULL,
                attempts INTEGER NOT NULL DEFAULT 0,
                lease_id TEXT,
                lease_expires_at INTEGER,
                runtime_version TEXT NOT NULL,
                required_capabilities_json TEXT NOT NULL,
                resumed INTEGER NOT NULL DEFAULT 0,
                cancel_requested INTEGER NOT NULL DEFAULT 0,
                error TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )",
        )
        .execute(&self.control)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS enterprise_worker_tokens_worker
             ON enterprise_worker_tokens (worker_id, created_at DESC)",
        )
        .execute(&self.control)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS enterprise_worker_jobs_tenant
             ON enterprise_worker_jobs (tenant_id, created_at DESC)",
        )
        .execute(&self.control)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS enterprise_worker_jobs_status
             ON enterprise_worker_jobs (tenant_id, status, created_at)",
        )
        .execute(&self.control)
        .await?;
        self.ensure_worker_columns().await?;
        Ok(())
    }

    async fn ensure_worker_columns(&self) -> Result<(), WorkerError> {
        let exists: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'enterprise_workers'",
        )
        .fetch_optional(&self.control)
        .await?;
        if exists.is_none() {
            return Ok(());
        }
        let columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('enterprise_workers')")
                .fetch_all(&self.control)
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
                sqlx::query(statement).execute(&self.control).await?;
            }
        }
        Ok(())
    }

    pub async fn mint_worker_token(
        &self,
        worker_id: &str,
    ) -> Result<(String, String), WorkerError> {
        let worker: Option<(String,)> =
            sqlx::query_as("SELECT tenant_id FROM enterprise_workers WHERE id = ?")
                .bind(worker_id)
                .fetch_optional(&self.control)
                .await?;
        let Some((tenant_id,)) = worker else {
            return Err(WorkerError::not_found(format!(
                "worker `{worker_id}` does not exist"
            )));
        };
        let mut bytes = [0u8; 32];
        getrandom::getrandom(&mut bytes)
            .map_err(|error| WorkerError::internal(format!("cannot generate token: {error}")))?;
        let token = format!("hpa_wrk_{}", URL_SAFE_NO_PAD.encode(bytes));
        let hash = hash_token(&token);
        let token_id = format!("wrktok_{}", Uuid::now_v7());
        let now = now_ms();
        let mut tx = self.control.begin().await?;
        sqlx::query(
            "UPDATE enterprise_worker_tokens SET revoked_at = ?
             WHERE worker_id = ? AND revoked_at IS NULL",
        )
        .bind(now)
        .bind(worker_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO enterprise_worker_tokens
             (id, worker_id, tenant_id, token_hash, created_at, revoked_at)
             VALUES (?, ?, ?, ?, ?, NULL)",
        )
        .bind(&token_id)
        .bind(worker_id)
        .bind(&tenant_id)
        .bind(&hash)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok((token, token_id))
    }

    pub async fn revoke_worker_tokens(&self, worker_id: &str) -> Result<(), WorkerError> {
        sqlx::query(
            "UPDATE enterprise_worker_tokens SET revoked_at = ?
             WHERE worker_id = ? AND revoked_at IS NULL",
        )
        .bind(now_ms())
        .bind(worker_id)
        .execute(&self.control)
        .await?;
        Ok(())
    }

    pub async fn authenticate_worker(
        &self,
        token: &str,
    ) -> Result<Option<WorkerAuth>, WorkerError> {
        if !token.starts_with("hpa_wrk_") || token.len() <= "hpa_wrk_".len() {
            return Ok(None);
        }
        let row: Option<(String, String, String, String, String)> = sqlx::query_as(
            "SELECT t.worker_id, t.tenant_id, w.status, w.tenant_id, tenant.status
             FROM enterprise_worker_tokens t
             JOIN enterprise_workers w
               ON w.id = t.worker_id AND w.tenant_id = t.tenant_id
             JOIN enterprise_tenants tenant ON tenant.id = t.tenant_id
             WHERE t.token_hash = ? AND t.revoked_at IS NULL",
        )
        .bind(hash_token(token))
        .fetch_optional(&self.control)
        .await?;
        let Some((worker_id, tenant_id, worker_status, worker_tenant, tenant_status)) = row else {
            return Ok(None);
        };
        if worker_status == "disabled" || worker_tenant != tenant_id || tenant_status != "active" {
            return Ok(None);
        }
        Ok(Some(WorkerAuth {
            worker_id,
            tenant_id,
        }))
    }

    #[allow(dead_code)]
    pub async fn update_worker_runtime(
        &self,
        worker_id: &str,
        version: &str,
        capabilities: &[String],
        max_concurrency: i64,
    ) -> Result<(), WorkerError> {
        let capabilities = validate_capabilities(capabilities)?;
        if !(1..=MAX_CONCURRENCY).contains(&max_concurrency) {
            return Err(WorkerError::invalid(format!(
                "max concurrency must be between 1 and {MAX_CONCURRENCY}"
            )));
        }
        let capabilities_json = serde_json::to_string(&capabilities)?;
        let result = sqlx::query(
            "UPDATE enterprise_workers
             SET runtime_version = ?, capabilities_json = ?, max_concurrency = ?,
                 last_heartbeat_at = ?,
                 status = CASE WHEN status = 'disabled' THEN 'disabled' ELSE 'active' END
             WHERE id = ?",
        )
        .bind(version)
        .bind(capabilities_json)
        .bind(max_concurrency)
        .bind(now_ms())
        .bind(worker_id)
        .execute(&self.control)
        .await?;
        if result.rows_affected() == 0 {
            return Err(WorkerError::not_found(format!(
                "worker `{worker_id}` does not exist"
            )));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn enqueue_job(
        &self,
        tenant_id: &str,
        worker_id: Option<&str>,
        execution_id: &str,
        workflow_id: &str,
        workflow_version: &str,
        workflow_json: &str,
        state_json: &str,
        runtime_version: &str,
    ) -> Result<WorkerJob, WorkerError> {
        validate_tenant(tenant_id)?;
        if execution_id.trim().is_empty() {
            return Err(WorkerError::invalid("execution id is required"));
        }
        if workflow_id.trim().is_empty() {
            return Err(WorkerError::invalid("workflow id is required"));
        }
        if workflow_version.trim().is_empty() {
            return Err(WorkerError::invalid("workflow version is required"));
        }
        if runtime_version.trim().is_empty() {
            return Err(WorkerError::invalid("runtime version is required"));
        }
        let tenant: Option<String> = sqlx::query_scalar(
            "SELECT id FROM enterprise_tenants WHERE id = ? AND status = 'active'",
        )
        .bind(tenant_id)
        .fetch_optional(&self.control)
        .await?;
        if tenant.is_none() {
            return Err(WorkerError::not_found(format!(
                "tenant `{tenant_id}` is not active"
            )));
        }
        if let Some(worker_id) = worker_id {
            let worker: Option<(String, String)> =
                sqlx::query_as("SELECT tenant_id, status FROM enterprise_workers WHERE id = ?")
                    .bind(worker_id)
                    .fetch_optional(&self.control)
                    .await?;
            let Some((worker_tenant, status)) = worker else {
                return Err(WorkerError::not_found(format!(
                    "worker `{worker_id}` does not exist"
                )));
            };
            if worker_tenant != tenant_id {
                return Err(WorkerError::not_found(format!(
                    "worker `{worker_id}` does not exist"
                )));
            }
            if status == "disabled" {
                return Err(WorkerError::invalid(format!(
                    "worker `{worker_id}` is disabled"
                )));
            }
        }
        let id = format!("job_{}", Uuid::now_v7());
        let now = now_ms();
        let required_capabilities = json!([DEFAULT_CAPABILITY]).to_string();
        let insert = sqlx::query(
            "INSERT INTO enterprise_worker_jobs
             (id, tenant_id, worker_id, execution_id, workflow_id, workflow_version,
              workflow_json, state_json, status, attempts, lease_id, lease_expires_at,
              runtime_version, required_capabilities_json, resumed, cancel_requested, error,
              created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'queued', 0, NULL, NULL, ?, ?, 0, 0, NULL, ?, ?)",
        )
        .bind(&id)
        .bind(tenant_id)
        .bind(worker_id)
        .bind(execution_id)
        .bind(workflow_id)
        .bind(workflow_version)
        .bind(workflow_json)
        .bind(state_json)
        .bind(runtime_version)
        .bind(&required_capabilities)
        .bind(now)
        .bind(now)
        .execute(&self.control)
        .await;
        if let Err(error) = insert {
            if error
                .as_database_error()
                .is_some_and(|database| database.is_unique_violation())
            {
                return Err(WorkerError::conflict(format!(
                    "execution `{execution_id}` is already enqueued"
                )));
            }
            return Err(WorkerError::internal(error.to_string()));
        }
        self.fetch_job(&id, tenant_id)
            .await?
            .ok_or_else(|| WorkerError::internal("enqueued job disappeared"))
    }

    pub async fn lease_jobs(
        &self,
        auth: &WorkerAuth,
        slots: i64,
        version: &str,
        capabilities: &[String],
    ) -> Result<(Vec<WorkerJob>, Vec<String>), WorkerError> {
        let capabilities = validate_capabilities(capabilities)?;
        let requested: HashSet<String> = capabilities.iter().cloned().collect();
        let now = now_ms();
        let mut tx = self.control.begin().await?;
        expire_leases_with(&mut tx, now).await?;
        let worker: Option<(String, String, i64, String)> = sqlx::query_as(
            "SELECT tenant_id, status, max_concurrency, capabilities_json
             FROM enterprise_workers WHERE id = ?",
        )
        .bind(&auth.worker_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((worker_tenant, worker_status, max_concurrency, registered_json)) = worker else {
            return Err(WorkerError::forbidden("worker is not registered"));
        };
        if worker_tenant != auth.tenant_id || worker_status == "disabled" {
            return Err(WorkerError::forbidden("worker is not available"));
        }
        let capabilities_json = serde_json::to_string(&capabilities)?;
        sqlx::query(
            "UPDATE enterprise_workers
             SET runtime_version = ?, capabilities_json = ?, last_heartbeat_at = ?,
                 status = 'active'
             WHERE id = ? AND tenant_id = ?",
        )
        .bind(version)
        .bind(&capabilities_json)
        .bind(now)
        .bind(&auth.worker_id)
        .bind(&auth.tenant_id)
        .execute(&mut *tx)
        .await?;
        let registered: HashSet<String> = serde_json::from_str::<Vec<String>>(&registered_json)
            .unwrap_or_default()
            .into_iter()
            .collect();
        let active: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM enterprise_worker_jobs
             WHERE tenant_id = ? AND worker_id = ? AND status IN ('leased', 'running')",
        )
        .bind(&auth.tenant_id)
        .bind(&auth.worker_id)
        .fetch_one(&mut *tx)
        .await?;
        let available = max_concurrency
            .clamp(1, MAX_CONCURRENCY)
            .saturating_sub(active)
            .min(slots.max(0));
        let mut jobs = Vec::new();
        if available > 0 {
            let query = format!(
                "{JOB_SELECT} WHERE tenant_id = ? AND status = 'queued'
                 AND (worker_id IS NULL OR worker_id = ?) AND runtime_version = ?
                 ORDER BY created_at ASC, id ASC LIMIT 256"
            );
            let candidates: Vec<(String, String)> = sqlx::query_as(sqlx::AssertSqlSafe(query))
                .bind(&auth.tenant_id)
                .bind(&auth.worker_id)
                .bind(version)
                .fetch_all(&mut *tx)
                .await?;
            for (job_id, required_json) in candidates {
                if jobs.len() as i64 >= available {
                    break;
                }
                let required = required_capabilities(&required_json);
                if required.iter().any(|capability| {
                    !requested.contains(capability) || !registered.contains(capability)
                }) {
                    continue;
                }
                let lease_id = format!("lease_{}", Uuid::now_v7());
                let expires_at = now + LEASE_TTL_MS;
                let result = sqlx::query(
                    "UPDATE enterprise_worker_jobs
                     SET status = 'leased', worker_id = ?, attempts = attempts + 1,
                         lease_id = ?, lease_expires_at = ?, updated_at = ?
                     WHERE id = ? AND tenant_id = ? AND status = 'queued'",
                )
                .bind(&auth.worker_id)
                .bind(&lease_id)
                .bind(expires_at)
                .bind(now)
                .bind(&job_id)
                .bind(&auth.tenant_id)
                .execute(&mut *tx)
                .await?;
                if result.rows_affected() == 0 {
                    continue;
                }
                let job = fetch_job_with(&mut tx, &job_id, &auth.tenant_id)
                    .await?
                    .ok_or_else(|| WorkerError::internal("leased job disappeared"))?;
                jobs.push(job);
            }
        }
        let cancels: Vec<(String,)> = sqlx::query_as(
            "SELECT id FROM enterprise_worker_jobs
             WHERE tenant_id = ? AND worker_id = ? AND cancel_requested = 1
               AND status IN ('leased', 'running', 'waiting_approval')
             ORDER BY created_at ASC",
        )
        .bind(&auth.tenant_id)
        .bind(&auth.worker_id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok((jobs, cancels.into_iter().map(|(id,)| id).collect()))
    }

    pub async fn heartbeat(
        &self,
        auth: &WorkerAuth,
        version: &str,
        capabilities: &[String],
        max_concurrency: i64,
        active_jobs: &[String],
    ) -> Result<Vec<String>, WorkerError> {
        let capabilities = validate_capabilities(capabilities)?;
        if !(1..=MAX_CONCURRENCY).contains(&max_concurrency) {
            return Err(WorkerError::invalid(format!(
                "max concurrency must be between 1 and {MAX_CONCURRENCY}"
            )));
        }
        let now = now_ms();
        let mut tx = self.control.begin().await?;
        let worker: Option<(String, String)> =
            sqlx::query_as("SELECT tenant_id, status FROM enterprise_workers WHERE id = ?")
                .bind(&auth.worker_id)
                .fetch_optional(&mut *tx)
                .await?;
        let Some((worker_tenant, worker_status)) = worker else {
            return Err(WorkerError::forbidden("worker is not registered"));
        };
        if worker_tenant != auth.tenant_id || worker_status == "disabled" {
            return Err(WorkerError::forbidden("worker is not available"));
        }
        let capabilities_json = serde_json::to_string(&capabilities)?;
        sqlx::query(
            "UPDATE enterprise_workers
             SET runtime_version = ?, capabilities_json = ?, max_concurrency = ?,
                 last_heartbeat_at = ?, status = 'active'
             WHERE id = ? AND tenant_id = ?",
        )
        .bind(version)
        .bind(capabilities_json)
        .bind(max_concurrency)
        .bind(now)
        .bind(&auth.worker_id)
        .bind(&auth.tenant_id)
        .execute(&mut *tx)
        .await?;
        for job_id in active_jobs {
            sqlx::query(
                "UPDATE enterprise_worker_jobs
                 SET lease_expires_at = ?, updated_at = ?
                 WHERE id = ? AND tenant_id = ? AND worker_id = ?
                   AND status IN ('leased', 'running')",
            )
            .bind(now + LEASE_TTL_MS)
            .bind(now)
            .bind(job_id)
            .bind(&auth.tenant_id)
            .bind(&auth.worker_id)
            .execute(&mut *tx)
            .await?;
        }
        let cancels: Vec<(String,)> = sqlx::query_as(
            "SELECT id FROM enterprise_worker_jobs
             WHERE tenant_id = ? AND worker_id = ? AND cancel_requested = 1
               AND status IN ('leased', 'running', 'waiting_approval')
             ORDER BY created_at ASC",
        )
        .bind(&auth.tenant_id)
        .bind(&auth.worker_id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(cancels.into_iter().map(|(id,)| id).collect())
    }

    pub async fn start_job(
        &self,
        auth: &WorkerAuth,
        job_id: &str,
        lease_id: &str,
    ) -> Result<WorkerJob, WorkerError> {
        let job = self
            .fetch_job(job_id, &auth.tenant_id)
            .await?
            .ok_or_else(|| WorkerError::not_found(format!("job `{job_id}` does not exist")))?;
        if job.worker_id.as_deref() != Some(auth.worker_id.as_str()) {
            return Err(WorkerError::conflict("job is not leased to this worker"));
        }
        if job.lease_id.as_deref() != Some(lease_id) {
            return Err(WorkerError::conflict("lease is stale"));
        }
        if !matches!(job.status.as_str(), "leased" | "running") {
            return Err(WorkerError::conflict(format!(
                "job cannot start from status `{}`",
                job.status
            )));
        }
        let result = sqlx::query(
            "UPDATE enterprise_worker_jobs
             SET status = 'running', error = NULL, updated_at = ?
             WHERE id = ? AND tenant_id = ? AND lease_id = ?
               AND status IN ('leased', 'running')",
        )
        .bind(now_ms())
        .bind(job_id)
        .bind(&auth.tenant_id)
        .bind(lease_id)
        .execute(&self.control)
        .await?;
        if result.rows_affected() == 0 {
            return Err(WorkerError::conflict("lease is stale"));
        }
        self.fetch_job(job_id, &auth.tenant_id)
            .await?
            .ok_or_else(|| WorkerError::not_found(format!("job `{job_id}` does not exist")))
    }

    pub async fn report_job(
        &self,
        auth: &WorkerAuth,
        job_id: &str,
        lease_id: &str,
        status: &str,
        state_json: &str,
    ) -> Result<WorkerJob, WorkerError> {
        if !matches!(status, "running" | "waiting_approval") {
            return Err(WorkerError::invalid(format!(
                "status `{status}` is not accepted"
            )));
        }
        let job = self
            .fetch_job(job_id, &auth.tenant_id)
            .await?
            .ok_or_else(|| WorkerError::not_found(format!("job `{job_id}` does not exist")))?;
        if job.worker_id.as_deref() != Some(auth.worker_id.as_str()) {
            return Err(WorkerError::conflict("job is not leased to this worker"));
        }
        if job.lease_id.as_deref() != Some(lease_id) {
            return Err(WorkerError::conflict("lease is stale"));
        }
        if !matches!(job.status.as_str(), "leased" | "running") {
            return Err(WorkerError::conflict(format!(
                "job cannot report from status `{}`",
                job.status
            )));
        }
        validate_state(&job, state_json)?;
        let now = now_ms();
        let result = if status == "waiting_approval" {
            sqlx::query(
                "UPDATE enterprise_worker_jobs
                 SET status = 'waiting_approval', state_json = ?, lease_id = NULL,
                     lease_expires_at = NULL, updated_at = ?
                 WHERE id = ? AND tenant_id = ? AND lease_id = ?
                   AND status IN ('leased', 'running')",
            )
            .bind(state_json)
            .bind(now)
            .bind(job_id)
            .bind(&auth.tenant_id)
            .bind(lease_id)
            .execute(&self.control)
            .await?
        } else {
            sqlx::query(
                "UPDATE enterprise_worker_jobs
                 SET status = 'running', state_json = ?, updated_at = ?
                 WHERE id = ? AND tenant_id = ? AND lease_id = ?
                   AND status IN ('leased', 'running')",
            )
            .bind(state_json)
            .bind(now)
            .bind(job_id)
            .bind(&auth.tenant_id)
            .bind(lease_id)
            .execute(&self.control)
            .await?
        };
        if result.rows_affected() == 0 {
            return Err(WorkerError::conflict("lease is stale"));
        }
        self.fetch_job(job_id, &auth.tenant_id)
            .await?
            .ok_or_else(|| WorkerError::not_found(format!("job `{job_id}` does not exist")))
    }

    pub async fn complete_job(
        &self,
        auth: &WorkerAuth,
        job_id: &str,
        lease_id: &str,
        status: &str,
        state_json: &str,
        error: Option<&str>,
    ) -> Result<WorkerJob, WorkerError> {
        if !matches!(
            status,
            "succeeded" | "failed" | "cancelled" | "waiting_approval"
        ) {
            return Err(WorkerError::invalid(format!(
                "status `{status}` is not accepted"
            )));
        }
        let job = self
            .fetch_job(job_id, &auth.tenant_id)
            .await?
            .ok_or_else(|| WorkerError::not_found(format!("job `{job_id}` does not exist")))?;
        if job.worker_id.as_deref() != Some(auth.worker_id.as_str()) {
            return Err(WorkerError::conflict("job is not leased to this worker"));
        }
        if is_terminal(&job.status) {
            if job.status == status {
                return Ok(job);
            }
            return Err(WorkerError::conflict(format!(
                "job is already `{}`",
                job.status
            )));
        }
        if job.status == "waiting_approval" && status == "waiting_approval" {
            return Ok(job);
        }
        validate_state(&job, state_json)?;
        if !matches!(
            job.status.as_str(),
            "leased" | "running" | "waiting_approval"
        ) {
            return Err(WorkerError::conflict(format!(
                "job cannot complete from status `{}`",
                job.status
            )));
        }
        if job.status != "waiting_approval" && job.lease_id.as_deref() != Some(lease_id) {
            return Err(WorkerError::conflict("lease is stale"));
        }
        let now = now_ms();
        let result = if status == "waiting_approval" {
            sqlx::query(
                "UPDATE enterprise_worker_jobs
                 SET status = 'waiting_approval', state_json = ?, lease_id = NULL,
                     lease_expires_at = NULL, error = NULL, updated_at = ?
                 WHERE id = ? AND tenant_id = ? AND worker_id = ?",
            )
            .bind(state_json)
            .bind(now)
            .bind(job_id)
            .bind(&auth.tenant_id)
            .bind(&auth.worker_id)
            .execute(&self.control)
            .await?
        } else {
            sqlx::query(
                "UPDATE enterprise_worker_jobs
                 SET status = ?, state_json = ?, lease_id = NULL, lease_expires_at = NULL,
                     error = ?, updated_at = ?
                 WHERE id = ? AND tenant_id = ? AND worker_id = ?",
            )
            .bind(status)
            .bind(state_json)
            .bind(error)
            .bind(now)
            .bind(job_id)
            .bind(&auth.tenant_id)
            .bind(&auth.worker_id)
            .execute(&self.control)
            .await?
        };
        if result.rows_affected() == 0 {
            return Err(WorkerError::conflict("lease is stale"));
        }
        self.fetch_job(job_id, &auth.tenant_id)
            .await?
            .ok_or_else(|| WorkerError::not_found(format!("job `{job_id}` does not exist")))
    }

    pub async fn cancel_job(
        &self,
        tenant_id: &str,
        job_id: &str,
    ) -> Result<WorkerJob, WorkerError> {
        let job = self
            .fetch_job(job_id, tenant_id)
            .await?
            .ok_or_else(|| WorkerError::not_found(format!("job `{job_id}` does not exist")))?;
        if is_terminal(&job.status) {
            return Err(WorkerError::conflict(format!(
                "job is already `{}`",
                job.status
            )));
        }
        let now = now_ms();
        let result = if job.status == "running" {
            sqlx::query(
                "UPDATE enterprise_worker_jobs
                 SET cancel_requested = 1, updated_at = ?
                 WHERE id = ? AND tenant_id = ? AND status = 'running'",
            )
            .bind(now)
            .bind(job_id)
            .bind(tenant_id)
            .execute(&self.control)
            .await?
        } else {
            sqlx::query(
                "UPDATE enterprise_worker_jobs
                 SET status = 'cancelled', cancel_requested = 1, lease_id = NULL,
                     lease_expires_at = NULL, updated_at = ?
                 WHERE id = ? AND tenant_id = ?
                   AND status IN ('queued', 'leased', 'waiting_approval')",
            )
            .bind(now)
            .bind(job_id)
            .bind(tenant_id)
            .execute(&self.control)
            .await?
        };
        if result.rows_affected() == 0 {
            return Err(WorkerError::conflict("job status changed"));
        }
        self.fetch_job(job_id, tenant_id)
            .await?
            .ok_or_else(|| WorkerError::not_found(format!("job `{job_id}` does not exist")))
    }

    pub async fn job_for_execution(
        &self,
        execution_id: &str,
    ) -> Result<Option<WorkerJob>, WorkerError> {
        let query = format!("{JOB_SELECT} WHERE execution_id = ?");
        Ok(sqlx::query_as::<_, WorkerJob>(sqlx::AssertSqlSafe(query))
            .bind(execution_id)
            .fetch_optional(&self.control)
            .await?)
    }

    pub async fn requeue_execution(
        &self,
        execution_id: &str,
        state_json: &str,
    ) -> Result<Option<WorkerJob>, WorkerError> {
        let Some(job) = self.job_for_execution(execution_id).await? else {
            return Ok(None);
        };
        if is_terminal(&job.status) {
            return Err(WorkerError::conflict(format!(
                "job is already `{}`",
                job.status
            )));
        }
        if !matches!(job.status.as_str(), "waiting_approval" | "running") {
            return Err(WorkerError::conflict(format!(
                "job cannot be requeued from status `{}`",
                job.status
            )));
        }
        let result = sqlx::query(
            "UPDATE enterprise_worker_jobs
             SET status = 'queued', state_json = ?, resumed = 1, worker_id = NULL,
                 lease_id = NULL, lease_expires_at = NULL, cancel_requested = 0,
                 error = NULL, updated_at = ?
             WHERE id = ? AND tenant_id = ?
               AND status IN ('waiting_approval', 'running')",
        )
        .bind(state_json)
        .bind(now_ms())
        .bind(&job.id)
        .bind(&job.tenant_id)
        .execute(&self.control)
        .await?;
        if result.rows_affected() == 0 {
            return Err(WorkerError::conflict("job status changed"));
        }
        self.fetch_job(&job.id, &job.tenant_id).await
    }

    pub async fn cancel_execution_job(
        &self,
        execution_id: &str,
    ) -> Result<Option<WorkerJob>, WorkerError> {
        let Some(job) = self.job_for_execution(execution_id).await? else {
            return Ok(None);
        };
        Ok(Some(self.cancel_job(&job.tenant_id, &job.id).await?))
    }

    pub async fn list_jobs(
        &self,
        tenant_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<WorkerJob>, WorkerError> {
        let limit = limit.clamp(1, 200);
        let jobs = match tenant_id {
            Some(tenant_id) => {
                validate_tenant(tenant_id)?;
                let query = format!(
                    "{JOB_SELECT} WHERE tenant_id = ?
                     ORDER BY created_at DESC, id DESC LIMIT ?"
                );
                sqlx::query_as::<_, WorkerJob>(sqlx::AssertSqlSafe(query))
                    .bind(tenant_id)
                    .bind(limit)
                    .fetch_all(&self.control)
                    .await?
            }
            None => {
                let query = format!("{JOB_SELECT} ORDER BY created_at DESC, id DESC LIMIT ?");
                sqlx::query_as::<_, WorkerJob>(sqlx::AssertSqlSafe(query))
                    .bind(limit)
                    .fetch_all(&self.control)
                    .await?
            }
        };
        Ok(jobs)
    }

    pub async fn get_job(
        &self,
        tenant_id: &str,
        job_id: &str,
    ) -> Result<Option<WorkerJob>, WorkerError> {
        self.fetch_job(job_id, tenant_id).await
    }

    pub async fn expire_leases(&self) -> Result<usize, WorkerError> {
        let mut tx = self.control.begin().await?;
        let expired = expire_leases_with(&mut tx, now_ms()).await?;
        tx.commit().await?;
        Ok(expired)
    }

    async fn fetch_job(
        &self,
        job_id: &str,
        tenant_id: &str,
    ) -> Result<Option<WorkerJob>, WorkerError> {
        let query = format!("{JOB_SELECT} WHERE id = ? AND tenant_id = ?");
        Ok(sqlx::query_as::<_, WorkerJob>(sqlx::AssertSqlSafe(query))
            .bind(job_id)
            .bind(tenant_id)
            .fetch_optional(&self.control)
            .await?)
    }
}

async fn expire_leases_with(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    now: i64,
) -> Result<usize, WorkerError> {
    let expired = sqlx::query(
        "UPDATE enterprise_worker_jobs
         SET status = 'queued', worker_id = NULL, lease_id = NULL, lease_expires_at = NULL,
             updated_at = ?
         WHERE status IN ('leased', 'running') AND lease_expires_at IS NOT NULL
           AND lease_expires_at < ? AND cancel_requested = 0",
    )
    .bind(now)
    .bind(now)
    .execute(&mut **tx)
    .await?
    .rows_affected() as usize;
    let cancelled = sqlx::query(
        "UPDATE enterprise_worker_jobs
         SET status = 'cancelled', lease_id = NULL, lease_expires_at = NULL, updated_at = ?
         WHERE status IN ('leased', 'running') AND lease_expires_at IS NOT NULL
           AND lease_expires_at < ? AND cancel_requested = 1",
    )
    .bind(now)
    .bind(now)
    .execute(&mut **tx)
    .await?
    .rows_affected() as usize;
    Ok(expired + cancelled)
}

async fn fetch_job_with(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    job_id: &str,
    tenant_id: &str,
) -> Result<Option<WorkerJob>, WorkerError> {
    let query = format!("{JOB_SELECT} WHERE id = ? AND tenant_id = ?");
    Ok(sqlx::query_as::<_, WorkerJob>(sqlx::AssertSqlSafe(query))
        .bind(job_id)
        .bind(tenant_id)
        .fetch_optional(&mut **tx)
        .await?)
}

fn validate_state(job: &WorkerJob, state_json: &str) -> Result<(), WorkerError> {
    let state: Value = serde_json::from_str(state_json)
        .map_err(|_| WorkerError::invalid("state must be valid JSON"))?;
    let object = state
        .as_object()
        .ok_or_else(|| WorkerError::invalid("state must be a JSON object"))?;
    if object.get("id").and_then(Value::as_str) != Some(job.execution_id.as_str()) {
        return Err(WorkerError::invalid(
            "state id must match the job execution id",
        ));
    }
    Ok(())
}

fn is_terminal(status: &str) -> bool {
    matches!(status, "succeeded" | "failed" | "cancelled")
}

fn required_capabilities(raw: &str) -> Vec<String> {
    let capabilities = serde_json::from_str::<Vec<String>>(raw).unwrap_or_default();
    if capabilities.is_empty() {
        vec![DEFAULT_CAPABILITY.to_string()]
    } else {
        capabilities
    }
}

fn validate_capabilities(capabilities: &[String]) -> Result<Vec<String>, WorkerError> {
    let mut normalized = Vec::with_capacity(capabilities.len());
    for capability in capabilities {
        let capability = capability.trim();
        if capability.is_empty() {
            return Err(WorkerError::invalid(
                "capabilities must contain non-empty strings",
            ));
        }
        normalized.push(capability.to_string());
    }
    Ok(normalized)
}

fn validate_tenant(tenant_id: &str) -> Result<(), WorkerError> {
    if tenant_id.is_empty()
        || !tenant_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err(WorkerError::invalid("tenant id must match [A-Za-z0-9_-]+"));
    }
    Ok(())
}

fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enterprise_ops::OpsService;
    use serde_json::json;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::path::PathBuf;

    struct Harness {
        service: WorkerService,
        ops: OpsService,
        pool: SqlitePool,
        root: PathBuf,
    }

    impl Harness {
        async fn new() -> Self {
            let root = std::env::temp_dir().join(format!("hpa-worker-test-{}", Uuid::now_v7()));
            std::fs::create_dir_all(&root).unwrap();
            let options = SqliteConnectOptions::new()
                .filename(root.join("control.db"))
                .create_if_missing(true);
            let pool = SqlitePoolOptions::new()
                .max_connections(4)
                .connect_with(options)
                .await
                .unwrap();
            let ops = OpsService::new(pool.clone(), root.join("data"));
            ops.ensure_schema().await.unwrap();
            let service = WorkerService::new(pool.clone());
            service.ensure_schema().await.unwrap();
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS enterprise_tenants (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    status TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                )",
            )
            .execute(&pool)
            .await
            .unwrap();
            for tenant in ["acme", "beta"] {
                sqlx::query(
                    "INSERT INTO enterprise_tenants
                     (id, name, status, created_at, updated_at)
                     VALUES (?, ?, 'active', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                )
                .bind(tenant)
                .bind(tenant)
                .execute(&pool)
                .await
                .unwrap();
            }
            Self {
                service,
                ops,
                pool,
                root,
            }
        }

        async fn worker(
            &self,
            tenant: &str,
            name: &str,
            capabilities: &[&str],
            max_concurrency: i64,
        ) -> String {
            let capabilities = capabilities
                .iter()
                .map(|capability| capability.to_string())
                .collect();
            let worker = self
                .ops
                .register_worker_config(
                    tenant,
                    name,
                    "runner",
                    json!({}),
                    capabilities,
                    max_concurrency,
                )
                .await
                .unwrap();
            worker["id"].as_str().unwrap().to_string()
        }

        async fn auth(&self, worker_id: &str) -> (String, WorkerAuth) {
            let (token, _) = self.service.mint_worker_token(worker_id).await.unwrap();
            let auth = self
                .service
                .authenticate_worker(&token)
                .await
                .unwrap()
                .unwrap();
            (token, auth)
        }

        async fn enqueue(&self, tenant: &str, worker: Option<&str>, execution: &str) -> WorkerJob {
            self.enqueue_version(tenant, worker, execution, "0.1.0")
                .await
        }

        async fn enqueue_version(
            &self,
            tenant: &str,
            worker: Option<&str>,
            execution: &str,
            version: &str,
        ) -> WorkerJob {
            self.service
                .enqueue_job(
                    tenant,
                    worker,
                    execution,
                    "wf_demo",
                    "1.0.0",
                    &json!({"id": "wf_demo", "nodes": []}).to_string(),
                    &state_json(execution),
                    version,
                )
                .await
                .unwrap()
        }

        async fn cleanup(self) {
            self.pool.close().await;
            std::fs::remove_dir_all(&self.root).ok();
        }
    }

    fn state_json(execution_id: &str) -> String {
        json!({"id": execution_id, "status": "running"}).to_string()
    }

    fn state_for(execution_id: &str, marker: &str) -> String {
        json!({"id": execution_id, "marker": marker}).to_string()
    }

    async fn set_lease_expiry(pool: &SqlitePool, job_id: &str, at: i64) {
        sqlx::query("UPDATE enterprise_worker_jobs SET lease_expires_at = ? WHERE id = ?")
            .bind(at)
            .bind(job_id)
            .execute(pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn tokens_rotate_revoke_and_reject_disabled_workers() {
        let harness = Harness::new().await;
        let worker_id = harness.worker("acme", "alpha", &["workflow"], 1).await;
        let (token, token_id) = harness.service.mint_worker_token(&worker_id).await.unwrap();
        assert!(token.starts_with("hpa_wrk_"));
        assert!(token_id.starts_with("wrktok_"));
        let hash: String =
            sqlx::query_scalar("SELECT token_hash FROM enterprise_worker_tokens WHERE id = ?")
                .bind(&token_id)
                .fetch_one(&harness.pool)
                .await
                .unwrap();
        assert_eq!(hash.len(), 64);
        assert_ne!(hash, token);
        let auth = harness
            .service
            .authenticate_worker(&token)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(auth.worker_id, worker_id);
        assert_eq!(auth.tenant_id, "acme");
        assert!(
            harness
                .service
                .authenticate_worker("hpa_wrk_unknown")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            harness
                .service
                .authenticate_worker("not-a-worker-token")
                .await
                .unwrap()
                .is_none()
        );

        let (rotated, rotated_id) = harness.service.mint_worker_token(&worker_id).await.unwrap();
        assert_ne!(rotated, token);
        assert_ne!(rotated_id, token_id);
        assert!(
            harness
                .service
                .authenticate_worker(&token)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            harness
                .service
                .authenticate_worker(&rotated)
                .await
                .unwrap()
                .is_some()
        );

        harness
            .service
            .revoke_worker_tokens(&worker_id)
            .await
            .unwrap();
        assert!(
            harness
                .service
                .authenticate_worker(&rotated)
                .await
                .unwrap()
                .is_none()
        );

        let (disabled_token, _) = harness.service.mint_worker_token(&worker_id).await.unwrap();
        harness.ops.disable_worker(&worker_id).await.unwrap();
        assert!(
            harness
                .service
                .authenticate_worker(&disabled_token)
                .await
                .unwrap()
                .is_none()
        );

        let beta_worker = harness
            .worker("beta", "beta-worker", &["workflow"], 1)
            .await;
        let (_, beta_auth) = harness.auth(&beta_worker).await;
        assert_eq!(beta_auth.tenant_id, "beta");
        harness.cleanup().await;
    }

    #[tokio::test]
    async fn lease_start_report_complete_and_list() {
        let harness = Harness::new().await;
        let worker_a = harness.worker("acme", "alpha", &["workflow"], 2).await;
        let worker_b = harness.worker("acme", "bravo", &["workflow"], 1).await;
        let worker_c = harness.worker("acme", "charlie", &["custom"], 1).await;
        let beta_worker = harness
            .worker("beta", "beta-worker", &["workflow"], 1)
            .await;
        let (_, auth_a) = harness.auth(&worker_a).await;
        let (_, auth_c) = harness.auth(&worker_c).await;
        let (_, auth_beta) = harness.auth(&beta_worker).await;

        let assigned = harness
            .enqueue("acme", Some(&worker_a), "exec-assigned")
            .await;
        let flexible = harness.enqueue("acme", None, "exec-flexible").await;
        harness
            .enqueue("acme", Some(&worker_b), "exec-other-worker")
            .await;
        harness
            .enqueue_version("acme", None, "exec-other-version", "9.9.9")
            .await;
        assert_eq!(
            harness
                .service
                .enqueue_job(
                    "acme",
                    None,
                    "exec-assigned",
                    "wf_demo",
                    "1.0.0",
                    &json!({"id": "wf_demo"}).to_string(),
                    &state_json("exec-assigned"),
                    "0.1.0",
                )
                .await
                .unwrap_err()
                .kind,
            WorkerErrorKind::Conflict
        );

        assert!(
            harness
                .service
                .lease_jobs(&auth_c, 1, "0.1.0", &["custom".to_string()])
                .await
                .unwrap()
                .0
                .is_empty()
        );
        let (jobs, cancels) = harness
            .service
            .lease_jobs(&auth_a, 5, "0.1.0", &["workflow".to_string()])
            .await
            .unwrap();
        assert!(cancels.is_empty());
        assert_eq!(jobs.len(), 2);
        let mut ids = jobs.iter().map(|job| job.id.as_str()).collect::<Vec<_>>();
        ids.sort_unstable();
        let mut expected = vec![assigned.id.as_str(), flexible.id.as_str()];
        expected.sort_unstable();
        assert_eq!(ids, expected);
        for job in &jobs {
            assert_eq!(job.status, "leased");
            assert_eq!(job.worker_id.as_deref(), Some(worker_a.as_str()));
            assert_eq!(job.attempts, 1);
            assert!(job.lease_id.as_deref().unwrap().starts_with("lease_"));
            assert!(job.lease_expires_at.unwrap() > now_ms());
            let lease = job.lease_json();
            assert_eq!(lease["jobId"], job.id);
            assert_eq!(lease["attempt"], 1);
            assert_eq!(lease["workflow"]["id"], "wf_demo");
            assert_eq!(lease["state"]["id"], job.execution_id);
        }
        let (none, _) = harness
            .service
            .lease_jobs(&auth_a, 5, "0.1.0", &["workflow".to_string()])
            .await
            .unwrap();
        assert!(none.is_empty());
        let (cross_tenant, _) = harness
            .service
            .lease_jobs(&auth_beta, 5, "0.1.0", &["workflow".to_string()])
            .await
            .unwrap();
        assert!(cross_tenant.is_empty());
        harness.enqueue("beta", None, "exec-beta").await;

        let job_id = assigned.id.clone();
        let lease_id = jobs
            .iter()
            .find(|job| job.id == job_id)
            .unwrap()
            .lease_id
            .clone()
            .unwrap();
        let started = harness
            .service
            .start_job(&auth_a, &job_id, &lease_id)
            .await
            .unwrap();
        assert_eq!(started.status, "running");
        let reported = harness
            .service
            .report_job(
                &auth_a,
                &job_id,
                &lease_id,
                "running",
                &state_for("exec-assigned", "one"),
            )
            .await
            .unwrap();
        assert_eq!(reported.status, "running");
        assert!(reported.state_json.contains("one"));
        let completed = harness
            .service
            .complete_job(
                &auth_a,
                &job_id,
                &lease_id,
                "succeeded",
                &state_for("exec-assigned", "done"),
                None,
            )
            .await
            .unwrap();
        assert_eq!(completed.status, "succeeded");
        assert!(completed.lease_id.is_none());
        assert!(completed.lease_expires_at.is_none());
        let repeated = harness
            .service
            .complete_job(
                &auth_a,
                &job_id,
                "stale-lease",
                "succeeded",
                &state_for("exec-assigned", "done"),
                None,
            )
            .await
            .unwrap();
        assert_eq!(repeated.status, "succeeded");
        assert_eq!(
            harness
                .service
                .complete_job(
                    &auth_a,
                    &job_id,
                    &lease_id,
                    "failed",
                    &state_for("exec-assigned", "done"),
                    Some("late"),
                )
                .await
                .unwrap_err()
                .kind,
            WorkerErrorKind::Conflict
        );
        assert_eq!(
            harness
                .service
                .report_job(
                    &auth_a,
                    &job_id,
                    &lease_id,
                    "running",
                    &state_json("exec-assigned")
                )
                .await
                .unwrap_err()
                .kind,
            WorkerErrorKind::Conflict
        );

        let listed = harness.service.list_jobs(Some("acme"), 200).await.unwrap();
        assert_eq!(listed.len(), 4);
        assert!(listed.iter().all(|job| job.tenant_id == "acme"));
        let beta = harness.service.list_jobs(Some("beta"), 200).await.unwrap();
        assert_eq!(beta.len(), 1);
        assert_eq!(beta[0].execution_id, "exec-beta");
        assert!(
            harness
                .service
                .get_job("beta", &job_id)
                .await
                .unwrap()
                .is_none()
        );
        let stored = harness
            .service
            .get_job("acme", &job_id)
            .await
            .unwrap()
            .unwrap();
        let public = stored.public_json();
        assert_eq!(public["id"], job_id);
        assert_eq!(public["tenantId"], "acme");
        assert_eq!(public["executionId"], "exec-assigned");
        assert_eq!(public["status"], "succeeded");
        assert_eq!(public["attempts"], 1);
        assert_eq!(public["cancelRequested"], false);
        assert_eq!(public["workerId"], worker_a);
        assert!(public.get("workflow").is_none());
        assert!(public.get("state").is_none());
        harness.cleanup().await;
    }

    #[tokio::test]
    async fn expired_leases_are_reissued_and_old_leases_conflict() {
        let harness = Harness::new().await;
        let worker_id = harness.worker("acme", "alpha", &["workflow"], 2).await;
        let (_, auth) = harness.auth(&worker_id).await;
        let job = harness.enqueue("acme", None, "exec-expire").await;
        let (first, _) = harness
            .service
            .lease_jobs(&auth, 1, "0.1.0", &["workflow".to_string()])
            .await
            .unwrap();
        assert_eq!(first.len(), 1);
        let first_lease = first[0].lease_id.clone().unwrap();
        set_lease_expiry(&harness.pool, &job.id, now_ms() - 1).await;
        let (second, _) = harness
            .service
            .lease_jobs(&auth, 1, "0.1.0", &["workflow".to_string()])
            .await
            .unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].attempts, 2);
        assert_ne!(second[0].lease_id.as_deref(), Some(first_lease.as_str()));
        assert_eq!(
            harness
                .service
                .report_job(
                    &auth,
                    &job.id,
                    &first_lease,
                    "running",
                    &state_json("exec-expire")
                )
                .await
                .unwrap_err()
                .kind,
            WorkerErrorKind::Conflict
        );
        assert_eq!(
            harness
                .service
                .complete_job(
                    &auth,
                    &job.id,
                    &first_lease,
                    "failed",
                    &state_json("exec-expire"),
                    Some("stale"),
                )
                .await
                .unwrap_err()
                .kind,
            WorkerErrorKind::Conflict
        );
        let second_lease = second[0].lease_id.clone().unwrap();
        harness
            .service
            .start_job(&auth, &job.id, &second_lease)
            .await
            .unwrap();
        set_lease_expiry(&harness.pool, &job.id, now_ms() - 5_000).await;
        harness
            .service
            .heartbeat(
                &auth,
                "0.1.0",
                &["workflow".to_string()],
                2,
                std::slice::from_ref(&job.id),
            )
            .await
            .unwrap();
        let renewed = harness
            .service
            .get_job("acme", &job.id)
            .await
            .unwrap()
            .unwrap();
        assert!(renewed.lease_expires_at.unwrap() > now_ms());
        harness
            .service
            .report_job(
                &auth,
                &job.id,
                &second_lease,
                "running",
                &state_for("exec-expire", "renewed"),
            )
            .await
            .unwrap();

        let other = harness.enqueue("acme", None, "exec-expire-2").await;
        let (leased, _) = harness
            .service
            .lease_jobs(&auth, 1, "0.1.0", &["workflow".to_string()])
            .await
            .unwrap();
        assert_eq!(leased.len(), 1);
        set_lease_expiry(&harness.pool, &other.id, now_ms() - 1).await;
        assert_eq!(harness.service.expire_leases().await.unwrap(), 1);
        let queued = harness
            .service
            .get_job("acme", &other.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(queued.status, "queued");
        assert!(queued.worker_id.is_none());
        assert!(queued.lease_id.is_none());
        harness.cleanup().await;
    }

    #[tokio::test]
    async fn cancellation_is_immediate_for_idle_jobs_and_cooperative_for_running() {
        let harness = Harness::new().await;
        let worker_id = harness.worker("acme", "alpha", &["workflow"], 2).await;
        let (_, auth) = harness.auth(&worker_id).await;

        let queued = harness.enqueue("acme", None, "exec-queued").await;
        let cancelled = harness
            .service
            .cancel_job("acme", &queued.id)
            .await
            .unwrap();
        assert_eq!(cancelled.status, "cancelled");
        assert!(cancelled.cancel_requested);
        assert!(cancelled.lease_id.is_none());
        assert_eq!(
            harness
                .service
                .cancel_job("acme", &queued.id)
                .await
                .unwrap_err()
                .kind,
            WorkerErrorKind::Conflict
        );

        let running = harness.enqueue("acme", None, "exec-running").await;
        let (jobs, _) = harness
            .service
            .lease_jobs(&auth, 1, "0.1.0", &["workflow".to_string()])
            .await
            .unwrap();
        assert_eq!(jobs[0].id, running.id);
        let lease_id = jobs[0].lease_id.clone().unwrap();
        harness
            .service
            .start_job(&auth, &running.id, &lease_id)
            .await
            .unwrap();
        let flagged = harness
            .service
            .cancel_job("acme", &running.id)
            .await
            .unwrap();
        assert_eq!(flagged.status, "running");
        assert!(flagged.cancel_requested);
        let cancels = harness
            .service
            .heartbeat(
                &auth,
                "0.1.0",
                &["workflow".to_string()],
                2,
                std::slice::from_ref(&running.id),
            )
            .await
            .unwrap();
        assert_eq!(cancels, vec![running.id.clone()]);
        let completed = harness
            .service
            .complete_job(
                &auth,
                &running.id,
                &lease_id,
                "cancelled",
                &state_for("exec-running", "cancelled"),
                None,
            )
            .await
            .unwrap();
        assert_eq!(completed.status, "cancelled");
        assert!(
            harness
                .service
                .heartbeat(
                    &auth,
                    "0.1.0",
                    &["workflow".to_string()],
                    2,
                    std::slice::from_ref(&running.id),
                )
                .await
                .unwrap()
                .is_empty()
        );

        let leased = harness.enqueue("acme", None, "exec-leased").await;
        let (jobs, _) = harness
            .service
            .lease_jobs(&auth, 1, "0.1.0", &["workflow".to_string()])
            .await
            .unwrap();
        assert_eq!(jobs[0].id, leased.id);
        let leased_cancel = harness
            .service
            .cancel_job("acme", &leased.id)
            .await
            .unwrap();
        assert_eq!(leased_cancel.status, "cancelled");
        assert!(leased_cancel.lease_id.is_none());
        harness.cleanup().await;
    }

    #[tokio::test]
    async fn requeue_and_execution_lookup() {
        let harness = Harness::new().await;
        let worker_id = harness.worker("acme", "alpha", &["workflow"], 2).await;
        let (_, auth) = harness.auth(&worker_id).await;
        let job = harness.enqueue("acme", None, "exec-requeue").await;
        assert_eq!(
            harness
                .service
                .job_for_execution("exec-requeue")
                .await
                .unwrap()
                .unwrap()
                .id,
            job.id
        );
        assert!(
            harness
                .service
                .job_for_execution("exec-missing")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            harness
                .service
                .requeue_execution("exec-missing", &state_json("exec-missing"))
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            harness
                .service
                .cancel_execution_job("exec-missing")
                .await
                .unwrap()
                .is_none()
        );

        let (jobs, _) = harness
            .service
            .lease_jobs(&auth, 1, "0.1.0", &["workflow".to_string()])
            .await
            .unwrap();
        let lease_id = jobs[0].lease_id.clone().unwrap();
        harness
            .service
            .start_job(&auth, &job.id, &lease_id)
            .await
            .unwrap();
        let waiting = harness
            .service
            .report_job(
                &auth,
                &job.id,
                &lease_id,
                "waiting_approval",
                &state_json("exec-requeue"),
            )
            .await
            .unwrap();
        assert_eq!(waiting.status, "waiting_approval");
        assert!(waiting.lease_id.is_none());
        assert_eq!(waiting.worker_id.as_deref(), Some(worker_id.as_str()));

        let requeued = harness
            .service
            .requeue_execution("exec-requeue", &state_for("exec-requeue", "approved"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(requeued.status, "queued");
        assert!(requeued.resumed);
        assert!(requeued.worker_id.is_none());
        assert!(requeued.lease_id.is_none());
        assert!(!requeued.cancel_requested);
        assert!(requeued.state_json.contains("approved"));

        let (jobs, _) = harness
            .service
            .lease_jobs(&auth, 1, "0.1.0", &["workflow".to_string()])
            .await
            .unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].attempts, 2);
        assert!(jobs[0].resumed);
        assert!(jobs[0].state_json.contains("approved"));
        let cancelled = harness
            .service
            .cancel_execution_job("exec-requeue")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cancelled.status, "cancelled");
        assert_eq!(
            harness
                .service
                .requeue_execution("exec-requeue", &state_json("exec-requeue"))
                .await
                .unwrap_err()
                .kind,
            WorkerErrorKind::Conflict
        );
        harness.cleanup().await;
    }
}
