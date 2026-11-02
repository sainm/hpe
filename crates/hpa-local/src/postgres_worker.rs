use std::collections::HashSet;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::Row;
use sqlx::postgres::{PgPool, PgRow};
use sqlx::types::Json;
use thiserror::Error;
use uuid::Uuid;

const LEASE_TTL_MS: i64 = 30_000;
const DEFAULT_CAPABILITY: &str = "workflow";
const MAX_CONCURRENCY: i64 = 64;

#[derive(Debug, Error)]
pub enum PostgresWorkerError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid worker data: {0}")]
    Invalid(String),
    #[error("worker `{0}` was not found")]
    WorkerNotFound(String),
    #[error("job `{0}` was not found")]
    JobNotFound(String),
    #[error("worker lease is stale")]
    StaleLease,
    #[error("execution `{0}` is already enqueued")]
    DuplicateExecution(String),
}

#[derive(Debug, Clone)]
pub struct WorkerRecord {
    pub id: String,
    pub tenant_id: String,
    pub name: String,
    pub kind: String,
    pub status: String,
    pub capabilities: Vec<String>,
    pub max_concurrency: i64,
    pub runtime_version: Option<String>,
    pub last_heartbeat_at: i64,
    pub registered_at: i64,
    pub metadata: Value,
}

#[derive(Debug, Clone)]
pub struct WorkerAuthRecord {
    pub worker_id: String,
    pub tenant_id: String,
}

#[derive(Debug, Clone)]
pub struct WorkerJobRecord {
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

#[derive(Clone)]
pub struct PostgresWorkerStore {
    pool: PgPool,
}

impl PostgresWorkerStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_worker(
        &self,
        tenant_id: &str,
        id: &str,
        name: &str,
        kind: &str,
        status: &str,
        capabilities: &[String],
        max_concurrency: i64,
        runtime_version: Option<&str>,
        last_heartbeat_at: i64,
        registered_at: i64,
        metadata: &Value,
    ) -> Result<WorkerRecord, PostgresWorkerError> {
        if !(1..=MAX_CONCURRENCY).contains(&max_concurrency) {
            return Err(PostgresWorkerError::Invalid(format!(
                "max concurrency must be between 1 and {MAX_CONCURRENCY}"
            )));
        }
        let mut transaction = self.begin_tenant(tenant_id).await?;
        let row = sqlx::query(
            "INSERT INTO hpa_control.workers (
                 id, tenant_id, name, kind, status, capabilities_json,
                 max_concurrency, runtime_version, last_heartbeat_at,
                 registered_at, metadata_json
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
             ON CONFLICT (id) DO UPDATE
             SET name = EXCLUDED.name,
                 kind = EXCLUDED.kind,
                 status = EXCLUDED.status,
                 capabilities_json = EXCLUDED.capabilities_json,
                 max_concurrency = EXCLUDED.max_concurrency,
                 runtime_version = EXCLUDED.runtime_version,
                 last_heartbeat_at = EXCLUDED.last_heartbeat_at,
                 metadata_json = EXCLUDED.metadata_json
             WHERE hpa_control.workers.tenant_id = EXCLUDED.tenant_id
             RETURNING *",
        )
        .bind(id)
        .bind(tenant_id)
        .bind(name)
        .bind(kind)
        .bind(status)
        .bind(Json(capabilities))
        .bind(max_concurrency)
        .bind(runtime_version)
        .bind(last_heartbeat_at)
        .bind(registered_at)
        .bind(Json(metadata))
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(|| PostgresWorkerError::WorkerNotFound(id.to_string()))?;
        transaction.commit().await?;
        worker_from_row(row)
    }

