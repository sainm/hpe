use serde_json::{Value, json};
use sqlx::postgres::PgPool;
use sqlx::types::Json;
use sqlx::{FromRow, Row};
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum McpStoreError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("invalid MCP state: {0}")]
    Invalid(String),
    #[error("MCP session {0} was not found for this token or has expired")]
    SessionNotFound(String),
    #[error("MCP subscription {0} was not found or has expired")]
    SubscriptionNotFound(String),
    #[error("MCP task {0} was not found")]
    TaskNotFound(String),
    #[error("MCP task {task_id} cannot transition from {current} to {requested}")]
    InvalidTaskTransition {
        task_id: String,
        current: String,
        requested: String,
    },
}

#[derive(Debug, Clone, FromRow)]
pub struct PersistentSession {
    pub tenant_id: String,
    pub id: String,
    pub token_id: String,
    pub subject: Option<String>,
    pub created_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
    pub last_seen_at: Option<OffsetDateTime>,
    pub revoked_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, FromRow)]
pub struct McpNotification {
    pub sequence: i64,
    pub kind: String,
    pub payload_json: Value,
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, FromRow)]
pub struct PersistentTask {
    pub tenant_id: String,
    pub id: String,
    pub session_id: Option<String>,
    pub token_id: String,
    pub method: String,
    pub status: String,
    pub input_json: Value,
    pub output_json: Option<Value>,
    pub error_json: Option<Value>,
    pub approval_json: Option<Value>,
    pub execution_id: Option<String>,
    pub owner_node_id: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub expires_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone)]
pub struct TaskUpsert {
    pub id: String,
    pub session_id: Option<String>,
    pub method: String,
    pub status: String,
    pub input: Value,
    pub output: Option<Value>,
    pub error: Option<Value>,
    pub approval: Option<Value>,
    pub execution_id: Option<String>,
    pub owner_node_id: Option<String>,
    pub expires_in_seconds: Option<i32>,
}

#[derive(Debug, Clone, FromRow)]
pub struct TaskUpdateCommand {
    pub tenant_id: String,
    pub id: String,
    pub task_id: String,
    pub owner_node_id: String,
    pub token_id: String,
    pub action: String,
    pub payload_json: Value,
    pub attempts: i32,
    pub execution_id: Option<String>,
    pub task_status: String,
}

#[derive(Clone)]
pub struct PostgresMcpTaskNode {
    pool: PgPool,
    tenant_id: String,
    node_id: String,
}

#[derive(Clone)]
pub struct PostgresMcpStore {
    pool: PgPool,
    tenant_id: String,
    token_id: String,
}

impl PostgresMcpStore {
    pub fn new(pool: PgPool, tenant_id: impl Into<String>, token_id: impl Into<String>) -> Self {
        Self {
            pool,
            tenant_id: tenant_id.into(),
            token_id: token_id.into(),
        }
    }

