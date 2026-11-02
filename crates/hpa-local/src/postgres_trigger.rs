use serde_json::Value;
use sqlx::Row;
use sqlx::postgres::{PgPool, PgRow};
use sqlx::types::Json;
use thiserror::Error;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

#[derive(Debug, Error)]
pub enum PostgresTriggerError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid trigger data: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone)]
pub struct TriggerSeedRecord {
    pub trigger_id: String,
    pub kind: String,
    pub config_json: String,
    pub enabled: bool,
    pub next_fire_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct TriggerRecord {
    pub workflow_id: String,
    pub trigger_id: String,
    pub kind: String,
    pub config_json: String,
    pub workflow_version: Option<String>,
    pub enabled: i64,
    pub next_fire_at: Option<i64>,
    pub last_fire_at: Option<i64>,
    pub last_execution_id: Option<String>,
    pub last_status: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct TriggerFireRecord {
    pub id: String,
    pub workflow_id: String,
    pub trigger_id: String,
    pub kind: String,
    pub fired_at: String,
    pub scheduled_at: Option<String>,
    pub execution_id: Option<String>,
    pub status: String,
    pub event_id: Option<String>,
    pub detail_json: Option<String>,
}

#[derive(Clone)]
pub struct PostgresTriggerStore {
    pool: PgPool,
    tenant_id: String,
}

impl PostgresTriggerStore {
    pub fn new(pool: PgPool, tenant_id: impl Into<String>) -> Self {
        Self {
            pool,
            tenant_id: tenant_id.into(),
        }
    }

    pub async fn sync_triggers(
        &self,
        workflow_id: &str,
        version: Option<&str>,
        seeds: &[TriggerSeedRecord],
        now: &str,
    ) -> Result<(), PostgresTriggerError> {
        let now = parse_time(now)?;
        let trigger_ids = seeds
            .iter()
            .map(|seed| seed.trigger_id.clone())
            .collect::<Vec<_>>();
        let mut transaction = self.begin().await?;
        sqlx::query(
            "DELETE FROM hpa_tenant.trigger_state
             WHERE tenant_id = $1
               AND workflow_id = $2
               AND NOT (trigger_id = ANY($3))",
        )
        .bind(&self.tenant_id)
        .bind(workflow_id)
        .bind(&trigger_ids)
        .execute(&mut *transaction)
        .await?;
        for seed in seeds {
            let config: Value = serde_json::from_str(&seed.config_json)?;
            sqlx::query(
                "INSERT INTO hpa_tenant.trigger_state (
                     tenant_id, workflow_id, trigger_id, kind, config_json,
                     workflow_version, enabled, next_fire_at, updated_at
                 )
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                 ON CONFLICT (tenant_id, workflow_id, trigger_id) DO UPDATE
                 SET kind = EXCLUDED.kind,
                     config_json = EXCLUDED.config_json,
                     workflow_version = EXCLUDED.workflow_version,
                     enabled = EXCLUDED.enabled,
                     next_fire_at = EXCLUDED.next_fire_at,
                     updated_at = EXCLUDED.updated_at",
            )
            .bind(&self.tenant_id)
            .bind(workflow_id)
            .bind(&seed.trigger_id)
            .bind(&seed.kind)
            .bind(Json(&config))
            .bind(version)
            .bind(seed.enabled)
            .bind(seed.next_fire_at)
            .bind(now)
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn list_triggers(&self) -> Result<Vec<TriggerRecord>, PostgresTriggerError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT workflow_id, trigger_id, kind, config_json, workflow_version,
                    enabled, next_fire_at, last_fire_at, last_execution_id,
                    last_status, updated_at
             FROM hpa_tenant.trigger_state
             WHERE tenant_id = $1
             ORDER BY workflow_id, trigger_id",
        )
        .bind(&self.tenant_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.into_iter().map(trigger_from_row).collect()
    }

    pub async fn get_trigger(
        &self,
        workflow_id: &str,
        trigger_id: &str,
    ) -> Result<Option<TriggerRecord>, PostgresTriggerError> {
        let mut transaction = self.begin().await?;
        let row = sqlx::query(
            "SELECT workflow_id, trigger_id, kind, config_json, workflow_version,
                    enabled, next_fire_at, last_fire_at, last_execution_id,
                    last_status, updated_at
             FROM hpa_tenant.trigger_state
             WHERE tenant_id = $1 AND workflow_id = $2 AND trigger_id = $3",
        )
        .bind(&self.tenant_id)
        .bind(workflow_id)
        .bind(trigger_id)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.map(trigger_from_row).transpose()
    }

    pub async fn due_triggers(
        &self,
        now_ms: i64,
        limit: i64,
    ) -> Result<Vec<TriggerRecord>, PostgresTriggerError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT workflow_id, trigger_id, kind, config_json, workflow_version,
                    enabled, next_fire_at, last_fire_at, last_execution_id,
                    last_status, updated_at
             FROM hpa_tenant.trigger_state
             WHERE tenant_id = $1
               AND enabled
               AND kind = 'schedule'
               AND next_fire_at IS NOT NULL
               AND next_fire_at <= $2
             ORDER BY next_fire_at
             LIMIT $3",
        )
        .bind(&self.tenant_id)
        .bind(now_ms)
        .bind(limit)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.into_iter().map(trigger_from_row).collect()
    }