    pub async fn list_workers(
        &self,
        tenant_id: Option<&str>,
    ) -> Result<Vec<WorkerRecord>, PostgresWorkerError> {
        let mut transaction = self.begin_admin().await?;
        let rows = sqlx::query(
            "SELECT * FROM hpa_control.workers
             WHERE ($1::text IS NULL OR tenant_id = $1)
             ORDER BY registered_at DESC",
        )
        .bind(tenant_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.into_iter().map(worker_from_row).collect()
    }

    pub async fn heartbeat_worker(
        &self,
        worker_id: &str,
        now_ms: i64,
    ) -> Result<WorkerRecord, PostgresWorkerError> {
        let mut transaction = self.begin_admin().await?;
        let row = sqlx::query(
            "UPDATE hpa_control.workers
             SET last_heartbeat_at = $2,
                 status = CASE WHEN status = 'disabled' THEN 'disabled' ELSE 'active' END
             WHERE id = $1
             RETURNING *",
        )
        .bind(worker_id)
        .bind(now_ms)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(|| PostgresWorkerError::WorkerNotFound(worker_id.to_string()))?;
        transaction.commit().await?;
        worker_from_row(row)
    }

    pub async fn disable_worker(
        &self,
        worker_id: &str,
        now_ms: i64,
    ) -> Result<WorkerRecord, PostgresWorkerError> {
        let mut transaction = self.begin_admin().await?;
        let row = sqlx::query(
            "UPDATE hpa_control.workers
             SET status = 'disabled', last_heartbeat_at = $2
             WHERE id = $1
             RETURNING *",
        )
        .bind(worker_id)
        .bind(now_ms)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(|| PostgresWorkerError::WorkerNotFound(worker_id.to_string()))?;
        transaction.commit().await?;
        worker_from_row(row)
    }

    pub async fn mint_worker_token(
        &self,
        worker_id: &str,
        now_ms: i64,
    ) -> Result<(String, String), PostgresWorkerError> {
        let mut transaction = self.begin_admin().await?;
        let tenant_id: Option<String> =
            sqlx::query_scalar("SELECT tenant_id FROM hpa_control.workers WHERE id = $1")
                .bind(worker_id)
                .fetch_optional(&mut *transaction)
                .await?;
        let tenant_id =
            tenant_id.ok_or_else(|| PostgresWorkerError::WorkerNotFound(worker_id.to_string()))?;
        let mut bytes = [0u8; 32];
        getrandom::getrandom(&mut bytes).map_err(|error| {
            PostgresWorkerError::Invalid(format!("cannot generate token: {error}"))
        })?;
        let token = format!("hpa_wrk_{}", URL_SAFE_NO_PAD.encode(bytes));
        let token_hash = hash_token(&token);
        let token_id = format!("wrktok_{}", Uuid::now_v7());
        sqlx::query(
            "UPDATE hpa_control.worker_tokens SET revoked_at = $2
             WHERE worker_id = $1 AND revoked_at IS NULL",
        )
        .bind(worker_id)
        .bind(now_ms)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO hpa_control.worker_tokens (
                 id, worker_id, tenant_id, token_hash, created_at, revoked_at
             )
             VALUES ($1, $2, $3, $4, $5, NULL)",
        )
        .bind(&token_id)
        .bind(worker_id)
        .bind(&tenant_id)
        .bind(token_hash)
        .bind(now_ms)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok((token, token_id))
    }

