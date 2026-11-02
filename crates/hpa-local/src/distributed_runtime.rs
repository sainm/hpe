use serde_json::Value;
use sqlx::FromRow;
use sqlx::postgres::PgPool;
use sqlx::types::Json;
use thiserror::Error;
use time::OffsetDateTime;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("idempotency key {scope}/{key} was reused with a different request hash")]
    IdempotencyKeyReused { scope: String, key: String },
    #[error("invalid distributed runtime input: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone)]
pub struct OutboxMessageInput {
    pub id: String,
    pub topic: String,
    pub event_type: String,
    pub aggregate_type: String,
    pub aggregate_id: String,
    pub payload: Value,
    pub headers: Value,
    pub idempotency_key: String,
    pub max_attempts: i32,
    pub trace_id: Option<String>,
}

#[derive(Debug, Clone, FromRow)]
pub struct OutboxMessage {
    pub tenant_id: String,
    pub id: String,
    pub topic: String,
    pub event_type: String,
    pub aggregate_type: String,
    pub aggregate_id: String,
    pub payload_json: Value,
    pub headers_json: Value,
    pub idempotency_key: String,
    pub status: String,
    pub available_at: OffsetDateTime,
    pub attempts: i32,
    pub max_attempts: i32,
    pub locked_by: Option<String>,
    pub lease_expires_at: Option<OffsetDateTime>,
    pub fencing_token: i64,
    pub last_error: Option<String>,
    pub trace_id: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub processed_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone)]
pub struct IdempotencyReservation {
    pub acquired: bool,
    pub status: String,
    pub request_hash: String,
    pub resource_type: Option<String>,
    pub resource_id: Option<String>,
    pub response: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdempotencyStatus {
    InProgress,
    Succeeded,
    Failed,
}

impl IdempotencyStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::InProgress => "in_progress",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, FromRow)]
pub struct SchedulerLease {
    pub lease_key: String,
    pub holder_id: String,
    pub fencing_token: i64,
    pub acquired_at: OffsetDateTime,
    pub renewed_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
}

#[derive(Debug, Clone, FromRow)]
pub struct WorkerJobLease {
    pub tenant_id: String,
    pub id: String,
    pub worker_id: Option<String>,
    pub execution_id: String,
    pub status: String,
    pub attempts: i32,
    pub lease_id: Option<String>,
    pub lease_expires_at: Option<i64>,
    pub fencing_token: i64,
}

#[derive(Clone)]
pub struct TenantRuntime {
    pool: PgPool,
    tenant_id: String,
    consumer_id: String,
}

impl TenantRuntime {
    pub fn new(pool: PgPool, tenant_id: impl Into<String>, consumer_id: impl Into<String>) -> Self {
        Self {
            pool,
            tenant_id: tenant_id.into(),
            consumer_id: consumer_id.into(),
        }
    }