    pub async fn claim_schedule_slot(
        &self,
        workflow_id: &str,
        trigger_id: &str,
        expected_next: i64,
        now_ms: i64,
        next_fire_at: Option<i64>,
        updated_at: &str,
    ) -> Result<bool, PostgresTriggerError> {
        let updated_at = parse_time(updated_at)?;
        let mut transaction = self.begin().await?;
        let updated = sqlx::query(
            "UPDATE hpa_tenant.trigger_state
             SET next_fire_at = $5, last_fire_at = $6, updated_at = $7
             WHERE tenant_id = $1
               AND workflow_id = $2
               AND trigger_id = $3
               AND enabled
               AND next_fire_at = $4",
        )
        .bind(&self.tenant_id)
        .bind(workflow_id)
        .bind(trigger_id)
        .bind(expected_next)
        .bind(next_fire_at)
        .bind(now_ms)
        .bind(updated_at)
        .execute(&mut *transaction)
        .await?
        .rows_affected()
            == 1;
        transaction.commit().await?;
        Ok(updated)
    }

    pub async fn set_trigger_enabled(
        &self,
        workflow_id: &str,
        trigger_id: &str,
        enabled: bool,
        next_fire_at: Option<i64>,
        updated_at: &str,
    ) -> Result<bool, PostgresTriggerError> {
        let updated_at = parse_time(updated_at)?;
        let mut transaction = self.begin().await?;
        let updated = sqlx::query(
            "UPDATE hpa_tenant.trigger_state
             SET enabled = $4, next_fire_at = $5, updated_at = $6
             WHERE tenant_id = $1 AND workflow_id = $2 AND trigger_id = $3",
        )
        .bind(&self.tenant_id)
        .bind(workflow_id)
        .bind(trigger_id)
        .bind(enabled)
        .bind(next_fire_at)
        .bind(updated_at)
        .execute(&mut *transaction)
        .await?
        .rows_affected()
            == 1;
        transaction.commit().await?;
        Ok(updated)
    }