    pub async fn revoke_worker_tokens(
        &self,
        worker_id: &str,
        now_ms: i64,
    ) -> Result<(), PostgresWorkerError> {
        let mut transaction = self.begin_admin().await?;
        sqlx::query(
            "UPDATE hpa_control.worker_tokens SET revoked_at = $2
             WHERE worker_id = $1 AND revoked_at IS NULL",
        )
        .bind(worker_id)
        .bind(now_ms)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn authenticate_worker(
        &self,
        token: &str,
    ) -> Result<Option<WorkerAuthRecord>, PostgresWorkerError> {
        if !token.starts_with("hpa_wrk_") || token.len() <= "hpa_wrk_".len() {
            return Ok(None);
        }
        let mut transaction = self.begin_admin().await?;
        let row = sqlx::query(
            "SELECT token.worker_id, token.tenant_id
             FROM hpa_control.worker_tokens token
             JOIN hpa_control.workers worker
               ON worker.id = token.worker_id
              AND worker.tenant_id = token.tenant_id
             JOIN hpa_control.tenants tenant ON tenant.id = token.tenant_id
             WHERE token.token_hash = $1
               AND token.revoked_at IS NULL
               AND worker.status = 'active'
               AND tenant.status = 'active'",
        )
        .bind(hash_token(token))
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.map(|row| {
            Ok(WorkerAuthRecord {
                worker_id: row.try_get("worker_id")?,
                tenant_id: row.try_get("tenant_id")?,
            })
        })
        .transpose()
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
        now_ms: i64,
    ) -> Result<WorkerJobRecord, PostgresWorkerError> {
        let workflow: Value = serde_json::from_str(workflow_json)?;
        let state: Value = serde_json::from_str(state_json)?;
        let mut transaction = self.begin_tenant(tenant_id).await?;
        let active: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                 SELECT 1 FROM hpa_control.tenants
                 WHERE id = $1 AND status = 'active'
             )",
        )
        .bind(tenant_id)
        .fetch_one(&mut *transaction)
        .await?;
        if !active {
            return Err(PostgresWorkerError::Invalid(format!(
                "tenant `{tenant_id}` is not active"
            )));
        }
        if let Some(worker_id) = worker_id {
            let available: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                     SELECT 1 FROM hpa_control.workers
                     WHERE id = $1 AND tenant_id = $2 AND status = 'active'
                 )",
            )
            .bind(worker_id)
            .bind(tenant_id)
            .fetch_one(&mut *transaction)
            .await?;
            if !available {
                return Err(PostgresWorkerError::WorkerNotFound(worker_id.to_string()));
            }
        }
        let id = format!("job_{}", Uuid::now_v7());
        let row = sqlx::query(
            "INSERT INTO hpa_control.worker_jobs (
                 id, tenant_id, worker_id, execution_id, workflow_id,
                 workflow_version, workflow_json, state_json, status,
                 attempts, lease_id, lease_expires_at, runtime_version,
                 required_capabilities_json, resumed, cancel_requested,
                 error, created_at, updated_at
             )
             VALUES (
                 $1, $2, $3, $4, $5, $6, $7, $8, 'queued',
                 0, NULL, NULL, $9, $10, FALSE, FALSE,
                 NULL, $11, $11
             )
             RETURNING *",
        )
        .bind(&id)
        .bind(tenant_id)
        .bind(worker_id)
        .bind(execution_id)
        .bind(workflow_id)
        .bind(workflow_version)
        .bind(Json(&workflow))
        .bind(Json(&state))
        .bind(runtime_version)
        .bind(Json(vec![DEFAULT_CAPABILITY]))
        .bind(now_ms)
        .fetch_one(&mut *transaction)
        .await;
        let row = match row {
            Ok(row) => row,
            Err(error)
                if error
                    .as_database_error()
                    .is_some_and(|e| e.is_unique_violation()) =>
            {
                return Err(PostgresWorkerError::DuplicateExecution(
                    execution_id.to_string(),
                ));
            }
            Err(error) => return Err(error.into()),
        };
        transaction.commit().await?;
        job_from_row(row)
    }

    pub async fn lease_jobs(
        &self,
        auth: &WorkerAuthRecord,
        slots: i64,
        version: &str,
        capabilities: &[String],
        now_ms: i64,
    ) -> Result<(Vec<WorkerJobRecord>, Vec<String>), PostgresWorkerError> {
        let mut transaction = self.begin_tenant(&auth.tenant_id).await?;
        sqlx::query(
            "UPDATE hpa_control.worker_jobs
             SET status = 'queued', worker_id = NULL, lease_id = NULL,
                 lease_expires_at = NULL, updated_at = $1
             WHERE tenant_id = $2
               AND status IN ('leased', 'running')
               AND lease_expires_at IS NOT NULL
               AND lease_expires_at < $1
               AND cancel_requested = FALSE",
        )
        .bind(now_ms)
        .bind(&auth.tenant_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE hpa_control.worker_jobs
             SET status = 'cancelled', lease_id = NULL,
                 lease_expires_at = NULL, updated_at = $1
             WHERE tenant_id = $2
               AND status IN ('leased', 'running')
               AND lease_expires_at IS NOT NULL
               AND lease_expires_at < $1
               AND cancel_requested = TRUE",
        )
        .bind(now_ms)
        .bind(&auth.tenant_id)
        .execute(&mut *transaction)
        .await?;
        let worker = sqlx::query(
            "SELECT max_concurrency, capabilities_json
             FROM hpa_control.workers
             WHERE id = $1 AND tenant_id = $2 AND status = 'active'",
        )
        .bind(&auth.worker_id)
        .bind(&auth.tenant_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(|| PostgresWorkerError::WorkerNotFound(auth.worker_id.clone()))?;
        let max_concurrency: i64 = i64::from(worker.try_get::<i32, _>("max_concurrency")?);
        let registered_json: Value = worker.try_get("capabilities_json")?;
        let registered_values: Vec<String> =
            serde_json::from_value(registered_json).unwrap_or_default();
        let registered: HashSet<String> = registered_values.into_iter().collect();
        let requested: HashSet<String> = capabilities.iter().cloned().collect();
        sqlx::query(
            "UPDATE hpa_control.workers
             SET runtime_version = $3, capabilities_json = $4,
                 last_heartbeat_at = $5, status = 'active'
             WHERE id = $1 AND tenant_id = $2",
        )
        .bind(&auth.worker_id)
        .bind(&auth.tenant_id)
        .bind(version)
        .bind(Json(capabilities))
        .bind(now_ms)
        .execute(&mut *transaction)
        .await?;
        let active: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM hpa_control.worker_jobs
             WHERE tenant_id = $1 AND worker_id = $2
               AND status IN ('leased', 'running')",
        )
        .bind(&auth.tenant_id)
        .bind(&auth.worker_id)
        .fetch_one(&mut *transaction)
        .await?;
        let available = max_concurrency
            .clamp(1, MAX_CONCURRENCY)
            .saturating_sub(active)
            .min(slots.max(0));
        let candidates = sqlx::query(
            "SELECT id, required_capabilities_json
             FROM hpa_control.worker_jobs
             WHERE tenant_id = $1 AND status = 'queued'
               AND (worker_id IS NULL OR worker_id = $2)
               AND runtime_version = $3
             ORDER BY created_at ASC, id ASC
             LIMIT 256",
        )
        .bind(&auth.tenant_id)
        .bind(&auth.worker_id)
        .bind(version)
        .fetch_all(&mut *transaction)
        .await?;
        let mut jobs = Vec::new();
        for candidate in candidates {
            if jobs.len() as i64 >= available {
                break;
            }
            let job_id: String = candidate.try_get("id")?;
            let required_json: Value = candidate.try_get("required_capabilities_json")?;
            let required = required_capabilities(&serde_json::to_string(&required_json)?);
            if required.iter().any(|capability| {
                !requested.contains(capability) || !registered.contains(capability)
            }) {
                continue;
            }
            let lease_id = format!("lease_{}", Uuid::now_v7());
            let expires_at = now_ms + LEASE_TTL_MS;
            let updated = sqlx::query(
                "UPDATE hpa_control.worker_jobs
                 SET status = 'leased', worker_id = $2,
                     attempts = attempts + 1, lease_id = $3,
                     lease_expires_at = $4,
                     fencing_token = fencing_token + 1,
                     updated_at = $5
                 WHERE tenant_id = $1 AND id = $6 AND status = 'queued'",
            )
            .bind(&auth.tenant_id)
            .bind(&auth.worker_id)
            .bind(&lease_id)
            .bind(expires_at)
            .bind(now_ms)
            .bind(&job_id)
            .execute(&mut *transaction)
            .await?;
            if updated.rows_affected() == 0 {
                continue;
            }
            let row = sqlx::query(
                "SELECT * FROM hpa_control.worker_jobs
                 WHERE tenant_id = $1 AND id = $2",
            )
            .bind(&auth.tenant_id)
            .bind(&job_id)
            .fetch_one(&mut *transaction)
            .await?;
            jobs.push(job_from_row(row)?);
        }
        let cancels: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM hpa_control.worker_jobs
             WHERE tenant_id = $1 AND worker_id = $2
               AND cancel_requested = TRUE
               AND status IN ('leased', 'running', 'waiting_approval')
             ORDER BY created_at ASC",
        )
        .bind(&auth.tenant_id)
        .bind(&auth.worker_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok((jobs, cancels))
    }

    pub async fn heartbeat(
        &self,
        auth: &WorkerAuthRecord,
        version: &str,
        capabilities: &[String],
        max_concurrency: i64,
        active_jobs: &[String],
        now_ms: i64,
    ) -> Result<Vec<String>, PostgresWorkerError> {
        let mut transaction = self.begin_tenant(&auth.tenant_id).await?;
        let worker = sqlx::query(
            "UPDATE hpa_control.workers
             SET runtime_version = $3, capabilities_json = $4,
                 max_concurrency = $5, last_heartbeat_at = $6,
                 status = 'active'
             WHERE id = $1 AND tenant_id = $2 AND status = 'active'
             RETURNING id",
        )
        .bind(&auth.worker_id)
        .bind(&auth.tenant_id)
        .bind(version)
        .bind(Json(capabilities))
        .bind(max_concurrency)
        .bind(now_ms)
        .fetch_optional(&mut *transaction)
        .await?;
        if worker.is_none() {
            return Err(PostgresWorkerError::WorkerNotFound(auth.worker_id.clone()));
        }
        for job_id in active_jobs {
            sqlx::query(
                "UPDATE hpa_control.worker_jobs
                 SET lease_expires_at = $4, updated_at = $5
                 WHERE tenant_id = $1 AND id = $2 AND worker_id = $3
                   AND status IN ('leased', 'running')
                   AND lease_expires_at >= $5",
            )
            .bind(&auth.tenant_id)
            .bind(job_id)
            .bind(&auth.worker_id)
            .bind(now_ms + LEASE_TTL_MS)
            .bind(now_ms)
            .execute(&mut *transaction)
            .await?;
        }
        let cancels: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM hpa_control.worker_jobs
             WHERE tenant_id = $1 AND worker_id = $2
               AND cancel_requested = TRUE
               AND status IN ('leased', 'running', 'waiting_approval')
             ORDER BY created_at ASC",
        )
        .bind(&auth.tenant_id)
        .bind(&auth.worker_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(cancels)
    }

    pub async fn start_job(
        &self,
        auth: &WorkerAuthRecord,
        job_id: &str,
        lease_id: &str,
        now_ms: i64,
    ) -> Result<WorkerJobRecord, PostgresWorkerError> {
        let mut transaction = self.begin_tenant(&auth.tenant_id).await?;
        let job = fetch_job_tx(&mut transaction, &auth.tenant_id, job_id)
            .await?
            .ok_or_else(|| PostgresWorkerError::JobNotFound(job_id.to_string()))?;
        validate_lease(&job, &auth.worker_id, lease_id, now_ms)?;
        if !matches!(job.status.as_str(), "leased" | "running") {
            return Err(PostgresWorkerError::Invalid(format!(
                "job cannot start from status `{}`",
                job.status
            )));
        }
        sqlx::query(
            "UPDATE hpa_control.worker_jobs
             SET status = 'running', error = NULL, updated_at = $4
             WHERE tenant_id = $1 AND id = $2 AND worker_id = $3
               AND lease_id = $5
               AND status IN ('leased', 'running')
               AND lease_expires_at >= $4",
        )
        .bind(&auth.tenant_id)
        .bind(job_id)
        .bind(&auth.worker_id)
        .bind(now_ms)
        .bind(lease_id)
        .execute(&mut *transaction)
        .await?;
        let updated = fetch_job_tx(&mut transaction, &auth.tenant_id, job_id)
            .await?
            .ok_or_else(|| PostgresWorkerError::JobNotFound(job_id.to_string()))?;
        transaction.commit().await?;
        Ok(updated)
    }

    pub async fn report_job(
        &self,
        auth: &WorkerAuthRecord,
        job_id: &str,
        lease_id: &str,
        status: &str,
        state_json: &str,
        now_ms: i64,
    ) -> Result<WorkerJobRecord, PostgresWorkerError> {
        let state: Value = serde_json::from_str(state_json)?;
        let mut transaction = self.begin_tenant(&auth.tenant_id).await?;
        let job = fetch_job_tx(&mut transaction, &auth.tenant_id, job_id)
            .await?
            .ok_or_else(|| PostgresWorkerError::JobNotFound(job_id.to_string()))?;
        validate_lease(&job, &auth.worker_id, lease_id, now_ms)?;
        if !matches!(job.status.as_str(), "leased" | "running") {
            return Err(PostgresWorkerError::Invalid(format!(
                "job cannot report from status `{}`",
                job.status
            )));
        }
        if status == "waiting_approval" {
            sqlx::query(
                "UPDATE hpa_control.worker_jobs
                 SET status = 'waiting_approval', state_json = $4,
                     lease_id = NULL, lease_expires_at = NULL,
                     updated_at = $5
                 WHERE tenant_id = $1 AND id = $2 AND worker_id = $3
                   AND lease_id = $6 AND status IN ('leased', 'running')",
            )
            .bind(&auth.tenant_id)
            .bind(job_id)
            .bind(&auth.worker_id)
            .bind(Json(&state))
            .bind(now_ms)
            .bind(lease_id)
            .execute(&mut *transaction)
            .await?;
        } else {
            sqlx::query(
                "UPDATE hpa_control.worker_jobs
                 SET status = 'running', state_json = $4, updated_at = $5
                 WHERE tenant_id = $1 AND id = $2 AND worker_id = $3
                   AND lease_id = $6 AND status IN ('leased', 'running')
                   AND lease_expires_at >= $5",
            )
            .bind(&auth.tenant_id)
            .bind(job_id)
            .bind(&auth.worker_id)
            .bind(Json(&state))
            .bind(now_ms)
            .bind(lease_id)
            .execute(&mut *transaction)
            .await?;
        }
        let updated = fetch_job_tx(&mut transaction, &auth.tenant_id, job_id)
            .await?
            .ok_or_else(|| PostgresWorkerError::JobNotFound(job_id.to_string()))?;
        if updated.status != status {
            return Err(PostgresWorkerError::StaleLease);
        }
        transaction.commit().await?;
        Ok(updated)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn complete_job(
        &self,
        auth: &WorkerAuthRecord,
        job_id: &str,
        lease_id: &str,
        status: &str,
        state_json: &str,
        error: Option<&str>,
        now_ms: i64,
    ) -> Result<WorkerJobRecord, PostgresWorkerError> {
        let state: Value = serde_json::from_str(state_json)?;
        let mut transaction = self.begin_tenant(&auth.tenant_id).await?;
        let job = fetch_job_tx(&mut transaction, &auth.tenant_id, job_id)
            .await?
            .ok_or_else(|| PostgresWorkerError::JobNotFound(job_id.to_string()))?;
        if job.worker_id.as_deref() != Some(auth.worker_id.as_str()) {
            return Err(PostgresWorkerError::StaleLease);
        }
        if matches!(job.status.as_str(), "succeeded" | "failed" | "cancelled") {
            if job.status == status {
                transaction.commit().await?;
                return Ok(job);
            }
            return Err(PostgresWorkerError::Invalid(format!(
                "job is already `{}`",
                job.status
            )));
        }
        if status == "waiting_approval" {
            sqlx::query(
                "UPDATE hpa_control.worker_jobs
                 SET status = 'waiting_approval', state_json = $4,
                     lease_id = NULL, lease_expires_at = NULL,
                     error = NULL, updated_at = $5
                 WHERE tenant_id = $1 AND id = $2 AND worker_id = $3",
            )
            .bind(&auth.tenant_id)
            .bind(job_id)
            .bind(&auth.worker_id)
            .bind(Json(&state))
            .bind(now_ms)
            .execute(&mut *transaction)
            .await?;
        } else {
            if job.status != "waiting_approval" {
                validate_lease(&job, &auth.worker_id, lease_id, now_ms)?;
            }
            sqlx::query(
                "UPDATE hpa_control.worker_jobs
                 SET status = $4, state_json = $5,
                     lease_id = NULL, lease_expires_at = NULL,
                     error = $6, updated_at = $7
                 WHERE tenant_id = $1 AND id = $2 AND worker_id = $3",
            )
            .bind(&auth.tenant_id)
            .bind(job_id)
            .bind(&auth.worker_id)
            .bind(status)
            .bind(Json(&state))
            .bind(error)
            .bind(now_ms)
            .execute(&mut *transaction)
            .await?;
        }
        let updated = fetch_job_tx(&mut transaction, &auth.tenant_id, job_id)
            .await?
            .ok_or_else(|| PostgresWorkerError::JobNotFound(job_id.to_string()))?;
        transaction.commit().await?;
        Ok(updated)
    }

    pub async fn cancel_job(
        &self,
        tenant_id: &str,
        job_id: &str,
        now_ms: i64,
    ) -> Result<WorkerJobRecord, PostgresWorkerError> {
        let mut transaction = self.begin_tenant(tenant_id).await?;
        let job = fetch_job_tx(&mut transaction, tenant_id, job_id)
            .await?
            .ok_or_else(|| PostgresWorkerError::JobNotFound(job_id.to_string()))?;
        if matches!(job.status.as_str(), "succeeded" | "failed" | "cancelled") {
            return Err(PostgresWorkerError::Invalid(format!(
                "job is already `{}`",
                job.status
            )));
        }
        if job.status == "running" {
            sqlx::query(
                "UPDATE hpa_control.worker_jobs
                 SET cancel_requested = TRUE, updated_at = $3
                 WHERE tenant_id = $1 AND id = $2 AND status = 'running'",
            )
            .bind(tenant_id)
            .bind(job_id)
            .bind(now_ms)
            .execute(&mut *transaction)
            .await?;
        } else {
            sqlx::query(
                "UPDATE hpa_control.worker_jobs
                 SET status = 'cancelled', cancel_requested = TRUE,
                     lease_id = NULL, lease_expires_at = NULL,
                     updated_at = $3
                 WHERE tenant_id = $1 AND id = $2
                   AND status IN ('queued', 'leased', 'waiting_approval')",
            )
            .bind(tenant_id)
            .bind(job_id)
            .bind(now_ms)
            .execute(&mut *transaction)
            .await?;
        }
        let updated = fetch_job_tx(&mut transaction, tenant_id, job_id)
            .await?
            .ok_or_else(|| PostgresWorkerError::JobNotFound(job_id.to_string()))?;
        transaction.commit().await?;
        Ok(updated)
    }

    pub async fn requeue_execution(
        &self,
        execution_id: &str,
        state_json: &str,
        now_ms: i64,
    ) -> Result<Option<WorkerJobRecord>, PostgresWorkerError> {
        let state: Value = serde_json::from_str(state_json)?;
        let Some(job) = self.job_for_execution(execution_id).await? else {
            return Ok(None);
        };
        let mut transaction = self.begin_tenant(&job.tenant_id).await?;
        if matches!(job.status.as_str(), "succeeded" | "failed" | "cancelled") {
            return Err(PostgresWorkerError::Invalid(format!(
                "job is already `{}`",
                job.status
            )));
        }
        if !matches!(job.status.as_str(), "waiting_approval" | "running") {
            return Err(PostgresWorkerError::Invalid(format!(
                "job cannot be requeued from status `{}`",
                job.status
            )));
        }
        sqlx::query(
            "UPDATE hpa_control.worker_jobs
             SET status = 'queued', state_json = $3, resumed = TRUE,
                 worker_id = NULL, lease_id = NULL, lease_expires_at = NULL,
                 cancel_requested = FALSE, error = NULL, updated_at = $4
             WHERE tenant_id = $1 AND id = $2
               AND status IN ('waiting_approval', 'running')",
        )
        .bind(&job.tenant_id)
        .bind(&job.id)
        .bind(Json(&state))
        .bind(now_ms)
        .execute(&mut *transaction)
        .await?;
        let updated = fetch_job_tx(&mut transaction, &job.tenant_id, &job.id).await?;
        transaction.commit().await?;
        Ok(updated)
    }

    pub async fn get_job(
        &self,
        tenant_id: &str,
        job_id: &str,
    ) -> Result<Option<WorkerJobRecord>, PostgresWorkerError> {
        let mut transaction = self.begin_tenant(tenant_id).await?;
        let row = sqlx::query(
            "SELECT * FROM hpa_control.worker_jobs
             WHERE tenant_id = $1 AND id = $2",
        )
        .bind(tenant_id)
        .bind(job_id)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.map(job_from_row).transpose()
    }

    pub async fn list_jobs(
        &self,
        tenant_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<WorkerJobRecord>, PostgresWorkerError> {
        let mut transaction = self.begin_admin().await?;
        let rows = sqlx::query(
            "SELECT * FROM hpa_control.worker_jobs
             WHERE ($1::text IS NULL OR tenant_id = $1)
             ORDER BY created_at DESC, id DESC
             LIMIT $2",
        )
        .bind(tenant_id)
        .bind(limit.clamp(1, 200))
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.into_iter().map(job_from_row).collect()
    }

    pub async fn waiting_approval_jobs(&self) -> Result<Vec<WorkerJobRecord>, PostgresWorkerError> {
        let mut transaction = self.begin_admin().await?;
        let rows = sqlx::query(
            "SELECT * FROM hpa_control.worker_jobs
             WHERE status = 'waiting_approval' AND cancel_requested = FALSE
             ORDER BY created_at ASC LIMIT 200",
        )
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.into_iter().map(job_from_row).collect()
    }

    pub async fn job_for_execution(
        &self,
        execution_id: &str,
    ) -> Result<Option<WorkerJobRecord>, PostgresWorkerError> {
        let mut transaction = self.begin_admin().await?;
        let row = sqlx::query("SELECT * FROM hpa_control.worker_jobs WHERE execution_id = $1")
            .bind(execution_id)
            .fetch_optional(&mut *transaction)
            .await?;
        transaction.commit().await?;
        row.map(job_from_row).transpose()
    }

    pub async fn expire_leases(&self, now_ms: i64) -> Result<usize, PostgresWorkerError> {
        let mut transaction = self.begin_admin().await?;
        let requeued = sqlx::query(
            "UPDATE hpa_control.worker_jobs
             SET status = 'queued', worker_id = NULL, lease_id = NULL,
                 lease_expires_at = NULL, updated_at = $1
             WHERE status IN ('leased', 'running')
               AND lease_expires_at IS NOT NULL
               AND lease_expires_at < $1
               AND cancel_requested = FALSE",
        )
        .bind(now_ms)
        .execute(&mut *transaction)
        .await?
        .rows_affected() as usize;
        let cancelled = sqlx::query(
            "UPDATE hpa_control.worker_jobs
             SET status = 'cancelled', lease_id = NULL,
                 lease_expires_at = NULL, updated_at = $1
             WHERE status IN ('leased', 'running')
               AND lease_expires_at IS NOT NULL
               AND lease_expires_at < $1
               AND cancel_requested = TRUE",
        )
        .bind(now_ms)
        .execute(&mut *transaction)
        .await?
        .rows_affected() as usize;
        transaction.commit().await?;
        Ok(requeued + cancelled)
    }

    async fn begin_tenant(
        &self,
        tenant_id: &str,
    ) -> Result<sqlx::Transaction<'_, sqlx::Postgres>, PostgresWorkerError> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query("SET LOCAL ROLE hpa_tenant_runtime")
            .execute(&mut *transaction)
            .await?;
        sqlx::query("SELECT set_config('hpa.tenant_id', $1, true)")
            .bind(tenant_id)
            .execute(&mut *transaction)
            .await?;
        Ok(transaction)
    }

    async fn begin_admin(
        &self,
    ) -> Result<sqlx::Transaction<'_, sqlx::Postgres>, PostgresWorkerError> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query("SET LOCAL ROLE hpa_platform_admin")
            .execute(&mut *transaction)
            .await?;
        Ok(transaction)
    }
}

