use hpa_core::ir::Workflow;
use hpa_core::runtime::{ExecutionEvent, ExecutionState, ExecutionStatus};
use serde_json::Value;
use sqlx::Row;
use sqlx::postgres::{PgPool, PgRow};
use sqlx::types::Json;
use thiserror::Error;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::postgres_trigger::TriggerFireRecord;

#[derive(Debug, Error)]
pub enum PostgresExecutionError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid execution data: {0}")]
    Invalid(String),
    #[error("execution `{0}` was not found in PostgreSQL")]
    NotFound(String),
    #[error("workflow snapshot conflict for `{workflow_id}@{version}`")]
    WorkflowSnapshotConflict {
        workflow_id: String,
        version: String,
    },
    #[error("idempotency key `{key}` was reused with a different request")]
    IdempotencyConflict { key: String },
    #[error("execution tenant `{actual}` does not match database tenant `{expected}`")]
    TenantMismatch { expected: String, actual: String },
    #[error("event `{event_id}` does not target execution `{execution_id}`")]
    EventExecutionMismatch {
        event_id: String,
        execution_id: String,
    },
}

#[derive(Debug, Clone)]
pub struct ExecutionRecord {
    pub id: String,
    pub workflow_id: String,
    pub workflow_version: String,
    pub status: String,
    pub state_json: String,
    pub workflow_json: String,
    pub output_json: Option<String>,
    pub error_json: Option<String>,
    pub started_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct ArtifactRecord {
    pub id: String,
    pub execution_id: String,
    pub name: String,
    pub kind: String,
    pub mime_type: String,
    pub size: i64,
    pub content_json: String,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct WorkflowVersionRecord {
    pub version: String,
    pub registry_version: String,
    pub ir_json: String,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct WorkflowRecord {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub published: i64,
    pub ir_json: String,
    pub yaml: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct ExecutionLeaseRecord {
    pub holder_id: String,
    pub fencing_token: i64,
    pub expires_at: String,
}

#[derive(Clone)]
pub struct PostgresExecutionStore {
    pool: PgPool,
    tenant_id: String,
}

impl PostgresExecutionStore {
    pub fn new(pool: PgPool, tenant_id: impl Into<String>) -> Self {
        Self {
            pool,
            tenant_id: tenant_id.into(),
        }
    }

    pub async fn upsert_workflow(
        &self,
        workflow: &WorkflowRecord,
    ) -> Result<(), PostgresExecutionError> {
        let ir: Value = serde_json::from_str(&workflow.ir_json)?;
        let created_at = parse_time(&workflow.created_at)?;
        let updated_at = parse_time(&workflow.updated_at)?;
        let mut transaction = self.begin().await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.workflows (
                 tenant_id, id, name, version, description, published,
                 ir_json, yaml, created_at, updated_at
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             ON CONFLICT (tenant_id, id) DO UPDATE
             SET name = EXCLUDED.name,
                 version = EXCLUDED.version,
                 description = EXCLUDED.description,
                 published = EXCLUDED.published,
                 ir_json = EXCLUDED.ir_json,
                 yaml = EXCLUDED.yaml,
                 updated_at = EXCLUDED.updated_at",
        )
        .bind(&self.tenant_id)
        .bind(&workflow.id)
        .bind(&workflow.name)
        .bind(&workflow.version)
        .bind(&workflow.description)
        .bind(workflow.published != 0)
        .bind(Json(&ir))
        .bind(&workflow.yaml)
        .bind(created_at)
        .bind(updated_at)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn publish_workflow(
        &self,
        workflow: &WorkflowRecord,
        registry_version: &str,
    ) -> Result<(), PostgresExecutionError> {
        let ir: Value = serde_json::from_str(&workflow.ir_json)?;
        let updated_at = parse_time(&workflow.updated_at)?;
        let created_at = parse_time(&workflow.created_at)?;
        let mut transaction = self.begin().await?;
        let existing: Option<Value> = sqlx::query_scalar(
            "SELECT ir_json FROM hpa_tenant.workflow_versions
             WHERE tenant_id = $1 AND workflow_id = $2 AND version = $3",
        )
        .bind(&self.tenant_id)
        .bind(&workflow.id)
        .bind(&workflow.version)
        .fetch_optional(&mut *transaction)
        .await?;
        match existing {
            Some(stored) if stored != ir => {
                transaction.rollback().await?;
                return Err(PostgresExecutionError::WorkflowSnapshotConflict {
                    workflow_id: workflow.id.clone(),
                    version: workflow.version.clone(),
                });
            }
            Some(_) => {}
            None => {
                sqlx::query(
                    "INSERT INTO hpa_tenant.workflow_versions (
                         tenant_id, workflow_id, version, registry_version,
                         ir_json, created_at
                     )
                     VALUES ($1, $2, $3, $4, $5, $6)",
                )
                .bind(&self.tenant_id)
                .bind(&workflow.id)
                .bind(&workflow.version)
                .bind(registry_version)
                .bind(Json(&ir))
                .bind(created_at)
                .execute(&mut *transaction)
                .await?;
            }
        }
        sqlx::query(
            "INSERT INTO hpa_tenant.workflows (
                 tenant_id, id, name, version, description, published,
                 ir_json, yaml, created_at, updated_at
             )
             VALUES ($1, $2, $3, $4, $5, TRUE, $6, $7, $8, $9)
             ON CONFLICT (tenant_id, id) DO UPDATE
             SET name = EXCLUDED.name,
                 version = EXCLUDED.version,
                 description = EXCLUDED.description,
                 published = TRUE,
                 ir_json = EXCLUDED.ir_json,
                 yaml = EXCLUDED.yaml,
                 updated_at = EXCLUDED.updated_at",
        )
        .bind(&self.tenant_id)
        .bind(&workflow.id)
        .bind(&workflow.name)
        .bind(&workflow.version)
        .bind(&workflow.description)
        .bind(Json(&ir))
        .bind(&workflow.yaml)
        .bind(created_at)
        .bind(updated_at)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn list_workflows(&self) -> Result<Vec<WorkflowRecord>, PostgresExecutionError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT id, name, version, description, published,
                    ir_json, yaml, created_at, updated_at
             FROM hpa_tenant.workflows
             WHERE tenant_id = $1
             ORDER BY updated_at DESC",
        )
        .bind(&self.tenant_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.into_iter().map(workflow_from_row).collect()
    }

    pub async fn get_workflow(
        &self,
        id: &str,
    ) -> Result<Option<WorkflowRecord>, PostgresExecutionError> {
        let mut transaction = self.begin().await?;
        let row = sqlx::query(
            "SELECT id, name, version, description, published,
                    ir_json, yaml, created_at, updated_at
             FROM hpa_tenant.workflows
             WHERE tenant_id = $1 AND id = $2",
        )
        .bind(&self.tenant_id)
        .bind(id)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.map(workflow_from_row).transpose()
    }

    pub async fn list_versions(
        &self,
        workflow_id: &str,
    ) -> Result<Vec<WorkflowVersionRecord>, PostgresExecutionError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT version, registry_version, ir_json, created_at
             FROM hpa_tenant.workflow_versions
             WHERE tenant_id = $1 AND workflow_id = $2
             ORDER BY created_at DESC",
        )
        .bind(&self.tenant_id)
        .bind(workflow_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.into_iter().map(workflow_version_from_row).collect()
    }

    pub async fn workflow_count(&self) -> Result<i64, PostgresExecutionError> {
        let mut transaction = self.begin().await?;
        let count =
            sqlx::query_scalar("SELECT count(*) FROM hpa_tenant.workflows WHERE tenant_id = $1")
                .bind(&self.tenant_id)
                .fetch_one(&mut *transaction)
                .await?;
        transaction.commit().await?;
        Ok(count)
    }

    pub async fn insert_execution(
        &self,
        row: &ExecutionRecord,
        start_event: &ExecutionEvent,
    ) -> Result<(), PostgresExecutionError> {
        let workflow: Workflow = serde_json::from_str(&row.workflow_json)?;
        let workflow_json = serde_json::to_value(&workflow)?;
        let execution_state: ExecutionState = serde_json::from_str(&row.state_json)?;
        self.validate_state(&execution_state)?;
        self.validate_event(&execution_state.id, start_event)?;
        let state_json = serde_json::to_value(&execution_state)?;
        let output_json = row.output_json.as_deref().map(parse_value).transpose()?;
        let error_json = row.error_json.as_deref().map(parse_value).transpose()?;
        let started_at = parse_time(&row.started_at)?;
        let updated_at = parse_time(&row.updated_at)?;
        let mut transaction = self.begin().await?;

        sqlx::query(
            "INSERT INTO hpa_tenant.workflows (
                 tenant_id, id, name, version, description, published,
                 ir_json, yaml, created_at, updated_at
             )
             VALUES ($1, $2, $3, $4, $5, TRUE, $6, '', $7, $8)
             ON CONFLICT (tenant_id, id) DO NOTHING",
        )
        .bind(&self.tenant_id)
        .bind(&row.workflow_id)
        .bind(&workflow.name)
        .bind(&row.workflow_version)
        .bind(&workflow.description)
        .bind(Json(&workflow_json))
        .bind(started_at)
        .bind(updated_at)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.workflow_versions (
                 tenant_id, workflow_id, version, registry_version,
                 ir_json, created_at
             )
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (tenant_id, workflow_id, version) DO NOTHING",
        )
        .bind(&self.tenant_id)
        .bind(&row.workflow_id)
        .bind(&row.workflow_version)
        .bind(&execution_state.registry_version)
        .bind(Json(&workflow_json))
        .bind(started_at)
        .execute(&mut *transaction)
        .await?;

        let stored_workflow: Value = sqlx::query_scalar(
            "SELECT ir_json FROM hpa_tenant.workflow_versions
             WHERE tenant_id = $1 AND workflow_id = $2 AND version = $3",
        )
        .bind(&self.tenant_id)
        .bind(&row.workflow_id)
        .bind(&row.workflow_version)
        .fetch_one(&mut *transaction)
        .await?;
        if stored_workflow != workflow_json {
            transaction.rollback().await?;
            return Err(PostgresExecutionError::WorkflowSnapshotConflict {
                workflow_id: row.workflow_id.clone(),
                version: row.workflow_version.clone(),
            });
        }

        sqlx::query(
            "INSERT INTO hpa_tenant.executions (
                 tenant_id, id, workflow_id, workflow_version, status,
                 state_json, workflow_json, output_json, error_json,
                 started_at, updated_at
             )
             VALUES (
                 $1, $2, $3, $4, $5,
                 $6, $7, $8, $9,
                 $10, $11
             )",
        )
        .bind(&self.tenant_id)
        .bind(&row.id)
        .bind(&row.workflow_id)
        .bind(&row.workflow_version)
        .bind(&row.status)
        .bind(Json(&state_json))
        .bind(Json(&workflow_json))
        .bind(output_json.as_ref().map(Json))
        .bind(error_json.as_ref().map(Json))
        .bind(started_at)
        .bind(updated_at)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.events (
                 tenant_id, execution_id, sequence, payload_json
             )
             VALUES ($1, $2, $3, $4)",
        )
        .bind(&self.tenant_id)
        .bind(&row.id)
        .bind(start_event.sequence as i64)
        .bind(Json(serde_json::to_value(start_event)?))
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn insert_execution_idempotent(
        &self,
        row: &ExecutionRecord,
        start_event: &ExecutionEvent,
        idempotency_key: &str,
        request_hash: &str,
    ) -> Result<Option<ExecutionRecord>, PostgresExecutionError> {
        let mut lookup = self.begin().await?;
        let existing: Option<(String, String)> = sqlx::query_as(
            "SELECT request_hash, execution_id
             FROM hpa_tenant.execution_idempotency
             WHERE tenant_id = $1 AND idempotency_key = $2",
        )
        .bind(&self.tenant_id)
        .bind(idempotency_key)
        .fetch_optional(&mut *lookup)
        .await?;
        lookup.commit().await?;
        if let Some((existing_hash, execution_id)) = existing {
            if existing_hash != request_hash {
                return Err(PostgresExecutionError::IdempotencyConflict {
                    key: idempotency_key.to_string(),
                });
            }
            return self.get_execution(&execution_id).await;
        }

        let workflow: Workflow = serde_json::from_str(&row.workflow_json)?;
        let workflow_json = serde_json::to_value(&workflow)?;
        let execution_state: ExecutionState = serde_json::from_str(&row.state_json)?;
        self.validate_state(&execution_state)?;
        self.validate_event(&execution_state.id, start_event)?;
        let state_json = serde_json::to_value(&execution_state)?;
        let output_json = row.output_json.as_deref().map(parse_value).transpose()?;
        let error_json = row.error_json.as_deref().map(parse_value).transpose()?;
        let started_at = parse_time(&row.started_at)?;
        let updated_at = parse_time(&row.updated_at)?;
        let mut transaction = self.begin().await?;

        sqlx::query(
            "INSERT INTO hpa_tenant.workflows (
                 tenant_id, id, name, version, description, published,
                 ir_json, yaml, created_at, updated_at
             )
             VALUES ($1, $2, $3, $4, $5, TRUE, $6, '', $7, $8)
             ON CONFLICT (tenant_id, id) DO NOTHING",
        )
        .bind(&self.tenant_id)
        .bind(&row.workflow_id)
        .bind(&workflow.name)
        .bind(&row.workflow_version)
        .bind(&workflow.description)
        .bind(Json(&workflow_json))
        .bind(started_at)
        .bind(updated_at)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.workflow_versions (
                 tenant_id, workflow_id, version, registry_version,
                 ir_json, created_at
             )
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (tenant_id, workflow_id, version) DO NOTHING",
        )
        .bind(&self.tenant_id)
        .bind(&row.workflow_id)
        .bind(&row.workflow_version)
        .bind(&execution_state.registry_version)
        .bind(Json(&workflow_json))
        .bind(started_at)
        .execute(&mut *transaction)
        .await?;
        let stored_workflow: Value = sqlx::query_scalar(
            "SELECT ir_json FROM hpa_tenant.workflow_versions
             WHERE tenant_id = $1 AND workflow_id = $2 AND version = $3",
        )
        .bind(&self.tenant_id)
        .bind(&row.workflow_id)
        .bind(&row.workflow_version)
        .fetch_one(&mut *transaction)
        .await?;
        if stored_workflow != workflow_json {
            transaction.rollback().await?;
            return Err(PostgresExecutionError::WorkflowSnapshotConflict {
                workflow_id: row.workflow_id.clone(),
                version: row.workflow_version.clone(),
            });
        }
        sqlx::query(
            "INSERT INTO hpa_tenant.executions (
                 tenant_id, id, workflow_id, workflow_version, status,
                 state_json, workflow_json, output_json, error_json,
                 started_at, updated_at
             )
             VALUES (
                 $1, $2, $3, $4, $5,
                 $6, $7, $8, $9,
                 $10, $11
             )",
        )
        .bind(&self.tenant_id)
        .bind(&row.id)
        .bind(&row.workflow_id)
        .bind(&row.workflow_version)
        .bind(&row.status)
        .bind(Json(&state_json))
        .bind(Json(&workflow_json))
        .bind(output_json.as_ref().map(Json))
        .bind(error_json.as_ref().map(Json))
        .bind(started_at)
        .bind(updated_at)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.events (
                 tenant_id, execution_id, sequence, payload_json
             )
             VALUES ($1, $2, $3, $4)",
        )
        .bind(&self.tenant_id)
        .bind(&row.id)
        .bind(start_event.sequence as i64)
        .bind(Json(serde_json::to_value(start_event)?))
        .execute(&mut *transaction)
        .await?;
        let inserted = sqlx::query(
            "INSERT INTO hpa_tenant.execution_idempotency (
                 tenant_id, idempotency_key, request_hash, execution_id, created_at
             )
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (tenant_id, idempotency_key) DO NOTHING",
        )
        .bind(&self.tenant_id)
        .bind(idempotency_key)
        .bind(request_hash)
        .bind(&row.id)
        .bind(started_at)
        .execute(&mut *transaction)
        .await?
        .rows_affected()
            == 1;
        if inserted {
            transaction.commit().await?;
            return Ok(None);
        }
        transaction.rollback().await?;

        let existing: Option<(String, String)> = sqlx::query_as(
            "SELECT request_hash, execution_id
             FROM hpa_tenant.execution_idempotency
             WHERE tenant_id = $1 AND idempotency_key = $2",
        )
        .bind(&self.tenant_id)
        .bind(idempotency_key)
        .fetch_optional(&self.pool)
        .await?;
        let Some((existing_hash, execution_id)) = existing else {
            return Err(PostgresExecutionError::Database(sqlx::Error::RowNotFound));
        };
        if existing_hash != request_hash {
            return Err(PostgresExecutionError::IdempotencyConflict {
                key: idempotency_key.to_string(),
            });
        }
        self.get_execution(&execution_id).await
    }

    pub async fn request_execution_cancel(
        &self,
        execution_id: &str,
        requested_at: &str,
    ) -> Result<bool, PostgresExecutionError> {
        let requested_at = parse_time(requested_at)?;
        let mut transaction = self.begin().await?;
        let status: Option<String> = sqlx::query_scalar(
            "SELECT status FROM hpa_tenant.executions
             WHERE tenant_id = $1 AND id = $2",
        )
        .bind(&self.tenant_id)
        .bind(execution_id)
        .fetch_optional(&mut *transaction)
        .await?;
        if !status.is_some_and(|status| matches!(status.as_str(), "running" | "waiting_approval")) {
            transaction.rollback().await?;
            return Ok(false);
        }
        sqlx::query(
            "INSERT INTO hpa_tenant.execution_cancel_requests (
                 tenant_id, execution_id, requested_at
             )
             VALUES ($1, $2, $3)
             ON CONFLICT (tenant_id, execution_id) DO NOTHING",
        )
        .bind(&self.tenant_id)
        .bind(execution_id)
        .bind(requested_at)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(true)
    }

    pub async fn execution_cancel_requested(
        &self,
        execution_id: &str,
    ) -> Result<bool, PostgresExecutionError> {
        let mut transaction = self.begin().await?;
        let requested: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                 SELECT 1 FROM hpa_tenant.execution_cancel_requests
                 WHERE tenant_id = $1 AND execution_id = $2
             )",
        )
        .bind(&self.tenant_id)
        .bind(execution_id)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(requested)
    }

    pub async fn execution_idempotency_exists(
        &self,
        idempotency_key: &str,
    ) -> Result<bool, PostgresExecutionError> {
        let mut transaction = self.begin().await?;
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                 SELECT 1 FROM hpa_tenant.execution_idempotency
                 WHERE tenant_id = $1 AND idempotency_key = $2
             )",
        )
        .bind(&self.tenant_id)
        .bind(idempotency_key)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(exists)
    }

    pub async fn insert_execution_with_trigger(
        &self,
        row: &ExecutionRecord,
        start_event: &ExecutionEvent,
        fire: &TriggerFireRecord,
    ) -> Result<bool, PostgresExecutionError> {
        let workflow: Workflow = serde_json::from_str(&row.workflow_json)?;
        let workflow_json = serde_json::to_value(&workflow)?;
        let execution_state: ExecutionState = serde_json::from_str(&row.state_json)?;
        self.validate_state(&execution_state)?;
        self.validate_event(&execution_state.id, start_event)?;
        let state_json = serde_json::to_value(&execution_state)?;
        let output_json = row.output_json.as_deref().map(parse_value).transpose()?;
        let error_json = row.error_json.as_deref().map(parse_value).transpose()?;
        let started_at = parse_time(&row.started_at)?;
        let updated_at = parse_time(&row.updated_at)?;
        let fired_at = parse_time(&fire.fired_at)?;
        let scheduled_at = fire.scheduled_at.as_deref().map(parse_time).transpose()?;
        let detail: Option<Value> = fire
            .detail_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?;
        let mut transaction = self.begin().await?;

        sqlx::query(
            "INSERT INTO hpa_tenant.workflows (
                 tenant_id, id, name, version, description, published,
                 ir_json, yaml, created_at, updated_at
             )
             VALUES ($1, $2, $3, $4, $5, TRUE, $6, '', $7, $8)
             ON CONFLICT (tenant_id, id) DO NOTHING",
        )
        .bind(&self.tenant_id)
        .bind(&row.workflow_id)
        .bind(&workflow.name)
        .bind(&row.workflow_version)
        .bind(&workflow.description)
        .bind(Json(&workflow_json))
        .bind(started_at)
        .bind(updated_at)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.workflow_versions (
                 tenant_id, workflow_id, version, registry_version,
                 ir_json, created_at
             )
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (tenant_id, workflow_id, version) DO NOTHING",
        )
        .bind(&self.tenant_id)
        .bind(&row.workflow_id)
        .bind(&row.workflow_version)
        .bind(&execution_state.registry_version)
        .bind(Json(&workflow_json))
        .bind(started_at)
        .execute(&mut *transaction)
        .await?;
        let stored_workflow: Value = sqlx::query_scalar(
            "SELECT ir_json FROM hpa_tenant.workflow_versions
             WHERE tenant_id = $1 AND workflow_id = $2 AND version = $3",
        )
        .bind(&self.tenant_id)
        .bind(&row.workflow_id)
        .bind(&row.workflow_version)
        .fetch_one(&mut *transaction)
        .await?;
        if stored_workflow != workflow_json {
            transaction.rollback().await?;
            return Err(PostgresExecutionError::WorkflowSnapshotConflict {
                workflow_id: row.workflow_id.clone(),
                version: row.workflow_version.clone(),
            });
        }
        sqlx::query(
            "INSERT INTO hpa_tenant.executions (
                 tenant_id, id, workflow_id, workflow_version, status,
                 state_json, workflow_json, output_json, error_json,
                 started_at, updated_at
             )
             VALUES (
                 $1, $2, $3, $4, $5,
                 $6, $7, $8, $9,
                 $10, $11
             )",
        )
        .bind(&self.tenant_id)
        .bind(&row.id)
        .bind(&row.workflow_id)
        .bind(&row.workflow_version)
        .bind(&row.status)
        .bind(Json(&state_json))
        .bind(Json(&workflow_json))
        .bind(output_json.as_ref().map(Json))
        .bind(error_json.as_ref().map(Json))
        .bind(started_at)
        .bind(updated_at)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.events (
                 tenant_id, execution_id, sequence, payload_json
             )
             VALUES ($1, $2, $3, $4)",
        )
        .bind(&self.tenant_id)
        .bind(&row.id)
        .bind(start_event.sequence as i64)
        .bind(Json(serde_json::to_value(start_event)?))
        .execute(&mut *transaction)
        .await?;
        let inserted: bool = sqlx::query_scalar(
            "INSERT INTO hpa_tenant.trigger_fires (
                 tenant_id, id, workflow_id, trigger_id, kind, fired_at,
                 scheduled_at, execution_id, status, event_id, detail_json
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
             ON CONFLICT (tenant_id, trigger_id, event_id)
                 WHERE event_id IS NOT NULL
             DO NOTHING
             RETURNING true",
        )
        .bind(&self.tenant_id)
        .bind(&fire.id)
        .bind(&fire.workflow_id)
        .bind(&fire.trigger_id)
        .bind(&fire.kind)
        .bind(fired_at)
        .bind(scheduled_at)
        .bind(&fire.execution_id)
        .bind(&fire.status)
        .bind(&fire.event_id)
        .bind(detail.as_ref().map(Json))
        .fetch_optional(&mut *transaction)
        .await?
        .unwrap_or(false);
        if !inserted {
            transaction.rollback().await?;
            return Ok(false);
        }
        transaction.commit().await?;
        Ok(true)
    }

    pub async fn sync_workflow(
        &self,
        workflow_json: &str,
        version: &str,
        registry_version: &str,
        created_at: &str,
        updated_at: &str,
    ) -> Result<(), PostgresExecutionError> {
        let workflow: Workflow = serde_json::from_str(workflow_json)?;
        let workflow_json = serde_json::to_value(&workflow)?;
        let created_at = parse_time(created_at)?;
        let updated_at = parse_time(updated_at)?;
        let mut transaction = self.begin().await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.workflows (
                 tenant_id, id, name, version, description, published,
                 ir_json, yaml, created_at, updated_at
             )
             VALUES ($1, $2, $3, $4, $5, TRUE, $6, '', $7, $8)
             ON CONFLICT (tenant_id, id) DO NOTHING",
        )
        .bind(&self.tenant_id)
        .bind(&workflow.id)
        .bind(&workflow.name)
        .bind(version)
        .bind(&workflow.description)
        .bind(Json(&workflow_json))
        .bind(created_at)
        .bind(updated_at)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.workflow_versions (
                 tenant_id, workflow_id, version, registry_version,
                 ir_json, created_at
             )
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (tenant_id, workflow_id, version) DO UPDATE
             SET registry_version = CASE
                 WHEN hpa_tenant.workflow_versions.registry_version = ''
                 THEN EXCLUDED.registry_version
                 ELSE hpa_tenant.workflow_versions.registry_version
             END",
        )
        .bind(&self.tenant_id)
        .bind(&workflow.id)
        .bind(version)
        .bind(registry_version)
        .bind(Json(&workflow_json))
        .bind(created_at)
        .execute(&mut *transaction)
        .await?;
        let stored_workflow: Value = sqlx::query_scalar(
            "SELECT ir_json FROM hpa_tenant.workflow_versions
             WHERE tenant_id = $1 AND workflow_id = $2 AND version = $3",
        )
        .bind(&self.tenant_id)
        .bind(&workflow.id)
        .bind(version)
        .fetch_one(&mut *transaction)
        .await?;
        if stored_workflow != workflow_json {
            transaction.rollback().await?;
            return Err(PostgresExecutionError::WorkflowSnapshotConflict {
                workflow_id: workflow.id,
                version: version.to_string(),
            });
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn save_execution(
        &self,
        state: &ExecutionState,
        events: &[ExecutionEvent],
    ) -> Result<(), PostgresExecutionError> {
        self.validate_state(state)?;
        for event in events {
            self.validate_event(&state.id, event)?;
        }
        let state_json = serde_json::to_value(state)?;
        let output_json = state
            .output
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?;
        let error_json = state.error.as_ref().map(serde_json::to_value).transpose()?;
        let updated_at = parse_time(&state.updated_at)?;
        let mut transaction = self.begin().await?;
        let updated = sqlx::query(
            "UPDATE hpa_tenant.executions
             SET status = $4,
                 state_json = $5,
                 output_json = $6,
                 error_json = $7,
                 updated_at = $8
             WHERE tenant_id = $1 AND id = $2 AND workflow_id = $3",
        )
        .bind(&self.tenant_id)
        .bind(&state.id)
        .bind(&state.workflow_id)
        .bind(status_text(state.status))
        .bind(Json(&state_json))
        .bind(output_json.as_ref().map(Json))
        .bind(error_json.as_ref().map(Json))
        .bind(updated_at)
        .execute(&mut *transaction)
        .await?;
        if updated.rows_affected() != 1 {
            transaction.rollback().await?;
            return Err(PostgresExecutionError::NotFound(state.id.clone()));
        }
        for event in events {
            sqlx::query(
                "INSERT INTO hpa_tenant.events (
                     tenant_id, execution_id, sequence, payload_json
                 )
                 VALUES ($1, $2, $3, $4)
                 ON CONFLICT (tenant_id, execution_id, sequence) DO NOTHING",
            )
            .bind(&self.tenant_id)
            .bind(&state.id)
            .bind(event.sequence as i64)
            .bind(Json(serde_json::to_value(event)?))
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn get_execution(
        &self,
        id: &str,
    ) -> Result<Option<ExecutionRecord>, PostgresExecutionError> {
        let mut transaction = self.begin().await?;
        let row = sqlx::query(
            "SELECT id, workflow_id, workflow_version, status,
                    state_json, workflow_json, output_json, error_json,
                    started_at, updated_at
             FROM hpa_tenant.executions
             WHERE tenant_id = $1 AND id = $2",
        )
        .bind(&self.tenant_id)
        .bind(id)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.map(execution_from_row).transpose()
    }

    pub async fn list_executions(
        &self,
        limit: i64,
    ) -> Result<Vec<ExecutionRecord>, PostgresExecutionError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT id, workflow_id, workflow_version, status,
                    state_json, workflow_json, output_json, error_json,
                    started_at, updated_at
             FROM hpa_tenant.executions
             WHERE tenant_id = $1
             ORDER BY started_at DESC
             LIMIT $2",
        )
        .bind(&self.tenant_id)
        .bind(limit)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.into_iter().map(execution_from_row).collect()
    }

    /// Paged listing for the console: status filter plus stable ordering.
    pub async fn list_executions_page(
        &self,
        limit: i64,
        offset: i64,
        status: Option<&str>,
    ) -> Result<Vec<ExecutionRecord>, PostgresExecutionError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT id, workflow_id, workflow_version, status,
                    state_json, workflow_json, output_json, error_json,
                    started_at, updated_at
             FROM hpa_tenant.executions
             WHERE tenant_id = $1
               AND ($2::text IS NULL OR status = $2)
             ORDER BY started_at DESC, id DESC
             LIMIT $3 OFFSET $4",
        )
        .bind(&self.tenant_id)
        .bind(status)
        .bind(limit)
        .bind(offset)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.into_iter().map(execution_from_row).collect()
    }

    pub async fn count_executions(
        &self,
        status: Option<&str>,
    ) -> Result<i64, PostgresExecutionError> {
        let mut transaction = self.begin().await?;
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM hpa_tenant.executions
             WHERE tenant_id = $1 AND ($2::text IS NULL OR status = $2)",
        )
        .bind(&self.tenant_id)
        .bind(status)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(count)
    }

    pub async fn running_executions(&self) -> Result<Vec<ExecutionRecord>, PostgresExecutionError> {
        self.executions_with_status("running", true).await
    }

    pub async fn waiting_executions(&self) -> Result<Vec<ExecutionRecord>, PostgresExecutionError> {
        self.executions_with_status("waiting_approval", false).await
    }

    pub async fn waiting_local_executions(
        &self,
    ) -> Result<Vec<ExecutionRecord>, PostgresExecutionError> {
        self.executions_with_status("waiting_approval", true).await
    }

    async fn executions_with_status(
        &self,
        status: &str,
        exclude_remote: bool,
    ) -> Result<Vec<ExecutionRecord>, PostgresExecutionError> {
        let mut transaction = self.begin().await?;
        let query = if exclude_remote {
            "SELECT execution.id, execution.workflow_id, execution.workflow_version,
                    execution.status, execution.state_json, execution.workflow_json,
                    execution.output_json, execution.error_json,
                    execution.started_at, execution.updated_at
             FROM hpa_tenant.executions execution
             WHERE execution.tenant_id = $1
               AND execution.status = $2
               AND NOT EXISTS (
                   SELECT 1 FROM hpa_tenant.remote_executions remote
                   WHERE remote.tenant_id = execution.tenant_id
                     AND remote.execution_id = execution.id
               )
             ORDER BY execution.started_at ASC"
        } else {
            "SELECT id, workflow_id, workflow_version, status,
                    state_json, workflow_json, output_json, error_json,
                    started_at, updated_at
             FROM hpa_tenant.executions
             WHERE tenant_id = $1 AND status = $2
             ORDER BY started_at ASC"
        };
        let rows = sqlx::query(query)
            .bind(&self.tenant_id)
            .bind(status)
            .fetch_all(&mut *transaction)
            .await?;
        transaction.commit().await?;
        rows.into_iter().map(execution_from_row).collect()
    }

    pub async fn list_events(
        &self,
        execution_id: &str,
        after: i64,
        limit: i64,
    ) -> Result<Vec<ExecutionEvent>, PostgresExecutionError> {
        let mut transaction = self.begin().await?;
        let payloads: Vec<Value> = sqlx::query_scalar(
            "SELECT payload_json
             FROM hpa_tenant.events
             WHERE tenant_id = $1 AND execution_id = $2 AND sequence > $3
             ORDER BY sequence
             LIMIT $4",
        )
        .bind(&self.tenant_id)
        .bind(execution_id)
        .bind(after)
        .bind(limit)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        payloads
            .into_iter()
            .map(|value| serde_json::from_value(value).map_err(PostgresExecutionError::from))
            .collect()
    }

    pub async fn save_output_artifact(
        &self,
        state: &ExecutionState,
    ) -> Result<(), PostgresExecutionError> {
        let Some(output) = &state.output else {
            return Ok(());
        };
        let output = serde_json::to_value(output)?;
        let size = serde_json::to_vec(&output)?.len() as i64;
        let created_at = parse_time(&state.updated_at)?;
        let mut transaction = self.begin().await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.artifacts (
                 tenant_id, id, execution_id, name, kind, mime_type,
                 size, content_json, created_at
             )
             VALUES ($1, $2, $3, 'output.json', 'execution-output',
                     'application/json', $4, $5, $6)
             ON CONFLICT (tenant_id, id) DO UPDATE
             SET size = EXCLUDED.size,
                 content_json = EXCLUDED.content_json,
                 created_at = EXCLUDED.created_at",
        )
        .bind(&self.tenant_id)
        .bind(format!("{}:output", state.id))
        .bind(&state.id)
        .bind(size)
        .bind(Json(&output))
        .bind(created_at)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn list_artifacts(
        &self,
        execution_id: Option<&str>,
    ) -> Result<Vec<ArtifactRecord>, PostgresExecutionError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT id, execution_id, name, kind, mime_type, size,
                    content_json, created_at
             FROM hpa_tenant.artifacts
             WHERE tenant_id = $1
               AND ($2::text IS NULL OR execution_id = $2)
             ORDER BY created_at DESC",
        )
        .bind(&self.tenant_id)
        .bind(execution_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.into_iter().map(artifact_from_row).collect()
    }

    pub async fn get_artifact(
        &self,
        id: &str,
    ) -> Result<Option<ArtifactRecord>, PostgresExecutionError> {
        let mut transaction = self.begin().await?;
        let row = sqlx::query(
            "SELECT id, execution_id, name, kind, mime_type, size,
                    content_json, created_at
             FROM hpa_tenant.artifacts
             WHERE tenant_id = $1 AND id = $2",
        )
        .bind(&self.tenant_id)
        .bind(id)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.map(artifact_from_row).transpose()
    }

    pub async fn execution_counts(
        &self,
    ) -> Result<(i64, i64, i64, i64, i64), PostgresExecutionError> {
        let mut transaction = self.begin().await?;
        let executions =
            sqlx::query_scalar("SELECT count(*) FROM hpa_tenant.executions WHERE tenant_id = $1")
                .bind(&self.tenant_id)
                .fetch_one(&mut *transaction)
                .await?;
        let status_counts = sqlx::query_as::<_, (String, i64)>(
            "SELECT status, count(*)
             FROM hpa_tenant.executions
             WHERE tenant_id = $1
             GROUP BY status",
        )
        .bind(&self.tenant_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        let status = |name: &str| {
            status_counts
                .iter()
                .find(|(status, _)| status == name)
                .map(|(_, count)| *count)
                .unwrap_or_default()
        };
        Ok((
            executions,
            status("running"),
            status("waiting_approval"),
            status("succeeded"),
            status("failed"),
        ))
    }

    pub async fn execution_count_since(
        &self,
        date_prefix: &str,
    ) -> Result<i64, PostgresExecutionError> {
        let mut transaction = self.begin().await?;
        let count = sqlx::query_scalar(
            "SELECT count(*)
             FROM hpa_tenant.executions
             WHERE tenant_id = $1
               AND started_at >= ($2::date AT TIME ZONE 'UTC')",
        )
        .bind(&self.tenant_id)
        .bind(date_prefix)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(count)
    }

    pub async fn get_workflow_version(
        &self,
        workflow_id: &str,
        version: &str,
    ) -> Result<Option<WorkflowVersionRecord>, PostgresExecutionError> {
        let mut transaction = self.begin().await?;
        let row = sqlx::query(
            "SELECT version, registry_version, ir_json, created_at
             FROM hpa_tenant.workflow_versions
             WHERE tenant_id = $1 AND workflow_id = $2 AND version = $3",
        )
        .bind(&self.tenant_id)
        .bind(workflow_id)
        .bind(version)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.map(workflow_version_from_row).transpose()
    }

    pub async fn acquire_execution_lease(
        &self,
        execution_id: &str,
        holder_id: &str,
        ttl_seconds: i32,
    ) -> Result<Option<ExecutionLeaseRecord>, PostgresExecutionError> {
        self.validate_lease_ttl(ttl_seconds)?;
        let mut transaction = self.begin().await?;
        let row = sqlx::query(
            "INSERT INTO hpa_tenant.execution_leases (
                 tenant_id, execution_id, holder_id, fencing_token,
                 acquired_at, renewed_at, expires_at
             )
             VALUES (
                 $1, $2, $3, 1, now(), now(),
                 now() + make_interval(secs => $4)
             )
             ON CONFLICT (tenant_id, execution_id) DO UPDATE
             SET holder_id = EXCLUDED.holder_id,
                 fencing_token =
                     hpa_tenant.execution_leases.fencing_token + 1,
                 acquired_at = now(),
                 renewed_at = now(),
                 expires_at =
                     now() + make_interval(secs => $4)
             WHERE hpa_tenant.execution_leases.expires_at <= now()
             RETURNING holder_id, fencing_token, expires_at",
        )
        .bind(&self.tenant_id)
        .bind(execution_id)
        .bind(holder_id)
        .bind(ttl_seconds)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.map(lease_from_row).transpose()
    }

    pub async fn renew_execution_lease(
        &self,
        execution_id: &str,
        holder_id: &str,
        fencing_token: i64,
        ttl_seconds: i32,
    ) -> Result<bool, PostgresExecutionError> {
        self.validate_lease_ttl(ttl_seconds)?;
        let mut transaction = self.begin().await?;
        let renewed = sqlx::query(
            "UPDATE hpa_tenant.execution_leases
             SET renewed_at = now(),
                 expires_at = now() + make_interval(secs => $5)
             WHERE tenant_id = $1
               AND execution_id = $2
               AND holder_id = $3
               AND fencing_token = $4
               AND expires_at > now()",
        )
        .bind(&self.tenant_id)
        .bind(execution_id)
        .bind(holder_id)
        .bind(fencing_token)
        .bind(ttl_seconds)
        .execute(&mut *transaction)
        .await?
        .rows_affected()
            == 1;
        transaction.commit().await?;
        Ok(renewed)
    }

    pub async fn release_execution_lease(
        &self,
        execution_id: &str,
        holder_id: &str,
        fencing_token: i64,
    ) -> Result<bool, PostgresExecutionError> {
        let mut transaction = self.begin().await?;
        let released = sqlx::query(
            "DELETE FROM hpa_tenant.execution_leases
             WHERE tenant_id = $1
               AND execution_id = $2
               AND holder_id = $3
               AND fencing_token = $4",
        )
        .bind(&self.tenant_id)
        .bind(execution_id)
        .bind(holder_id)
        .bind(fencing_token)
        .execute(&mut *transaction)
        .await?
        .rows_affected()
            == 1;
        transaction.commit().await?;
        Ok(released)
    }

    pub async fn save_execution_fenced(
        &self,
        state: &ExecutionState,
        events: &[ExecutionEvent],
        holder_id: &str,
        fencing_token: i64,
    ) -> Result<(), PostgresExecutionError> {
        self.validate_state(state)?;
        for event in events {
            self.validate_event(&state.id, event)?;
        }
        let state_json = serde_json::to_value(state)?;
        let output_json = state
            .output
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?;
        let error_json = state.error.as_ref().map(serde_json::to_value).transpose()?;
        let updated_at = parse_time(&state.updated_at)?;
        let mut transaction = self.begin().await?;
        let updated = sqlx::query(
            "UPDATE hpa_tenant.executions execution
             SET status = $4,
                 state_json = $5,
                 output_json = $6,
                 error_json = $7,
                 updated_at = $8
             WHERE execution.tenant_id = $1
               AND execution.id = $2
               AND execution.workflow_id = $3
               AND EXISTS (
                   SELECT 1
                   FROM hpa_tenant.execution_leases lease
                   WHERE lease.tenant_id = execution.tenant_id
                     AND lease.execution_id = execution.id
                     AND lease.holder_id = $9
                     AND lease.fencing_token = $10
                     AND lease.expires_at > now()
               )",
        )
        .bind(&self.tenant_id)
        .bind(&state.id)
        .bind(&state.workflow_id)
        .bind(status_text(state.status))
        .bind(Json(&state_json))
        .bind(output_json.as_ref().map(Json))
        .bind(error_json.as_ref().map(Json))
        .bind(updated_at)
        .bind(holder_id)
        .bind(fencing_token)
        .execute(&mut *transaction)
        .await?;
        if updated.rows_affected() != 1 {
            transaction.rollback().await?;
            return Err(PostgresExecutionError::Invalid(
                "execution lease was lost or expired before state commit".to_string(),
            ));
        }
        for event in events {
            sqlx::query(
                "INSERT INTO hpa_tenant.events (
                     tenant_id, execution_id, sequence, payload_json
                 )
                 VALUES ($1, $2, $3, $4)
                 ON CONFLICT (tenant_id, execution_id, sequence) DO NOTHING",
            )
            .bind(&self.tenant_id)
            .bind(&state.id)
            .bind(event.sequence as i64)
            .bind(Json(serde_json::to_value(event)?))
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn active_execution_count(&self) -> Result<i64, PostgresExecutionError> {
        let mut transaction = self.begin().await?;
        let count = sqlx::query_scalar(
            "SELECT count(*)
             FROM hpa_tenant.executions
             WHERE tenant_id = $1
               AND status IN ('running', 'waiting_approval')",
        )
        .bind(&self.tenant_id)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(count)
    }

    pub async fn mark_remote_execution(
        &self,
        execution_id: &str,
        worker_id: &str,
    ) -> Result<(), PostgresExecutionError> {
        let now_ms = OffsetDateTime::now_utc().unix_timestamp() * 1000;
        let mut transaction = self.begin().await?;
        sqlx::query(
            "INSERT INTO hpa_control.workers (
                 id, tenant_id, name, kind, status, capabilities_json,
                 max_concurrency, runtime_version, last_heartbeat_at,
                 registered_at, metadata_json
             )
             VALUES (
                 $1, $2, $1, 'workflow', 'active', '[\"workflow\"]'::jsonb,
                 1, 'unknown', $3, $3, '{\"source\":\"execution-runtime\"}'::jsonb
             )
             ON CONFLICT (id) DO UPDATE
             SET last_heartbeat_at = EXCLUDED.last_heartbeat_at
             WHERE hpa_control.workers.tenant_id = EXCLUDED.tenant_id",
        )
        .bind(worker_id)
        .bind(&self.tenant_id)
        .bind(now_ms)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.remote_executions (
                 tenant_id, execution_id, worker_id, created_at
             )
             VALUES ($1, $2, $3, now())
             ON CONFLICT (tenant_id, execution_id) DO NOTHING",
        )
        .bind(&self.tenant_id)
        .bind(execution_id)
        .bind(worker_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn is_remote_execution(
        &self,
        execution_id: &str,
    ) -> Result<bool, PostgresExecutionError> {
        let mut transaction = self.begin().await?;
        let found: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                 SELECT 1 FROM hpa_tenant.remote_executions
                 WHERE tenant_id = $1 AND execution_id = $2
             )",
        )
        .bind(&self.tenant_id)
        .bind(execution_id)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(found)
    }

    async fn begin(&self) -> Result<sqlx::Transaction<'_, sqlx::Postgres>, PostgresExecutionError> {
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

    fn validate_state(&self, state: &ExecutionState) -> Result<(), PostgresExecutionError> {
        if state.tenant_id != self.tenant_id {
            return Err(PostgresExecutionError::TenantMismatch {
                expected: self.tenant_id.clone(),
                actual: state.tenant_id.clone(),
            });
        }
        Ok(())
    }

    fn validate_event(
        &self,
        execution_id: &str,
        event: &ExecutionEvent,
    ) -> Result<(), PostgresExecutionError> {
        if event.execution_id != execution_id {
            return Err(PostgresExecutionError::EventExecutionMismatch {
                event_id: event.id.clone(),
                execution_id: event.execution_id.clone(),
            });
        }
        Ok(())
    }

    fn validate_lease_ttl(&self, ttl_seconds: i32) -> Result<(), PostgresExecutionError> {
        if (1..=3600).contains(&ttl_seconds) {
            Ok(())
        } else {
            Err(PostgresExecutionError::Invalid(
                "execution lease ttl must be between 1 and 3600 seconds".to_string(),
            ))
        }
    }
}

