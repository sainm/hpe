//! Trigger runtime: publishes trigger state, fires schedules and webhooks.
//!
//! Schedules and webhooks only run **published** workflow versions, matching
//! the MCP gateway. Draft edits never execute in the background.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hpa_core::compiler::compile_workflow;
use hpa_core::ir::Workflow;
use hpa_core::runtime::{new_state, start_event};
use hpa_core::trigger::{
    EventSpec, MisfirePolicy, ScheduleSpec, TriggerSpec, next_fire_at, parse_trigger,
};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::db::{Db, DbError, ExecutionRow, TriggerFireRow, TriggerSeed, TriggerStateRow};
use crate::driver::RuntimeManager;
use crate::metrics::metrics;
use hpa_local::distributed_runtime::{SchedulerLease, SchedulerLeaseStore};

/// A tick that runs more than this late treats the slot as missed for `skip`.
const GRACE_MS: i64 = 60_000;
const MAX_DUE_PER_TICK: i64 = 32;
const MAX_ROUNDS: u32 = 50;
const DEFAULT_TICK_MS: u64 = 1_000;
const DEFAULT_LEASE_TTL_SECONDS: i32 = 15;
const DEFAULT_LEASE_KEY: &str = "hpa:scheduler";

#[derive(Clone)]
pub(crate) struct SchedulerCoordination {
    store: SchedulerLeaseStore,
    lease_key: String,
    ttl_seconds: i32,
}

impl SchedulerCoordination {
    pub(crate) fn postgres(pool: sqlx::postgres::PgPool, holder_id: impl Into<String>) -> Self {
        let ttl_seconds = std::env::var("HPA_SCHEDULER_LEASE_TTL_SECONDS")
            .ok()
            .and_then(|value| value.parse::<i32>().ok())
            .filter(|value| (1..=3600).contains(value))
            .unwrap_or(DEFAULT_LEASE_TTL_SECONDS);
        let lease_key = std::env::var("HPA_SCHEDULER_LEASE_KEY")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_LEASE_KEY.to_string());
        Self {
            store: SchedulerLeaseStore::new(pool, holder_id),
            lease_key,
            ttl_seconds,
        }
    }
}