    pub async fn note_trigger_execution(
        &self,
        workflow_id: &str,
        trigger_id: &str,
        execution_id: &str,
        status: &str,
    ) -> Result<(), PostgresTriggerError> {
        let mut transaction = self.begin().await?;
        sqlx::query(
            "UPDATE hpa_tenant.trigger_state
             SET last_execution_id = $4, last_status = $5
             WHERE tenant_id = $1 AND workflow_id = $2 AND trigger_id = $3",
        )
        .bind(&self.tenant_id)
        .bind(workflow_id)
        .bind(trigger_id)
        .bind(execution_id)
        .bind(status)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn record_trigger_fire(
        &self,
        row: &TriggerFireRecord,
    ) -> Result<bool, PostgresTriggerError> {
        let fired_at = parse_time(&row.fired_at)?;
        let scheduled_at = row.scheduled_at.as_deref().map(parse_time).transpose()?;
        let detail: Option<Value> = row
            .detail_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?;
        let mut transaction = self.begin().await?;
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
        .bind(&row.id)
        .bind(&row.workflow_id)
        .bind(&row.trigger_id)
        .bind(&row.kind)
        .bind(fired_at)
        .bind(scheduled_at)
        .bind(&row.execution_id)
        .bind(&row.status)
        .bind(&row.event_id)
        .bind(detail.as_ref().map(Json))
        .fetch_optional(&mut *transaction)
        .await?
        .unwrap_or(false);
        transaction.commit().await?;
        Ok(inserted)
    }

    pub async fn trigger_event_delivered(
        &self,
        trigger_id: &str,
        event_id: &str,
    ) -> Result<bool, PostgresTriggerError> {
        let mut transaction = self.begin().await?;
        let delivered: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                 SELECT 1 FROM hpa_tenant.trigger_fires
                 WHERE tenant_id = $1
                   AND trigger_id = $2
                   AND event_id = $3
             )",
        )
        .bind(&self.tenant_id)
        .bind(trigger_id)
        .bind(event_id)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(delivered)
    }

    pub async fn list_trigger_fires(
        &self,
        limit: i64,
    ) -> Result<Vec<TriggerFireRecord>, PostgresTriggerError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT id, workflow_id, trigger_id, kind, fired_at,
                    scheduled_at, execution_id, status, event_id, detail_json
             FROM hpa_tenant.trigger_fires
             WHERE tenant_id = $1
             ORDER BY fired_at DESC
             LIMIT $2",
        )
        .bind(&self.tenant_id)
        .bind(limit)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.into_iter().map(fire_from_row).collect()
    }

    async fn begin(&self) -> Result<sqlx::Transaction<'_, sqlx::Postgres>, PostgresTriggerError> {
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

fn trigger_from_row(row: PgRow) -> Result<TriggerRecord, PostgresTriggerError> {
    let config: Value = row.try_get("config_json")?;
    let updated_at: OffsetDateTime = row.try_get("updated_at")?;
    Ok(TriggerRecord {
        workflow_id: row.try_get("workflow_id")?,
        trigger_id: row.try_get("trigger_id")?,
        kind: row.try_get("kind")?,
        config_json: serde_json::to_string(&config)?,
        workflow_version: row.try_get("workflow_version")?,
        enabled: if row.try_get::<bool, _>("enabled")? {
            1
        } else {
            0
        },
        next_fire_at: row.try_get("next_fire_at")?,
        last_fire_at: row.try_get("last_fire_at")?,
        last_execution_id: row.try_get("last_execution_id")?,
        last_status: row.try_get("last_status")?,
        updated_at: format_time(updated_at)?,
    })
}

fn fire_from_row(row: PgRow) -> Result<TriggerFireRecord, PostgresTriggerError> {
    let fired_at: OffsetDateTime = row.try_get("fired_at")?;
    let scheduled_at: Option<OffsetDateTime> = row.try_get("scheduled_at")?;
    let detail: Option<Value> = row.try_get("detail_json")?;
    Ok(TriggerFireRecord {
        id: row.try_get("id")?,
        workflow_id: row.try_get("workflow_id")?,
        trigger_id: row.try_get("trigger_id")?,
        kind: row.try_get("kind")?,
        fired_at: format_time(fired_at)?,
        scheduled_at: scheduled_at.map(format_time).transpose()?,
        execution_id: row.try_get("execution_id")?,
        status: row.try_get("status")?,
        event_id: row.try_get("event_id")?,
        detail_json: detail.as_ref().map(serde_json::to_string).transpose()?,
    })
}

fn parse_time(value: &str) -> Result<OffsetDateTime, PostgresTriggerError> {
    OffsetDateTime::parse(value, &Rfc3339).map_err(|error| {
        PostgresTriggerError::Invalid(format!("invalid timestamp `{value}`: {error}"))
    })
}

fn format_time(value: OffsetDateTime) -> Result<String, PostgresTriggerError> {
    value
        .format(&Rfc3339)
        .map_err(|error| PostgresTriggerError::Invalid(format!("cannot format timestamp: {error}")))
}