fn worker_from_row(row: PgRow) -> Result<WorkerRecord, PostgresWorkerError> {
    let capabilities: Value = row.try_get("capabilities_json")?;
    let metadata: Value = row.try_get("metadata_json")?;
    Ok(WorkerRecord {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        name: row.try_get("name")?,
        kind: row.try_get("kind")?,
        status: row.try_get("status")?,
        capabilities: serde_json::from_value(capabilities).unwrap_or_default(),
        // `hpa_control.workers.max_concurrency` is INTEGER (INT4) by schema.
        max_concurrency: i64::from(row.try_get::<i32, _>("max_concurrency")?),
        runtime_version: row.try_get("runtime_version")?,
        last_heartbeat_at: row.try_get("last_heartbeat_at")?,
        registered_at: row.try_get("registered_at")?,
        metadata,
    })
}

fn job_from_row(row: PgRow) -> Result<WorkerJobRecord, PostgresWorkerError> {
    let workflow_json: Value = row.try_get("workflow_json")?;
    let state_json: Value = row.try_get("state_json")?;
    Ok(WorkerJobRecord {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        worker_id: row.try_get("worker_id")?,
        execution_id: row.try_get("execution_id")?,
        workflow_id: row.try_get("workflow_id")?,
        workflow_version: row.try_get("workflow_version")?,
        workflow_json: serde_json::to_string(&workflow_json)?,
        state_json: serde_json::to_string(&state_json)?,
        status: row.try_get("status")?,
        attempts: i64::from(row.try_get::<i32, _>("attempts")?),
        lease_id: row.try_get("lease_id")?,
        lease_expires_at: row.try_get("lease_expires_at")?,
        runtime_version: row.try_get("runtime_version")?,
        resumed: row.try_get("resumed")?,
        cancel_requested: row.try_get("cancel_requested")?,
        error: row.try_get("error")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

async fn fetch_job_tx(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: &str,
    job_id: &str,
) -> Result<Option<WorkerJobRecord>, PostgresWorkerError> {
    let row = sqlx::query(
        "SELECT * FROM hpa_control.worker_jobs
         WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id)
    .bind(job_id)
    .fetch_optional(&mut **transaction)
    .await?;
    row.map(job_from_row).transpose()
}

fn validate_lease(
    job: &WorkerJobRecord,
    worker_id: &str,
    lease_id: &str,
    now_ms: i64,
) -> Result<(), PostgresWorkerError> {
    if job.worker_id.as_deref() != Some(worker_id)
        || job.lease_id.as_deref() != Some(lease_id)
        || job
            .lease_expires_at
            .is_none_or(|expires_at| expires_at < now_ms)
    {
        return Err(PostgresWorkerError::StaleLease);
    }
    Ok(())
}

fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn required_capabilities(raw: &str) -> Vec<String> {
    let capabilities = serde_json::from_str::<Vec<String>>(raw).unwrap_or_default();
    if capabilities.is_empty() {
        vec![DEFAULT_CAPABILITY.to_string()]
    } else {
        capabilities
    }
}