fn parse_value(value: &str) -> Result<Value, PostgresExecutionError> {
    Ok(serde_json::from_str(value)?)
}

fn parse_time(value: &str) -> Result<OffsetDateTime, PostgresExecutionError> {
    OffsetDateTime::parse(value, &Rfc3339).map_err(|error| {
        PostgresExecutionError::Invalid(format!("invalid timestamp `{value}`: {error}"))
    })
}

fn format_time(value: OffsetDateTime) -> Result<String, PostgresExecutionError> {
    value.format(&Rfc3339).map_err(|error| {
        PostgresExecutionError::Invalid(format!("cannot format timestamp: {error}"))
    })
}

fn execution_from_row(row: PgRow) -> Result<ExecutionRecord, PostgresExecutionError> {
    let state_json: Value = row.try_get("state_json")?;
    let workflow_json: Value = row.try_get("workflow_json")?;
    let output_json: Option<Value> = row.try_get("output_json")?;
    let error_json: Option<Value> = row.try_get("error_json")?;
    let started_at: OffsetDateTime = row.try_get("started_at")?;
    let updated_at: OffsetDateTime = row.try_get("updated_at")?;
    Ok(ExecutionRecord {
        id: row.try_get("id")?,
        workflow_id: row.try_get("workflow_id")?,
        workflow_version: row.try_get("workflow_version")?,
        status: row.try_get("status")?,
        state_json: serde_json::to_string(&state_json)?,
        workflow_json: serde_json::to_string(&workflow_json)?,
        output_json: output_json
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?,
        error_json: error_json.as_ref().map(serde_json::to_string).transpose()?,
        started_at: format_time(started_at)?,
        updated_at: format_time(updated_at)?,
    })
}