    pub async fn enqueue(&self, message: &OutboxMessageInput) -> Result<bool, RuntimeError> {
        let mut transaction = self.begin().await?;
        let inserted: bool = sqlx::query_scalar(
            "INSERT INTO hpa_control.outbox (
                 tenant_id, id, topic, event_type, aggregate_type, aggregate_id,
                 payload_json, headers_json, idempotency_key, max_attempts,
                 trace_id, available_at, created_at, updated_at
             )
             VALUES (
                 $1, $2, $3, $4, $5, $6,
                 $7, $8, $9, $10,
                 $11, now(), now(), now()
             )
             ON CONFLICT (tenant_id, idempotency_key) DO NOTHING
             RETURNING true",
        )
        .bind(&self.tenant_id)
        .bind(&message.id)
        .bind(&message.topic)
        .bind(&message.event_type)
        .bind(&message.aggregate_type)
        .bind(&message.aggregate_id)
        .bind(Json(&message.payload))
        .bind(Json(&message.headers))
        .bind(&message.idempotency_key)
        .bind(message.max_attempts)
        .bind(&message.trace_id)
        .fetch_optional(&mut *transaction)
        .await?
        .unwrap_or(false);
        transaction.commit().await?;
        Ok(inserted)
    }

    pub async fn claim(
        &self,
        limit: i32,
        lease_seconds: i32,
    ) -> Result<Vec<OutboxMessage>, RuntimeError> {
        let mut transaction = self.begin().await?;
        let messages = sqlx::query_as::<_, OutboxMessage>(
            "SELECT * FROM hpa_internal.claim_outbox($1, $2, $3)",
        )
        .bind(&self.consumer_id)
        .bind(limit)
        .bind(lease_seconds)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(messages)
    }

    pub async fn claim_topic(
        &self,
        topic: &str,
        limit: i32,
        lease_seconds: i32,
    ) -> Result<Vec<OutboxMessage>, RuntimeError> {
        if topic.trim().is_empty() {
            return Err(RuntimeError::Invalid("topic is required".to_string()));
        }
        let mut transaction = self.begin().await?;
        let messages = sqlx::query_as::<_, OutboxMessage>(
            "SELECT * FROM hpa_internal.claim_outbox_topic($1, $2, $3, $4)",
        )
        .bind(&self.consumer_id)
        .bind(topic)
        .bind(limit)
        .bind(lease_seconds)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(messages)
    }

    pub async fn by_idempotency_key(
        &self,
        key: &str,
    ) -> Result<Option<OutboxMessage>, RuntimeError> {
        let mut transaction = self.begin().await?;
        let message = sqlx::query_as::<_, OutboxMessage>(
            "SELECT * FROM hpa_control.outbox
             WHERE tenant_id = $1 AND idempotency_key = $2",
        )
        .bind(&self.tenant_id)
        .bind(key)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(message)
    }

    pub async fn complete(&self, id: &str, fencing_token: i64) -> Result<bool, RuntimeError> {
        let mut transaction = self.begin().await?;
        let completed: bool = sqlx::query_scalar("SELECT hpa_internal.complete_outbox($1, $2, $3)")
            .bind(id)
            .bind(&self.consumer_id)
            .bind(fencing_token)
            .fetch_one(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(completed)
    }

    pub async fn fail(
        &self,
        id: &str,
        fencing_token: i64,
        error: &str,
        retry_after_seconds: i32,
    ) -> Result<bool, RuntimeError> {
        let mut transaction = self.begin().await?;
        let failed: bool =
            sqlx::query_scalar("SELECT hpa_internal.fail_outbox($1, $2, $3, $4, $5)")
                .bind(id)
                .bind(&self.consumer_id)
                .bind(fencing_token)
                .bind(error)
                .bind(retry_after_seconds)
                .fetch_one(&mut *transaction)
                .await?;
        transaction.commit().await?;
        Ok(failed)
    }

    pub async fn reserve_idempotency(
        &self,
        scope: &str,
        key: &str,
        request_hash: &str,
    ) -> Result<IdempotencyReservation, RuntimeError> {
        let mut transaction = self.begin().await?;
        let acquired: bool = sqlx::query_scalar(
            "INSERT INTO hpa_control.idempotency_keys (
                 tenant_id, scope, idempotency_key, request_hash, status,
                 created_at, updated_at
             )
             VALUES ($1, $2, $3, $4, 'in_progress', now(), now())
             ON CONFLICT (tenant_id, scope, idempotency_key) DO NOTHING
             RETURNING true",
        )
        .bind(&self.tenant_id)
        .bind(scope)
        .bind(key)
        .bind(request_hash)
        .fetch_optional(&mut *transaction)
        .await?
        .unwrap_or(false);
        if acquired {
            transaction.commit().await?;
            return Ok(IdempotencyReservation {
                acquired: true,
                status: "in_progress".to_string(),
                request_hash: request_hash.to_string(),
                resource_type: None,
                resource_id: None,
                response: None,
            });
        }

        let (existing_hash, status, resource_type, resource_id, response): (
            String,
            String,
            Option<String>,
            Option<String>,
            Option<Value>,
        ) = sqlx::query_as(
            "SELECT request_hash, status, resource_type, resource_id, response_json
             FROM hpa_control.idempotency_keys
             WHERE tenant_id = $1 AND scope = $2 AND idempotency_key = $3",
        )
        .bind(&self.tenant_id)
        .bind(scope)
        .bind(key)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        if existing_hash != request_hash {
            return Err(RuntimeError::IdempotencyKeyReused {
                scope: scope.to_string(),
                key: key.to_string(),
            });
        }
        Ok(IdempotencyReservation {
            acquired: false,
            status,
            request_hash: existing_hash,
            resource_type,
            resource_id,
            response,
        })
    }

    pub async fn finish_idempotency(
        &self,
        scope: &str,
        key: &str,
        status: IdempotencyStatus,
        resource_type: Option<&str>,
        resource_id: Option<&str>,
        response: Option<&Value>,
    ) -> Result<bool, RuntimeError> {
        let mut transaction = self.begin().await?;
        let updated = sqlx::query(
            "UPDATE hpa_control.idempotency_keys
             SET status = $4,
                 resource_type = COALESCE($5, resource_type),
                 resource_id = COALESCE($6, resource_id),
                 response_json = COALESCE($7, response_json),
                 updated_at = now()
             WHERE tenant_id = $1
               AND scope = $2
               AND idempotency_key = $3
               AND status = 'in_progress'",
        )
        .bind(&self.tenant_id)
        .bind(scope)
        .bind(key)
        .bind(status.as_str())
        .bind(resource_type)
        .bind(resource_id)
        .bind(response.map(Json))
        .execute(&mut *transaction)
        .await?
        .rows_affected()
            == 1;
        transaction.commit().await?;
        Ok(updated)
    }

    async fn begin(&self) -> Result<sqlx::Transaction<'_, sqlx::Postgres>, RuntimeError> {
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
}