pub(crate) struct SchedulerRuntime {
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl SchedulerRuntime {
    pub(crate) async fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let mut task = self.task;
        if tokio::time::timeout(Duration::from_secs(5), &mut task)
            .await
            .is_err()
        {
            task.abort();
            let _ = task.await;
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TriggerError {
    #[error("trigger `{trigger_id}` of workflow `{workflow_id}` was not found")]
    NotFound {
        workflow_id: String,
        trigger_id: String,
    },
    #[error("trigger `{trigger_id}` is disabled")]
    Disabled { trigger_id: String },
    #[error("trigger `{trigger_id}` of kind `{kind}` cannot be fired")]
    Unsupported { trigger_id: String, kind: String },
    #[error("duplicate event `{event_id}`")]
    Duplicate { event_id: String },
    #[error("no enabled event trigger subscribes to topic `{topic}`")]
    NoSubscribers { topic: String },
    #[error("trigger configuration is invalid: {0}")]
    Invalid(String),
    #[error("workflow version `{version}` of `{workflow_id}` is not published")]
    VersionMissing {
        workflow_id: String,
        version: String,
    },
    #[error("database error: {0}")]
    Db(#[from] DbError),
    #[error("runtime error: {0}")]
    Runtime(String),
}

/// Everything the trigger runtime needs, so tests can build it without the
/// whole API state.
#[derive(Clone)]
pub struct TriggerHost {
    pub db: Db,
    pub runtime: RuntimeManager,
    pub tenant_id: String,
}

impl TriggerHost {
    pub fn new(db: Db, runtime: RuntimeManager, tenant_id: impl Into<String>) -> Self {
        Self {
            db,
            runtime,
            tenant_id: tenant_id.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FireOutcome {
    pub execution_id: String,
    pub workflow_id: String,
    pub workflow_version: String,
    pub trigger_id: String,
    pub kind: String,
}

#[derive(Debug, Clone)]
pub struct FireRequest {
    pub workflow_id: String,
    pub trigger_id: String,
    pub input: Value,
    pub event_id: Option<String>,
    pub scheduled_at: Option<i64>,
    pub origin: String,
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

pub fn rfc3339_from_ms(ms: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(ms.div_euclid(1000))
        .ok()
        .and_then(|value| {
            value
                .format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_default()
}

/// Rebuilds `trigger_state` from a freshly published workflow version.
pub async fn sync_published(
    host: &TriggerHost,
    workflow: &Workflow,
    version: &str,
) -> Result<usize, TriggerError> {
    let now = now_ms();
    let mut seeds = Vec::with_capacity(workflow.triggers.len());
    for trigger in &workflow.triggers {
        let config_json = serde_json::to_string(&trigger.config)
            .map_err(|error| TriggerError::Invalid(error.to_string()))?;
        match parse_trigger(trigger) {
            Ok(TriggerSpec::Schedule(spec)) => seeds.push(TriggerSeed {
                trigger_id: trigger.id.clone(),
                kind: "schedule".to_string(),
                config_json,
                enabled: spec.enabled,
                next_fire_at: next_fire_at(&spec, now.div_euclid(1000)).map(|value| value * 1000),
            }),
            Ok(TriggerSpec::Webhook(spec)) => seeds.push(TriggerSeed {
                trigger_id: trigger.id.clone(),
                kind: "webhook".to_string(),
                config_json,
                enabled: spec.enabled,
                next_fire_at: None,
            }),
            Ok(TriggerSpec::Event(spec)) => seeds.push(TriggerSeed {
                trigger_id: trigger.id.clone(),
                kind: "event".to_string(),
                config_json,
                enabled: spec.enabled,
                next_fire_at: None,
            }),
            Ok(TriggerSpec::Mcp) => seeds.push(TriggerSeed {
                trigger_id: trigger.id.clone(),
                kind: "mcp".to_string(),
                config_json,
                enabled: false,
                next_fire_at: None,
            }),
            Err(message) => {
                return Err(TriggerError::Invalid(format!(
                    "trigger `{}`: {message}",
                    trigger.id
                )));
            }
        }
    }
    host.db
        .sync_triggers(
            &workflow.id,
            Some(version),
            &seeds,
            &crate::mcp::now_rfc3339(),
        )
        .await?;
    Ok(seeds.len())
}

fn schedule_of(row: &TriggerStateRow) -> Result<ScheduleSpec, TriggerError> {
    let config: Value = serde_json::from_str(&row.config_json)
        .map_err(|error| TriggerError::Invalid(error.to_string()))?;
    let trigger = hpa_core::ir::Trigger {
        id: row.trigger_id.clone(),
        trigger_type: hpa_core::ir::TriggerType::Schedule,
        config,
    };
    match parse_trigger(&trigger) {
        Ok(TriggerSpec::Schedule(spec)) => Ok(*spec),
        Ok(_) => Err(TriggerError::Unsupported {
            trigger_id: row.trigger_id.clone(),
            kind: row.kind.clone(),
        }),
        Err(message) => Err(TriggerError::Invalid(message)),
    }
}

fn event_of(row: &TriggerStateRow) -> Result<EventSpec, TriggerError> {
    let config: Value = serde_json::from_str(&row.config_json)
        .map_err(|error| TriggerError::Invalid(error.to_string()))?;
    let trigger = hpa_core::ir::Trigger {
        id: row.trigger_id.clone(),
        trigger_type: hpa_core::ir::TriggerType::Event,
        config,
    };
    match parse_trigger(&trigger) {
        Ok(TriggerSpec::Event(spec)) => Ok(spec),
        Ok(_) => Err(TriggerError::Unsupported {
            trigger_id: row.trigger_id.clone(),
            kind: row.kind.clone(),
        }),
        Err(message) => Err(TriggerError::Invalid(message)),
    }
}

/// One scheduler pass for a single tenant. `now` is injected so tests do not
/// have to sleep.
pub async fn tick(host: &TriggerHost, now: i64) -> Result<Vec<FireOutcome>, TriggerError> {
    let due = host.db.due_triggers(now, MAX_DUE_PER_TICK).await?;
    let mut fired = Vec::new();
    for row in due {
        let spec = match schedule_of(&row) {
            Ok(spec) => spec,
            Err(error) => {
                tracing::warn!(
                    workflow = %row.workflow_id,
                    trigger = %row.trigger_id,
                    error = %error,
                    "skipping malformed schedule trigger"
                );
                continue;
            }
        };
        let mut expected = match row.next_fire_at {
            Some(value) => value,
            None => continue,
        };
        let mut rounds = 0u32;
        loop {
            rounds += 1;
            if rounds > MAX_ROUNDS {
                break;
            }
            let late = now.saturating_sub(expected);
            let should_fire = match spec.misfire {
                MisfirePolicy::Skip => late <= GRACE_MS,
                MisfirePolicy::RunOnce | MisfirePolicy::CatchUp => true,
            };
            let next_slot =
                next_fire_at(&spec, expected.div_euclid(1000)).map(|value| value * 1000);
            let stored_next = match spec.misfire {
                MisfirePolicy::CatchUp => next_slot,
                _ => next_fire_at(&spec, now.div_euclid(1000)).map(|value| value * 1000),
            };
            let claimed = host
                .db
                .claim_schedule_slot(
                    &row.workflow_id,
                    &row.trigger_id,
                    expected,
                    now,
                    stored_next,
                    &crate::mcp::now_rfc3339(),
                )
                .await?;
            if !claimed {
                break;
            }
            if should_fire {
                let outcome = fire_row(
                    host,
                    &row,
                    &FireRequest {
                        workflow_id: row.workflow_id.clone(),
                        trigger_id: row.trigger_id.clone(),
                        input: spec.input.clone(),
                        event_id: None,
                        scheduled_at: Some(expected),
                        origin: "schedule".to_string(),
                    },
                )
                .await?;
                if let Some(outcome) = outcome {
                    fired.push(outcome);
                }
            }
            if spec.misfire != MisfirePolicy::CatchUp {
                break;
            }
            match next_slot {
                Some(next) if next <= now && fires_room(&spec, rounds) => expected = next,
                _ => break,
            }
        }
        // A capped catch-up must not leave stale slots behind, otherwise the
        // cap would be ignored on every following tick.
        if spec.misfire == MisfirePolicy::CatchUp
            && let Some(row) = host
                .db
                .get_trigger(&row.workflow_id, &row.trigger_id)
                .await?
            && let Some(current) = row.next_fire_at
            && current <= now
            && let Some(next) = next_fire_at(&spec, now.div_euclid(1000)).map(|value| value * 1000)
        {
            let _ = host
                .db
                .claim_schedule_slot(
                    &row.workflow_id,
                    &row.trigger_id,
                    current,
                    now,
                    Some(next),
                    &crate::mcp::now_rfc3339(),
                )
                .await?;
        }
    }
    Ok(fired)
}

fn fires_room(spec: &ScheduleSpec, rounds: u32) -> bool {
    rounds < spec.max_catch_up
}

/// Starts an execution from the published version recorded on the trigger row.
async fn fire_row(
    host: &TriggerHost,
    row: &TriggerStateRow,
    request: &FireRequest,
) -> Result<Option<FireOutcome>, TriggerError> {
    if let Some(event_id) = request.event_id.as_deref()
        && host
            .db
            .trigger_event_delivered(&request.trigger_id, event_id)
            .await?
    {
        return Err(TriggerError::Duplicate {
            event_id: event_id.to_string(),
        });
    }
    let Some(version) = row.workflow_version.clone() else {
        return Err(TriggerError::VersionMissing {
            workflow_id: row.workflow_id.clone(),
            version: "<none>".to_string(),
        });
    };
    let (mut outcome, execution_row, start_event) =
        prepare_published(host, &request.workflow_id, &version, request).await?;
    outcome.kind = row.kind.clone();
    let fire = TriggerFireRow {
        id: format!("fire_{}", Uuid::now_v7()),
        workflow_id: request.workflow_id.clone(),
        trigger_id: request.trigger_id.clone(),
        kind: row.kind.clone(),
        fired_at: crate::mcp::now_rfc3339(),
        scheduled_at: request.scheduled_at.map(rfc3339_from_ms),
        execution_id: Some(outcome.execution_id.clone()),
        status: "started".to_string(),
        event_id: request.event_id.clone(),
        detail_json: Some(
            json!({
                "origin": request.origin,
                "tenant": host.tenant_id,
            })
            .to_string(),
        ),
    };
    let recorded = host
        .db
        .insert_execution_with_trigger(&execution_row, &start_event, &fire)
        .await?;
    if !recorded {
        metrics().inc(
            "hpa_trigger_fires_total",
            &[("kind", row.kind.as_str()), ("result", "duplicate")],
        );
        return Err(TriggerError::Duplicate {
            event_id: request.event_id.clone().unwrap_or_default(),
        });
    }
    host.runtime.trigger(outcome.execution_id.clone());
    metrics().inc(
        "hpa_trigger_fires_total",
        &[("kind", row.kind.as_str()), ("result", "started")],
    );
    let _ = host
        .db
        .note_trigger_execution(
            &request.workflow_id,
            &request.trigger_id,
            &outcome.execution_id,
            "running",
        )
        .await;
    Ok(Some(outcome))
}

async fn prepare_published(
    host: &TriggerHost,
    workflow_id: &str,
    version: &str,
    request: &FireRequest,
) -> Result<(FireOutcome, ExecutionRow, hpa_core::runtime::ExecutionEvent), TriggerError> {
    let row = match host.db.get_version(workflow_id, version).await? {
        Some(row) => row,
        None => {
            let available = host
                .db
                .list_versions(workflow_id)
                .await
                .map(|rows| rows.into_iter().map(|row| row.version).collect::<Vec<_>>())
                .unwrap_or_default();
            tracing::error!(
                workflow_id,
                version,
                available = ?available,
                "trigger references a missing published workflow version"
            );
            return Err(TriggerError::VersionMissing {
                workflow_id: workflow_id.to_string(),
                version: version.to_string(),
            });
        }
    };
    let workflow: Workflow = serde_json::from_str(&row.ir_json)
        .map_err(|error| TriggerError::Invalid(format!("workflow is not valid IR: {error}")))?;
    let compilation = compile_workflow(&workflow).map_err(|issues| {
        TriggerError::Runtime(format!("workflow does not compile: {issues:?}"))
    })?;
    let execution_id = format!("exec_{}", Uuid::now_v7());
    let mut execution = new_state(
        &workflow,
        &compilation.plan,
        &execution_id,
        request.input.clone(),
        Default::default(),
        host.tenant_id.clone(),
    )
    .map_err(|error| TriggerError::Runtime(error.to_string()))?;
    if !row.registry_version.is_empty() {
        execution.registry_version = row.registry_version.clone();
    }
    let event = start_event(&mut execution);
    let execution_row = ExecutionRow {
        id: execution.id.clone(),
        workflow_id: execution.workflow_id.clone(),
        workflow_version: execution.workflow_version.clone(),
        status: "running".to_string(),
        state_json: serde_json::to_string(&execution)
            .map_err(|error| TriggerError::Runtime(error.to_string()))?,
        workflow_json: workflow
            .to_json_string(false)
            .map_err(|error| TriggerError::Runtime(error.to_string()))?,
        output_json: None,
        error_json: None,
        started_at: execution.started_at.clone(),
        updated_at: execution.updated_at.clone(),
    };
    Ok((
        FireOutcome {
            execution_id: execution.id.clone(),
            workflow_id: execution.workflow_id.clone(),
            workflow_version: execution.workflow_version.clone(),
            trigger_id: request.trigger_id.clone(),
            kind: String::new(),
        },
        execution_row,
        event,
    ))
}

/// Fire a webhook or manually triggered workflow immediately.
pub async fn fire_now(
    host: &TriggerHost,
    request: FireRequest,
) -> Result<FireOutcome, TriggerError> {
    let row = host
        .db
        .get_trigger(&request.workflow_id, &request.trigger_id)
        .await?
        .ok_or_else(|| TriggerError::NotFound {
            workflow_id: request.workflow_id.clone(),
            trigger_id: request.trigger_id.clone(),
        })?;
    if row.enabled == 0 {
        return Err(TriggerError::Disabled {
            trigger_id: request.trigger_id.clone(),
        });
    }
    if !matches!(row.kind.as_str(), "webhook" | "schedule" | "event") {
        return Err(TriggerError::Unsupported {
            trigger_id: request.trigger_id.clone(),
            kind: row.kind.clone(),
        });
    }
    fire_row(host, &row, &request)
        .await?
        .ok_or_else(|| TriggerError::Runtime("execution was not created".to_string()))
}

pub async fn fire_event(
    host: &TriggerHost,
    topic: &str,
    payload: Value,
    event_id: Option<String>,
) -> Result<(String, Vec<FireOutcome>), TriggerError> {
    hpa_core::trigger::validate_event_topic(topic).map_err(TriggerError::Invalid)?;
    let event_id = event_id.unwrap_or_else(|| format!("evt_{}", Uuid::now_v7()));
    let rows = host.db.list_triggers().await?;
    let mut subscriptions = Vec::new();
    for row in rows {
        if row.kind != "event" || row.enabled == 0 {
            continue;
        }
        let spec = event_of(&row)?;
        if spec.topic == topic {
            subscriptions.push(row);
        }
    }
    if subscriptions.is_empty() {
        return Err(TriggerError::NoSubscribers {
            topic: topic.to_string(),
        });
    }
    let mut pending = Vec::with_capacity(subscriptions.len());
    for row in subscriptions {
        if !host
            .db
            .trigger_event_delivered(&row.trigger_id, &event_id)
            .await?
        {
            pending.push(row);
        }
    }
    if pending.is_empty() {
        return Err(TriggerError::Duplicate {
            event_id: event_id.clone(),
        });
    }
    let mut outcomes = Vec::with_capacity(pending.len());
    for row in pending {
        let outcome = fire_now(
            host,
            FireRequest {
                workflow_id: row.workflow_id.clone(),
                trigger_id: row.trigger_id.clone(),
                input: payload.clone(),
                event_id: Some(event_id.clone()),
                scheduled_at: None,
                origin: format!("event:{topic}"),
            },
        )
        .await?;
        outcomes.push(outcome);
    }
    Ok((event_id, outcomes))
}

pub async fn ensure_event_subscribers(host: &TriggerHost, topic: &str) -> Result<(), TriggerError> {
    hpa_core::trigger::validate_event_topic(topic).map_err(TriggerError::Invalid)?;
    let rows = host.db.list_triggers().await?;
    for row in rows {
        if row.kind != "event" || row.enabled == 0 {
            continue;
        }
        if event_of(&row)?.topic == topic {
            return Ok(());
        }
    }
    Err(TriggerError::NoSubscribers {
        topic: topic.to_string(),
    })
}

/// Enables or disables a trigger. Enabling a schedule recomputes the next slot.
pub async fn set_enabled(
    host: &TriggerHost,
    workflow_id: &str,
    trigger_id: &str,
    enabled: bool,
) -> Result<Option<i64>, TriggerError> {
    let row = host
        .db
        .get_trigger(workflow_id, trigger_id)
        .await?
        .ok_or_else(|| TriggerError::NotFound {
            workflow_id: workflow_id.to_string(),
            trigger_id: trigger_id.to_string(),
        })?;
    let next = if enabled && row.kind == "schedule" {
        let spec = schedule_of(&row)?;
        next_fire_at(&spec, now_ms().div_euclid(1000)).map(|value| value * 1000)
    } else {
        None
    };
    let updated = host
        .db
        .set_trigger_enabled(
            workflow_id,
            trigger_id,
            enabled,
            next,
            &crate::mcp::now_rfc3339(),
        )
        .await?;
    if !updated {
        return Err(TriggerError::NotFound {
            workflow_id: workflow_id.to_string(),
            trigger_id: trigger_id.to_string(),
        });
    }
    Ok(next)
}

/// Background loop. Each pass walks every active tenant's runtime.
pub(crate) fn spawn(
    enterprise: crate::enterprise::EnterpriseState,
    coordination: Option<SchedulerCoordination>,
) -> SchedulerRuntime {
    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let interval = std::env::var("HPA_SCHEDULER_TICK_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value >= 100)
            .unwrap_or(DEFAULT_TICK_MS);
        let mut ticker = tokio::time::interval(Duration::from_millis(interval));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut current_lease: Option<SchedulerLease> = None;
        loop {
            tokio::select! {
                _ = &mut stop_rx => break,
                _ = ticker.tick() => {}
            }
            let now = now_ms();
            if let Some(coordination) = coordination.as_ref() {
                let mut renewed = false;
                if let Some(lease) = current_lease.as_ref() {
                    match coordination
                        .store
                        .renew(
                            &coordination.lease_key,
                            lease.fencing_token,
                            coordination.ttl_seconds,
                        )
                        .await
                    {
                        Ok(value) => renewed = value,
                        Err(error) => {
                            tracing::warn!(%error, "cannot renew scheduler leadership lease");
                        }
                    }
                }
                if !renewed {
                    current_lease = match coordination
                        .store
                        .acquire(&coordination.lease_key, coordination.ttl_seconds)
                        .await
                    {
                        Ok(lease) => lease,
                        Err(error) => {
                            metrics().set("hpa_scheduler_leader", &[], 0.0);
                            tracing::warn!(%error, "cannot acquire scheduler leadership lease");
                            continue;
                        }
                    };
                }
                let leader = current_lease.is_some();
                metrics().set("hpa_scheduler_leader", &[], if leader { 1.0 } else { 0.0 });
                metrics().mark_scheduler_tick(now);
                if !leader {
                    continue;
                }
            }
            let tenants = match enterprise.active_tenants().await {
                Ok(tenants) => tenants,
                Err(error) => {
                    tracing::warn!(error = %error.message, "cannot list tenants for scheduler");
                    continue;
                }
            };
            for tenant in tenants {
                if let (Some(coordination), Some(lease)) =
                    (coordination.as_ref(), current_lease.as_ref())
                {
                    match coordination
                        .store
                        .renew(
                            &coordination.lease_key,
                            lease.fencing_token,
                            coordination.ttl_seconds,
                        )
                        .await
                    {
                        Ok(true) => {}
                        Ok(false) => {
                            current_lease = None;
                            metrics().set("hpa_scheduler_leader", &[], 0.0);
                            tracing::warn!(
                                "scheduler leadership lease was lost before tenant tick"
                            );
                            break;
                        }
                        Err(error) => {
                            current_lease = None;
                            metrics().set("hpa_scheduler_leader", &[], 0.0);
                            tracing::warn!(%error, "scheduler leadership renewal failed before tenant tick");
                            break;
                        }
                    }
                }
                let state = match enterprise.tenant_api_state(&tenant).await {
                    Ok(state) => state,
                    Err(error) => {
                        tracing::warn!(tenant = %tenant, error = %error.message, "cannot open tenant for scheduler");
                        continue;
                    }
                };
                let host = state.trigger_host();
                let started = std::time::Instant::now();
                let outcome = tick(&host, now).await;
                metrics().observe(
                    "hpa_scheduler_tick_seconds",
                    &[],
                    started.elapsed().as_secs_f64(),
                );
                metrics().inc("hpa_scheduler_ticks_total", &[]);
                metrics().mark_scheduler_tick(now);
                match outcome {
                    Ok(fired) if !fired.is_empty() => {
                        metrics().inc_by("hpa_scheduler_fires_total", &[], fired.len() as f64);
                        tracing::info!(tenant = %tenant, count = fired.len(), "scheduler fired triggers");
                    }
                    Ok(_) => {}
                    Err(error) => {
                        tracing::warn!(tenant = %tenant, error = %error, "scheduler tick failed");
                    }
                }
            }
        }
        if let (Some(coordination), Some(lease)) = (coordination.as_ref(), current_lease.as_ref()) {
            let _ = tokio::time::timeout(
                Duration::from_secs(2),
                coordination
                    .store
                    .release(&coordination.lease_key, lease.fencing_token),
            )
            .await;
        }
        metrics().set("hpa_scheduler_leader", &[], 0.0);
    });
    SchedulerRuntime {
        stop: Some(stop_tx),
        task,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{TriggerSeed, WorkflowRow};
    use serde_json::json;

    fn workflow_json(triggers: Value) -> Value {
        json!({
            "$schema": "https://hpa.dev/schema/ir/v0.1/workflow.schema.json",
            "schemaVersion": "hpa.dev/v1",
            "id": "wf_sched",
            "name": "Scheduled",
            "workflowVersion": "1.0.0",
            "nodes": [
                {"id": "start", "type": "start", "typeVersion": "1.0.0", "name": "Start", "config": {}},
                {"id": "end", "type": "end", "typeVersion": "1.0.0", "name": "End", "config": {}}
            ],
            "edges": [
                {"id": "e1", "source": "start", "sourcePort": "out", "target": "end", "targetPort": "in"}
            ],
            "triggers": triggers
        })
    }

    async fn host(label: &str) -> (TriggerHost, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "hpa-scheduler-{label}-{}.sqlite",
            uuid::Uuid::now_v7()
        ));
        let db = Db::open(&path).await.expect("open db");
        let runtime = RuntimeManager::new(db.clone());
        (TriggerHost::new(db, runtime, "local"), path)
    }

    async fn publish(host: &TriggerHost, triggers: Value) -> Workflow {
        let value = workflow_json(triggers);
        let workflow: Workflow = serde_json::from_value(value.clone()).expect("workflow ir");
        host.db
            .publish_workflow(
                &WorkflowRow {
                    id: workflow.id.clone(),
                    name: workflow.name.clone(),
                    version: workflow.workflow_version.clone(),
                    description: None,
                    published: 1,
                    ir_json: serde_json::to_string(&value).unwrap(),
                    yaml: String::new(),
                    created_at: "2026-11-01T00:00:00Z".to_string(),
                    updated_at: "2026-11-01T00:00:00Z".to_string(),
                },
                "2026.11.1",
            )
            .await
            .expect("publish");
        workflow
    }

    fn schedule_trigger(cron: &str, extra: Value) -> Value {
        let mut config = json!({"cron": cron, "timezone": "+08:00"});
        if let (Some(target), Some(source)) = (config.as_object_mut(), extra.as_object()) {
            for (key, value) in source {
                target.insert(key.clone(), value.clone());
            }
        }
        json!([{"id": "nightly", "type": "schedule", "config": config}])
    }

    async fn force_due(host: &TriggerHost, config: Value, next_fire_at: i64) {
        host.db
            .sync_triggers(
                "wf_sched",
                Some("1.0.0"),
                &[TriggerSeed {
                    trigger_id: "nightly".to_string(),
                    kind: "schedule".to_string(),
                    config_json: serde_json::to_string(&config).unwrap(),
                    enabled: true,
                    next_fire_at: Some(next_fire_at),
                }],
                "2026-11-01T00:00:00Z",
            )
            .await
            .expect("seed trigger");
    }

    #[tokio::test]
    async fn publishing_materialises_schedules_and_webhooks() {
        let (host, path) = host("sync").await;
        let workflow = publish(
            &host,
            json!([
                {"id": "nightly", "type": "schedule", "config": {"cron": "0 9 * * *", "timezone": "+08:00"}},
                {"id": "hook", "type": "webhook", "config": {"secret": "0123456789abcdef"}},
                {"id": "bus", "type": "event", "config": {}}
            ]),
        )
        .await;
        let count = sync_published(&host, &workflow, "1.0.0")
            .await
            .expect("sync");
        assert_eq!(count, 3);
        let rows = host.db.list_triggers().await.expect("list");
        assert_eq!(rows.len(), 3);
        let schedule = rows.iter().find(|row| row.trigger_id == "nightly").unwrap();
        assert_eq!(schedule.kind, "schedule");
        assert_eq!(schedule.enabled, 1);
        // 09:00 +08:00 is 01:00 UTC: the stored slot must be on that boundary.
        let next = schedule.next_fire_at.expect("next fire");
        assert_eq!(next % 60_000, 0);
        assert!(
            rfc3339_from_ms(next).ends_with("T01:00:00Z"),
            "unexpected slot {}",
            rfc3339_from_ms(next)
        );
        let event = rows.iter().find(|row| row.trigger_id == "bus").unwrap();
        assert_eq!(event.enabled, 1, "event triggers subscribe by default");
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn due_schedule_fires_once_and_advances() {
        let (host, path) = host("due").await;
        publish(&host, schedule_trigger("* * * * *", json!({}))).await;
        let now = 1_793_000_000_000i64;
        force_due(
            &host,
            json!({"cron": "* * * * *", "timezone": "UTC"}),
            now - 5_000,
        )
        .await;

        let fired = tick(&host, now).await.expect("tick");
        assert_eq!(fired.len(), 1, "one slot fires");
        let fires = host.db.list_trigger_fires(10).await.expect("fires");
        assert_eq!(fires.len(), 1);
        assert!(fires[0].execution_id.is_some());
        let execution = host
            .db
            .get_execution(fires[0].execution_id.as_deref().unwrap())
            .await
            .expect("get execution")
            .expect("execution exists");
        assert_eq!(execution.workflow_version, "1.0.0");

        let row = host
            .db
            .get_trigger("wf_sched", "nightly")
            .await
            .unwrap()
            .unwrap();
        assert!(
            row.next_fire_at.unwrap() > now,
            "next slot must move forward"
        );
        assert!(row.last_execution_id.is_some());

        // A second tick on the same timestamp must not fire again.
        let fired = tick(&host, now).await.expect("tick again");
        assert!(fired.is_empty());
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn skip_policy_drops_very_late_slots() {
        let (host, path) = host("skip").await;
        publish(&host, schedule_trigger("* * * * *", json!({}))).await;
        let now = 1_793_000_000_000i64;
        force_due(
            &host,
            json!({"cron": "* * * * *", "timezone": "UTC"}),
            now - 600_000,
        )
        .await;

        let fired = tick(&host, now).await.expect("tick");
        assert!(fired.is_empty(), "skip drops the missed slot");
        let fires = host.db.list_trigger_fires(10).await.expect("fires");
        assert!(fires.is_empty());
        let row = host
            .db
            .get_trigger("wf_sched", "nightly")
            .await
            .unwrap()
            .unwrap();
        assert!(row.next_fire_at.unwrap() > now);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn catch_up_policy_fires_each_missed_slot_up_to_the_cap() {
        let (host, path) = host("catchup").await;
        publish(
            &host,
            schedule_trigger(
                "*/5 * * * *",
                json!({"misfire": "catch_up", "maxCatchUp": 3}),
            ),
        )
        .await;
        let now = 1_793_000_000_000i64;
        let first_slot = (now / 300_000) * 300_000 - 900_000;
        force_due(
            &host,
            json!({"cron": "*/5 * * * *", "timezone": "UTC", "misfire": "catch_up", "maxCatchUp": 3}),
            first_slot,
        )
        .await;

        let fired = tick(&host, now).await.expect("tick");
        assert_eq!(fired.len(), 3, "capped at maxCatchUp");
        let fires = host.db.list_trigger_fires(10).await.expect("fires");
        assert_eq!(fires.len(), 3);
        let row = host
            .db
            .get_trigger("wf_sched", "nightly")
            .await
            .unwrap()
            .unwrap();
        assert!(row.next_fire_at.unwrap() > now);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn disabled_triggers_are_skipped_and_webhooks_are_idempotent() {
        let (host, path) = host("hook").await;
        let workflow = publish(
            &host,
            json!([{"id": "hook", "type": "webhook", "config": {"secret": "0123456789abcdef"}}]),
        )
        .await;
        sync_published(&host, &workflow, "1.0.0")
            .await
            .expect("sync");

        let request = FireRequest {
            workflow_id: "wf_sched".to_string(),
            trigger_id: "hook".to_string(),
            input: json!({"ping": 1}),
            event_id: Some("evt-1".to_string()),
            scheduled_at: None,
            origin: "webhook".to_string(),
        };
        let outcome = fire_now(&host, request.clone()).await.expect("fire");
        assert_eq!(outcome.kind, "webhook");
        let duplicate = fire_now(&host, request).await;
        assert!(
            matches!(duplicate, Err(TriggerError::Duplicate { .. })),
            "same event id must be rejected"
        );

        set_enabled(&host, "wf_sched", "hook", false)
            .await
            .expect("disable");
        let disabled = fire_now(
            &host,
            FireRequest {
                workflow_id: "wf_sched".to_string(),
                trigger_id: "hook".to_string(),
                input: json!({}),
                event_id: None,
                scheduled_at: None,
                origin: "webhook".to_string(),
            },
        )
        .await;
        assert!(matches!(disabled, Err(TriggerError::Disabled { .. })));
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn partial_event_fanout_retries_only_missing_subscribers() {
        let (host, path) = host("partial-event").await;
        let workflow = publish(
            &host,
            json!([
                {"id": "bus-a", "type": "event", "config": {"topic": "orders.created"}},
                {"id": "bus-b", "type": "event", "config": {"topic": "orders.created"}}
            ]),
        )
        .await;
        sync_published(&host, &workflow, "1.0.0")
            .await
            .expect("sync");
        let payload = json!({"orderId": "order-1"});
        fire_now(
            &host,
            FireRequest {
                workflow_id: "wf_sched".to_string(),
                trigger_id: "bus-a".to_string(),
                input: payload.clone(),
                event_id: Some("evt-partial".to_string()),
                scheduled_at: None,
                origin: "event:orders.created".to_string(),
            },
        )
        .await
        .expect("pre-deliver first subscriber");
        assert!(
            host.db
                .trigger_event_delivered("bus-a", "evt-partial")
                .await
                .expect("first delivery")
        );
        assert!(
            !host
                .db
                .trigger_event_delivered("bus-b", "evt-partial")
                .await
                .expect("second delivery")
        );

        let (_, outcomes) = fire_event(
            &host,
            "orders.created",
            payload.clone(),
            Some("evt-partial".to_string()),
        )
        .await
        .expect("retry partial fanout");
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].trigger_id, "bus-b");
        assert!(
            host.db
                .trigger_event_delivered("bus-a", "evt-partial")
                .await
                .expect("first delivery remains")
        );
        assert!(
            host.db
                .trigger_event_delivered("bus-b", "evt-partial")
                .await
                .expect("second delivery completed")
        );
        assert!(matches!(
            fire_event(
                &host,
                "orders.created",
                payload,
                Some("evt-partial".to_string())
            )
            .await,
            Err(TriggerError::Duplicate { .. })
        ));
        let _ = std::fs::remove_file(path);
    }
}