fn workflow_from_row(row: PgRow) -> Result<WorkflowRecord, PostgresExecutionError> {
    let ir_json: Value = row.try_get("ir_json")?;
    let created_at: OffsetDateTime = row.try_get("created_at")?;
    let updated_at: OffsetDateTime = row.try_get("updated_at")?;
    Ok(WorkflowRecord {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        version: row.try_get("version")?,
        description: row.try_get("description")?,
        published: if row.try_get::<bool, _>("published")? {
            1
        } else {
            0
        },
        ir_json: serde_json::to_string(&ir_json)?,
        yaml: row.try_get("yaml")?,
        created_at: format_time(created_at)?,
        updated_at: format_time(updated_at)?,
    })
}

fn artifact_from_row(row: PgRow) -> Result<ArtifactRecord, PostgresExecutionError> {
    let content_json: Value = row.try_get("content_json")?;
    let created_at: OffsetDateTime = row.try_get("created_at")?;
    Ok(ArtifactRecord {
        id: row.try_get("id")?,
        execution_id: row.try_get("execution_id")?,
        name: row.try_get("name")?,
        kind: row.try_get("kind")?,
        mime_type: row.try_get("mime_type")?,
        size: row.try_get("size")?,
        content_json: serde_json::to_string(&content_json)?,
        created_at: format_time(created_at)?,
    })
}