#[derive(Clone)]
pub struct SchedulerLeaseStore {
    pool: PgPool,
    holder_id: String,
}

impl SchedulerLeaseStore {
    pub fn new(pool: PgPool, holder_id: impl Into<String>) -> Self {
        Self {
            pool,
            holder_id: holder_id.into(),
        }
    }

    pub async fn acquire(
        &self,
        lease_key: &str,
        ttl_seconds: i32,
    ) -> Result<Option<SchedulerLease>, RuntimeError> {
        if !(1..=3600).contains(&ttl_seconds) {
            return Err(RuntimeError::Invalid(
                "lease ttl must be between 1 and 3600 seconds".to_string(),
            ));
        }
        let mut transaction = self.pool.begin().await?;
        sqlx::query("SET LOCAL ROLE hpa_scheduler_runtime")
            .execute(&mut *transaction)
            .await?;
        let lease = sqlx::query_as::<_, SchedulerLease>(
            "INSERT INTO hpa_control.runtime_leases (
                 lease_key, holder_id, fencing_token, acquired_at, renewed_at, expires_at
             )
             VALUES (
                 $1, $2, 1, now(), now(), now() + make_interval(secs => $3)
             )
             ON CONFLICT (lease_key) DO UPDATE
             SET holder_id = EXCLUDED.holder_id,
                 fencing_token = hpa_control.runtime_leases.fencing_token + 1,
                 acquired_at = now(),
                 renewed_at = now(),
                 expires_at = now() + make_interval(secs => $3)
             WHERE hpa_control.runtime_leases.expires_at <= now()
             RETURNING lease_key, holder_id, fencing_token,
                       acquired_at, renewed_at, expires_at",
        )
        .bind(lease_key)
        .bind(&self.holder_id)
        .bind(ttl_seconds)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(lease)
    }

    pub async fn renew(
        &self,
        lease_key: &str,
        fencing_token: i64,
        ttl_seconds: i32,
    ) -> Result<bool, RuntimeError> {
        if !(1..=3600).contains(&ttl_seconds) {
            return Err(RuntimeError::Invalid(
                "lease ttl must be between 1 and 3600 seconds".to_string(),
            ));
        }
        let mut transaction = self.pool.begin().await?;
        sqlx::query("SET LOCAL ROLE hpa_scheduler_runtime")
            .execute(&mut *transaction)
            .await?;
        let renewed = sqlx::query(
            "UPDATE hpa_control.runtime_leases
             SET renewed_at = now(),
                 expires_at = now() + make_interval(secs => $4)
             WHERE lease_key = $1
               AND holder_id = $2
               AND fencing_token = $3
               AND expires_at > now()",
        )
        .bind(lease_key)
        .bind(&self.holder_id)
        .bind(fencing_token)
        .bind(ttl_seconds)
        .execute(&mut *transaction)
        .await?
        .rows_affected()
            == 1;
        transaction.commit().await?;
        Ok(renewed)
    }

    pub async fn release(&self, lease_key: &str, fencing_token: i64) -> Result<bool, RuntimeError> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query("SET LOCAL ROLE hpa_scheduler_runtime")
            .execute(&mut *transaction)
            .await?;
        let released = sqlx::query(
            "DELETE FROM hpa_control.runtime_leases
             WHERE lease_key = $1
               AND holder_id = $2
               AND fencing_token = $3",
        )
        .bind(lease_key)
        .bind(&self.holder_id)
        .bind(fencing_token)
        .execute(&mut *transaction)
        .await?
        .rows_affected()
            == 1;
        transaction.commit().await?;
        Ok(released)
    }
}