    pub async fn create_session(
        &self,
        subject: Option<&str>,
        ttl_seconds: i32,
    ) -> Result<PersistentSession, McpStoreError> {
        self.validate_ttl(ttl_seconds)?;
        let id = format!("mcpsess_{}", Uuid::now_v7());
        let mut transaction = self.begin().await?;
        let session = sqlx::query_as::<_, PersistentSession>(
            "INSERT INTO hpa_control.mcp_sessions (
                 tenant_id, id, token_id, subject, created_at, expires_at
             )
             VALUES (
                 $1, $2, $3, $4, now(), now() + make_interval(secs => $5)
             )
             RETURNING *",
        )
        .bind(&self.tenant_id)
        .bind(id)
        .bind(&self.token_id)
        .bind(subject)
        .bind(ttl_seconds)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(session)
    }

    pub async fn validate_session(&self, id: &str) -> Result<bool, McpStoreError> {
        let mut transaction = self.begin().await?;
        let valid: bool = sqlx::query_scalar(
            "UPDATE hpa_control.mcp_sessions
             SET last_seen_at = now()
             WHERE id = $1
               AND token_id = $2
               AND revoked_at IS NULL
               AND expires_at > now()
             RETURNING true",
        )
        .bind(id)
        .bind(&self.token_id)
        .fetch_optional(&mut *transaction)
        .await?
        .unwrap_or(false);
        transaction.commit().await?;
        Ok(valid)
    }

    pub async fn close_session(&self, id: &str) -> Result<bool, McpStoreError> {
        let mut transaction = self.begin().await?;
        let closed = sqlx::query(
            "DELETE FROM hpa_control.mcp_sessions
             WHERE id = $1 AND token_id = $2",
        )
        .bind(id)
        .bind(&self.token_id)
        .execute(&mut *transaction)
        .await?
        .rows_affected()
            == 1;
        transaction.commit().await?;
        Ok(closed)
    }

    pub async fn create_subscription(
        &self,
        session_id: &str,
        requested: &Value,
        ttl_seconds: i32,
    ) -> Result<String, McpStoreError> {
        self.validate_ttl(ttl_seconds)?;
        if !requested.is_object() {
            return Err(McpStoreError::Invalid(
                "subscription request must be an object".to_string(),
            ));
        }
        let id = format!("mcpsub_{}", Uuid::now_v7());
        let mut transaction = self.begin().await?;
        let inserted: Option<String> = sqlx::query_scalar(
            "INSERT INTO hpa_control.mcp_subscriptions (
                 tenant_id, id, session_id, requested_json, created_at, expires_at
             )
             SELECT $1, $2, session.id, $4, now(),
                    now() + make_interval(secs => $5)
             FROM hpa_control.mcp_sessions session
             WHERE session.id = $3
               AND session.token_id = $6
               AND session.revoked_at IS NULL
               AND session.expires_at > now()
             RETURNING id",
        )
        .bind(&self.tenant_id)
        .bind(&id)
        .bind(session_id)
        .bind(Json(requested))
        .bind(ttl_seconds)
        .bind(&self.token_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(inserted) = inserted else {
            transaction.rollback().await?;
            return Err(McpStoreError::SessionNotFound(session_id.to_string()));
        };
        transaction.commit().await?;
        Ok(inserted)
    }

    pub async fn publish_notification(
        &self,
        kind: &str,
        payload: &Value,
    ) -> Result<i64, McpStoreError> {
        if !["tools", "resources", "prompts", "tasks"].contains(&kind) {
            return Err(McpStoreError::Invalid(format!(
                "unsupported MCP notification kind {kind:?}"
            )));
        }
        let mut transaction = self.begin().await?;
        let sequence = sqlx::query_scalar(
            "INSERT INTO hpa_control.mcp_notifications (
                 tenant_id, kind, payload_json, created_at
             )
             VALUES ($1, $2, $3, now())
             RETURNING sequence",
        )
        .bind(&self.tenant_id)
        .bind(kind)
        .bind(Json(payload))
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(sequence)
    }

    pub async fn latest_notification_sequence(&self) -> Result<i64, McpStoreError> {
        let mut transaction = self.begin().await?;
        let sequence = sqlx::query_scalar(
            "SELECT COALESCE(max(sequence), 0)
             FROM hpa_control.mcp_notifications",
        )
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(sequence)
    }

    /// Counts matching notifications after a cursor, capped so a slow
    /// consumer check remains bounded regardless of total backlog.
    pub async fn pending_notification_count(
        &self,
        subscription_id: &str,
        after_sequence: i64,
        cap: i32,
    ) -> Result<i64, McpStoreError> {
        if !(1..=10_000).contains(&cap) {
            return Err(McpStoreError::Invalid(
                "notification backlog cap must be between 1 and 10000".to_string(),
            ));
        }
        let mut transaction = self.begin().await?;
        let requested: Option<Value> = sqlx::query_scalar(
            "SELECT requested_json
             FROM hpa_control.mcp_subscriptions
             WHERE id = $1
               AND revoked_at IS NULL
               AND expires_at > now()",
        )
        .bind(subscription_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(requested) = requested else {
            return Err(McpStoreError::SubscriptionNotFound(
                subscription_id.to_string(),
            ));
        };
        let enabled = enabled_notification_kinds(&requested);
        if enabled.is_empty() {
            transaction.commit().await?;
            return Ok(0);
        }
        let count = sqlx::query_scalar(
            "SELECT count(*)
             FROM (
                 SELECT 1
                 FROM hpa_control.mcp_notifications
                 WHERE sequence > $1
                   AND kind = ANY($2)
                 ORDER BY sequence
                 LIMIT $3
             ) pending",
        )
        .bind(after_sequence)
        .bind(&enabled)
        .bind(cap)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(count)
    }

    pub async fn poll_subscription(
        &self,
        subscription_id: &str,
        after_sequence: i64,
        limit: i32,
    ) -> Result<Vec<McpNotification>, McpStoreError> {
        if !(1..=100).contains(&limit) {
            return Err(McpStoreError::Invalid(
                "notification limit must be between 1 and 100".to_string(),
            ));
        }
        let mut transaction = self.begin().await?;
        let requested: Option<Value> = sqlx::query_scalar(
            "SELECT requested_json
             FROM hpa_control.mcp_subscriptions
             WHERE id = $1
               AND revoked_at IS NULL
               AND expires_at > now()",
        )
        .bind(subscription_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(requested) = requested else {
            return Err(McpStoreError::SubscriptionNotFound(
                subscription_id.to_string(),
            ));
        };
        let enabled = enabled_notification_kinds(&requested);
        if enabled.is_empty() {
            transaction.commit().await?;
            return Ok(Vec::new());
        }
        let notifications = sqlx::query_as::<_, McpNotification>(
            "SELECT sequence, kind, payload_json, created_at
             FROM hpa_control.mcp_notifications
             WHERE sequence > $1
               AND kind = ANY($2)
             ORDER BY sequence
             LIMIT $3",
        )
        .bind(after_sequence)
        .bind(&enabled)
        .bind(limit)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(notifications)
    }

    pub async fn upsert_task(&self, task: &TaskUpsert) -> Result<PersistentTask, McpStoreError> {
        validate_task_status(&task.status)?;
        if !(1..=86400).contains(&task.expires_in_seconds.unwrap_or(3600)) {
            return Err(McpStoreError::Invalid(
                "task expiry must be between 1 and 86400 seconds".to_string(),
            ));
        }
        let mut transaction = self.begin().await?;
        let task = sqlx::query_as::<_, PersistentTask>(
            "INSERT INTO hpa_control.mcp_tasks (
                 tenant_id, id, session_id, token_id, method, status,
                 input_json, output_json, error_json, approval_json,
                 execution_id, owner_node_id, created_at, updated_at, expires_at
             )
             VALUES (
                 $1, $2, $3, $4, $5, $6,
                 $7, $8, $9, $10,
                 $11, $12, now(), now(),
                 now() + make_interval(
                     secs => COALESCE($13, 3600)
                 )
             )
             ON CONFLICT (tenant_id, id) DO UPDATE
             SET session_id = COALESCE(EXCLUDED.session_id, hpa_control.mcp_tasks.session_id),
                 token_id = EXCLUDED.token_id,
                 method = EXCLUDED.method,
                 status = EXCLUDED.status,
                 input_json = EXCLUDED.input_json,
                 output_json = COALESCE(EXCLUDED.output_json, hpa_control.mcp_tasks.output_json),
                 error_json = COALESCE(EXCLUDED.error_json, hpa_control.mcp_tasks.error_json),
                 approval_json = COALESCE(
                     EXCLUDED.approval_json,
                     hpa_control.mcp_tasks.approval_json
                 ),
                 execution_id = COALESCE(
                     EXCLUDED.execution_id,
                     hpa_control.mcp_tasks.execution_id
                 ),
                 owner_node_id = COALESCE(
                     EXCLUDED.owner_node_id,
                     hpa_control.mcp_tasks.owner_node_id
                 ),
                 updated_at = now(),
                 expires_at = COALESCE(
                     EXCLUDED.expires_at,
                     hpa_control.mcp_tasks.expires_at
                 )
             RETURNING *",
        )
        .bind(&self.tenant_id)
        .bind(&task.id)
        .bind(&task.session_id)
        .bind(&self.token_id)
        .bind(&task.method)
        .bind(&task.status)
        .bind(Json(&task.input))
        .bind(task.output.as_ref().map(Json))
        .bind(task.error.as_ref().map(Json))
        .bind(task.approval.as_ref().map(Json))
        .bind(&task.execution_id)
        .bind(&task.owner_node_id)
        .bind(task.expires_in_seconds)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(task)
    }

    pub async fn get_task(&self, id: &str) -> Result<PersistentTask, McpStoreError> {
        let mut transaction = self.begin().await?;
        let task = sqlx::query_as::<_, PersistentTask>(
            "SELECT * FROM hpa_control.mcp_tasks
             WHERE id = $1
               AND token_id = $2
               AND (expires_at IS NULL OR expires_at > now())",
        )
        .bind(id)
        .bind(&self.token_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(|| McpStoreError::TaskNotFound(id.to_string()))?;
        transaction.commit().await?;
        Ok(task)
    }

    pub async fn get_task_optional(
        &self,
        id: &str,
    ) -> Result<Option<PersistentTask>, McpStoreError> {
        let mut transaction = self.begin().await?;
        let task = sqlx::query_as::<_, PersistentTask>(
            "SELECT * FROM hpa_control.mcp_tasks
             WHERE id = $1
               AND token_id = $2
               AND (expires_at IS NULL OR expires_at > now())",
        )
        .bind(id)
        .bind(&self.token_id)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(task)
    }

    pub async fn cancel_task(&self, id: &str) -> Result<PersistentTask, McpStoreError> {
        let mut transaction = self.begin().await?;
        let current: Option<String> = sqlx::query_scalar(
            "SELECT status FROM hpa_control.mcp_tasks
             WHERE id = $1 AND token_id = $2
             FOR UPDATE",
        )
        .bind(id)
        .bind(&self.token_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let current = current.ok_or_else(|| McpStoreError::TaskNotFound(id.to_string()))?;
        if matches!(
            current.as_str(),
            "completed" | "failed" | "cancelled" | "expired"
        ) {
            return Err(McpStoreError::InvalidTaskTransition {
                task_id: id.to_string(),
                current,
                requested: "cancelled".to_string(),
            });
        }
        let task = sqlx::query_as::<_, PersistentTask>(
            "UPDATE hpa_control.mcp_tasks
             SET status = 'cancelled', updated_at = now()
             WHERE id = $1 AND token_id = $2
             RETURNING *",
        )
        .bind(id)
        .bind(&self.token_id)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(task)
    }

    pub async fn request_task_cancel(&self, id: &str) -> Result<PersistentTask, McpStoreError> {
        let mut transaction = self.begin().await?;
        let row = sqlx::query(
            "SELECT status, owner_node_id
             FROM hpa_control.mcp_tasks
             WHERE tenant_id = $1 AND id = $2 AND token_id = $3
               AND (expires_at IS NULL OR expires_at > now())
             FOR UPDATE",
        )
        .bind(&self.tenant_id)
        .bind(id)
        .bind(&self.token_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(|| McpStoreError::TaskNotFound(id.to_string()))?;
        let status: String = row.get("status");
        if matches!(
            status.as_str(),
            "completed" | "failed" | "cancelled" | "expired"
        ) {
            return Err(McpStoreError::InvalidTaskTransition {
                task_id: id.to_string(),
                current: status,
                requested: "cancelled".to_string(),
            });
        }
        let owner_node_id: Option<String> = row.get("owner_node_id");
        let owner_node_id = owner_node_id.ok_or_else(|| {
            McpStoreError::Invalid(
                "task has no owner node; durable cancellation cannot be routed".to_string(),
            )
        })?;
        let update_id = format!("mcptaskupd_{}", Uuid::now_v7());
        sqlx::query(
            "INSERT INTO hpa_control.mcp_task_updates (
                 tenant_id, id, task_id, owner_node_id, token_id, action,
                 payload_json, status, attempts, available_at, created_at, updated_at
             )
             VALUES (
                 $1, $2, $3, $4, $5, 'cancel',
                 $6, 'pending', 0, now(), now(), now()
             )
             ON CONFLICT (tenant_id, task_id, action)
                 WHERE status IN ('pending', 'claimed')
             DO UPDATE SET
                 available_at = LEAST(
                     hpa_control.mcp_task_updates.available_at,
                     EXCLUDED.available_at
                 ),
                 updated_at = now()",
        )
        .bind(&self.tenant_id)
        .bind(update_id)
        .bind(id)
        .bind(owner_node_id)
        .bind(&self.token_id)
        .bind(Json(json!({"status": "cancelled"})))
        .execute(&mut *transaction)
        .await?;
        let task = sqlx::query_as::<_, PersistentTask>(
            "UPDATE hpa_control.mcp_tasks
             SET approval_json = jsonb_set(
                     COALESCE(approval_json, '{}'::jsonb),
                     '{cancelRequested}',
                     'true'::jsonb,
                     true
                 ),
                 updated_at = now()
             WHERE tenant_id = $1 AND id = $2 AND token_id = $3
             RETURNING *",
        )
        .bind(&self.tenant_id)
        .bind(id)
        .bind(&self.token_id)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(task)
    }

    pub async fn update_task_projection(
        &self,
        id: &str,
        status: &str,
        output: Option<&Value>,
        error: Option<&Value>,
        approval: Option<&Value>,
    ) -> Result<(PersistentTask, bool), McpStoreError> {
        validate_task_status(status)?;
        let mut transaction = self.begin().await?;
        let current: Option<String> = sqlx::query_scalar(
            "SELECT status FROM hpa_control.mcp_tasks
             WHERE tenant_id = $1 AND id = $2 AND token_id = $3
             FOR UPDATE",
        )
        .bind(&self.tenant_id)
        .bind(id)
        .bind(&self.token_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let current = current.ok_or_else(|| McpStoreError::TaskNotFound(id.to_string()))?;
        let task = sqlx::query_as::<_, PersistentTask>(
            "UPDATE hpa_control.mcp_tasks
             SET status = $4,
                 output_json = COALESCE($5, output_json),
                 error_json = COALESCE($6, error_json),
                 approval_json = CASE
                     WHEN approval_json->>'cancelRequested' = 'true'
                     THEN jsonb_set(
                         COALESCE($7, approval_json),
                         '{cancelRequested}',
                         'true'::jsonb,
                         true
                     )
                     ELSE COALESCE($7, approval_json)
                 END,
                 updated_at = now()
             WHERE tenant_id = $1 AND id = $2 AND token_id = $3
             RETURNING *",
        )
        .bind(&self.tenant_id)
        .bind(id)
        .bind(&self.token_id)
        .bind(status)
        .bind(output.map(Json))
        .bind(error.map(Json))
        .bind(approval.map(Json))
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok((task, current != status))
    }

    fn validate_ttl(&self, ttl_seconds: i32) -> Result<(), McpStoreError> {
        if (1..=86400).contains(&ttl_seconds) {
            Ok(())
        } else {
            Err(McpStoreError::Invalid(
                "MCP state ttl must be between 1 and 86400 seconds".to_string(),
            ))
        }
    }

    async fn begin(&self) -> Result<sqlx::Transaction<'_, sqlx::Postgres>, McpStoreError> {
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

fn enabled_notification_kinds(requested: &Value) -> Vec<String> {
    [
        ("tools", "toolsListChanged"),
        ("resources", "resourcesListChanged"),
        ("prompts", "promptsListChanged"),
        ("tasks", "tasksChanged"),
    ]
    .into_iter()
    .filter_map(|(kind, field)| {
        requested
            .get(field)
            .and_then(Value::as_bool)
            .filter(|enabled| *enabled)
            .map(|_| kind.to_string())
    })
    .collect()
}

impl PostgresMcpTaskNode {
    pub fn new(pool: PgPool, tenant_id: impl Into<String>, node_id: impl Into<String>) -> Self {
        Self {
            pool,
            tenant_id: tenant_id.into(),
            node_id: node_id.into(),
        }
    }

    pub async fn claim(&self, limit: i32) -> Result<Vec<TaskUpdateCommand>, McpStoreError> {
        if !(1..=100).contains(&limit) {
            return Err(McpStoreError::Invalid(
                "task update claim limit must be between 1 and 100".to_string(),
            ));
        }
        let mut transaction = self.begin().await?;
        let commands = sqlx::query_as::<_, TaskUpdateCommand>(
            "WITH candidates AS (
                 SELECT update_row.id
                 FROM hpa_control.mcp_task_updates update_row
                 JOIN hpa_control.mcp_tasks task
                   ON task.tenant_id = update_row.tenant_id
                  AND task.id = update_row.task_id
                 WHERE update_row.tenant_id = $1
                   AND update_row.owner_node_id = $2
                   AND (
                       (update_row.status = 'pending' AND update_row.available_at <= now())
                       OR (
                           update_row.status = 'claimed'
                           AND update_row.lease_until < now()
                       )
                   )
                 ORDER BY update_row.available_at, update_row.created_at
                 FOR UPDATE OF update_row SKIP LOCKED
                 LIMIT $3
             )
             UPDATE hpa_control.mcp_task_updates update_row
             SET status = 'claimed',
                 attempts = attempts + 1,
                 lease_until = now() + interval '30 seconds',
                 updated_at = now()
             FROM candidates, hpa_control.mcp_tasks task
             WHERE update_row.id = candidates.id
               AND update_row.tenant_id = $1
               AND task.tenant_id = update_row.tenant_id
               AND task.id = update_row.task_id
             RETURNING
                 update_row.tenant_id,
                 update_row.id,
                 update_row.task_id,
                 update_row.owner_node_id,
                 update_row.token_id,
                 update_row.action,
                 update_row.payload_json,
                 update_row.attempts,
                 task.execution_id,
                 task.status AS task_status",
        )
        .bind(&self.tenant_id)
        .bind(&self.node_id)
        .bind(limit)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(commands)
    }

    pub async fn owned_tasks(&self, limit: i32) -> Result<Vec<PersistentTask>, McpStoreError> {
        if !(1..=100).contains(&limit) {
            return Err(McpStoreError::Invalid(
                "owned task limit must be between 1 and 100".to_string(),
            ));
        }
        let mut transaction = self.begin().await?;
        let tasks = sqlx::query_as::<_, PersistentTask>(
            "SELECT *
             FROM hpa_control.mcp_tasks
             WHERE tenant_id = $1
               AND owner_node_id = $2
               AND status NOT IN ('completed', 'failed', 'cancelled', 'expired')
               AND (expires_at IS NULL OR expires_at > now())
             ORDER BY updated_at
             LIMIT $3",
        )
        .bind(&self.tenant_id)
        .bind(&self.node_id)
        .bind(limit)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(tasks)
    }

    pub async fn complete(&self, id: &str) -> Result<bool, McpStoreError> {
        let mut transaction = self.begin().await?;
        let updated = sqlx::query(
            "UPDATE hpa_control.mcp_task_updates
             SET status = 'applied', lease_until = NULL, last_error = NULL, updated_at = now()
             WHERE tenant_id = $1 AND id = $2 AND owner_node_id = $3
               AND status = 'claimed'",
        )
        .bind(&self.tenant_id)
        .bind(id)
        .bind(&self.node_id)
        .execute(&mut *transaction)
        .await?
        .rows_affected()
            == 1;
        transaction.commit().await?;
        Ok(updated)
    }

    pub async fn retry(
        &self,
        id: &str,
        error: &str,
        delay_seconds: i64,
    ) -> Result<bool, McpStoreError> {
        if !(1..=3600).contains(&delay_seconds) {
            return Err(McpStoreError::Invalid(
                "task update retry delay must be between 1 and 3600 seconds".to_string(),
            ));
        }
        let mut transaction = self.begin().await?;
        let updated = sqlx::query(
            "UPDATE hpa_control.mcp_task_updates
             SET status = CASE WHEN attempts >= 10 THEN 'failed' ELSE 'pending' END,
                 available_at = now() + make_interval(secs => $4),
                 lease_until = NULL,
                 last_error = $3,
                 updated_at = now()
             WHERE tenant_id = $1 AND id = $2 AND owner_node_id = $5
               AND status = 'claimed'",
        )
        .bind(&self.tenant_id)
        .bind(id)
        .bind(error)
        .bind(delay_seconds)
        .bind(&self.node_id)
        .execute(&mut *transaction)
        .await?
        .rows_affected()
            == 1;
        transaction.commit().await?;
        Ok(updated)
    }

    pub async fn fail(&self, id: &str, error: &str) -> Result<bool, McpStoreError> {
        let mut transaction = self.begin().await?;
        let updated = sqlx::query(
            "UPDATE hpa_control.mcp_task_updates
             SET status = 'failed', lease_until = NULL, last_error = $3, updated_at = now()
             WHERE tenant_id = $1 AND id = $2 AND owner_node_id = $4
               AND status = 'claimed'",
        )
        .bind(&self.tenant_id)
        .bind(id)
        .bind(error)
        .bind(&self.node_id)
        .execute(&mut *transaction)
        .await?
        .rows_affected()
            == 1;
        transaction.commit().await?;
        Ok(updated)
    }

    async fn begin(&self) -> Result<sqlx::Transaction<'_, sqlx::Postgres>, McpStoreError> {
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

fn validate_task_status(status: &str) -> Result<(), McpStoreError> {
    if [
        "working",
        "input_required",
        "completed",
        "failed",
        "cancelled",
        "expired",
    ]
    .contains(&status)
    {
        Ok(())
    } else {
        Err(McpStoreError::Invalid(format!(
            "unsupported MCP task status {status:?}"
        )))
    }
}