fn workflow_version_from_row(row: PgRow) -> Result<WorkflowVersionRecord, PostgresExecutionError> {
    let ir_json: Value = row.try_get("ir_json")?;
    let created_at: OffsetDateTime = row.try_get("created_at")?;
    Ok(WorkflowVersionRecord {
        version: row.try_get("version")?,
        registry_version: row.try_get("registry_version")?,
        ir_json: serde_json::to_string(&ir_json)?,
        created_at: format_time(created_at)?,
    })
}

fn lease_from_row(row: PgRow) -> Result<ExecutionLeaseRecord, PostgresExecutionError> {
    let expires_at: OffsetDateTime = row.try_get("expires_at")?;
    Ok(ExecutionLeaseRecord {
        holder_id: row.try_get("holder_id")?,
        fencing_token: row.try_get("fencing_token")?,
        expires_at: format_time(expires_at)?,
    })
}

fn status_text(status: ExecutionStatus) -> &'static str {
    match status {
        ExecutionStatus::Pending => "pending",
        ExecutionStatus::Running => "running",
        ExecutionStatus::WaitingApproval => "waiting_approval",
        ExecutionStatus::Succeeded => "succeeded",
        ExecutionStatus::Failed => "failed",
        ExecutionStatus::Cancelled => "cancelled",
    }
}