#[derive(Clone)]
pub struct WorkerLeaseStore {
    pool: PgPool,
    tenant_id: String,
    worker_id: String,
}

impl WorkerLeaseStore {
    pub fn new(pool: PgPool, tenant_id: impl Into<String>, worker_id: impl Into<String>) -> Self {
        Self {
            pool,
            tenant_id: tenant_id.into(),
            worker_id: worker_id.into(),
        }
    }

    pub async fn claim(&self, lease_seconds: i32) -> Result<Option<WorkerJobLease>, RuntimeError> {
        self.validate_ttl(lease_seconds)?;
        let lease_id = format!("lease_{}", uuid::Uuid::now_v7().simple());
        let mut transaction = self.begin().await?;
        let job = sqlx::query_as::<_, WorkerJobLease>(
            "WITH candidate AS (
                 SELECT job.id
                 FROM hpa_control.worker_jobs job
                 JOIN hpa_control.workers worker
                   ON worker.tenant_id = job.tenant_id
                  AND worker.id = $1
                 WHERE worker.status = 'active'
                   AND (
                       job.status = 'queued'
                       OR (
                           job.status IN ('leased', 'running')
                           AND job.lease_expires_at < (
                               extract(epoch FROM now()) * 1000
                           )::bigint
                       )
                   )
                   AND job.required_capabilities_json <@ worker.capabilities_json
                 ORDER BY job.created_at, job.id
                 FOR UPDATE OF job SKIP LOCKED
                 LIMIT 1
             )
             UPDATE hpa_control.worker_jobs job
             SET worker_id = $1,
                 status = 'leased',
                 attempts = job.attempts + 1,
                 lease_id = $3,
                 lease_expires_at = (
                     extract(epoch FROM now() + make_interval(secs => $2)) * 1000
                 )::bigint,
                 fencing_token = job.fencing_token + 1,
                 updated_at = (extract(epoch FROM now()) * 1000)::bigint
             FROM candidate
             WHERE job.id = candidate.id
             RETURNING job.tenant_id, job.id, job.worker_id, job.execution_id,
                       job.status, job.attempts, job.lease_id,
                       job.lease_expires_at, job.fencing_token",
        )
        .bind(&self.worker_id)
        .bind(lease_seconds)
        .bind(lease_id)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(job)
    }

    pub async fn renew(
        &self,
        job_id: &str,
        lease_id: &str,
        fencing_token: i64,
        lease_seconds: i32,
    ) -> Result<bool, RuntimeError> {
        self.validate_ttl(lease_seconds)?;
        let mut transaction = self.begin().await?;
        let renewed = sqlx::query(
            "UPDATE hpa_control.worker_jobs
             SET lease_expires_at = (
                     extract(epoch FROM now() + make_interval(secs => $5)) * 1000
                 )::bigint,
                 updated_at = (extract(epoch FROM now()) * 1000)::bigint
             WHERE id = $1
               AND worker_id = $2
               AND lease_id = $3
               AND fencing_token = $4
               AND status IN ('leased', 'running')
               AND lease_expires_at >= (extract(epoch FROM now()) * 1000)::bigint",
        )
        .bind(job_id)
        .bind(&self.worker_id)
        .bind(lease_id)
        .bind(fencing_token)
        .bind(lease_seconds)
        .execute(&mut *transaction)
        .await?
        .rows_affected()
            == 1;
        transaction.commit().await?;
        Ok(renewed)
    }

    pub async fn finish(
        &self,
        job_id: &str,
        lease_id: &str,
        fencing_token: i64,
        status: &str,
        error: Option<&str>,
    ) -> Result<bool, RuntimeError> {
        if !["succeeded", "failed", "cancelled"].contains(&status) {
            return Err(RuntimeError::Invalid(
                "worker job finish status must be succeeded, failed, or cancelled".to_string(),
            ));
        }
        let mut transaction = self.begin().await?;
        let finished = sqlx::query(
            "UPDATE hpa_control.worker_jobs
             SET status = $5,
                 error = $6,
                 lease_id = NULL,
                 lease_expires_at = NULL,
                 updated_at = (extract(epoch FROM now()) * 1000)::bigint
             WHERE id = $1
               AND worker_id = $2
               AND lease_id = $3
               AND fencing_token = $4
               AND status IN ('leased', 'running')",
        )
        .bind(job_id)
        .bind(&self.worker_id)
        .bind(lease_id)
        .bind(fencing_token)
        .bind(status)
        .bind(error)
        .execute(&mut *transaction)
        .await?
        .rows_affected()
            == 1;
        transaction.commit().await?;
        Ok(finished)
    }

    pub async fn requeue(
        &self,
        job_id: &str,
        lease_id: &str,
        fencing_token: i64,
        error: &str,
    ) -> Result<bool, RuntimeError> {
        let mut transaction = self.begin().await?;
        let requeued = sqlx::query(
            "UPDATE hpa_control.worker_jobs
             SET status = 'queued',
                 worker_id = NULL,
                 lease_id = NULL,
                 lease_expires_at = NULL,
                 error = $5,
                 updated_at = (extract(epoch FROM now()) * 1000)::bigint
             WHERE id = $1
               AND worker_id = $2
               AND lease_id = $3
               AND fencing_token = $4
               AND status IN ('leased', 'running')",
        )
        .bind(job_id)
        .bind(&self.worker_id)
        .bind(lease_id)
        .bind(fencing_token)
        .bind(error)
        .execute(&mut *transaction)
        .await?
        .rows_affected()
            == 1;
        transaction.commit().await?;
        Ok(requeued)
    }

    pub async fn heartbeat(&self) -> Result<bool, RuntimeError> {
        let mut transaction = self.begin().await?;
        let updated = sqlx::query(
            "UPDATE hpa_control.workers
             SET last_heartbeat_at = (extract(epoch FROM now()) * 1000)::bigint
             WHERE id = $1 AND status = 'active'",
        )
        .bind(&self.worker_id)
        .execute(&mut *transaction)
        .await?
        .rows_affected()
            == 1;
        transaction.commit().await?;
        Ok(updated)
    }

    fn validate_ttl(&self, lease_seconds: i32) -> Result<(), RuntimeError> {
        if (1..=3600).contains(&lease_seconds) {
            Ok(())
        } else {
            Err(RuntimeError::Invalid(
                "worker lease ttl must be between 1 and 3600 seconds".to_string(),
            ))
        }
    }

    async fn begin(&self) -> Result<sqlx::Transaction<'_, sqlx::Postgres>, RuntimeError> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query("SET LOCAL ROLE hpa_worker_runtime")
            .execute(&mut *transaction)
            .await?;
        sqlx::query("SELECT set_config('hpa.tenant_id', $1, true)")
            .bind(&self.tenant_id)
            .execute(&mut *transaction)
            .await?;
        Ok(transaction)
    }
}
