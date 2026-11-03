use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use hpa_core::compiler::compile_workflow;
use hpa_core::ir::Workflow;
use hpa_core::runtime::{ExecutionStatus, StepStatus, cancel_execution, expire_approval, step};
use hpa_local::postgres_execution::ExecutionLeaseRecord;
use opentelemetry::trace::FutureExt as _;
use serde_json::json;
use time::OffsetDateTime;
use tokio::sync::Mutex;
use tracing::{Instrument, error, info, warn};

use crate::db::{Db, parse_state, status_text};
use crate::metrics::{metrics, now_ms};

#[derive(Clone)]
pub struct RuntimeManager {
    db: Db,
    running: Arc<Mutex<HashSet<String>>>,
    holder_id: String,
    lease_ttl_seconds: i32,
}

impl RuntimeManager {
    pub fn new(db: Db) -> Self {
        let holder_id = std::env::var("HPA_NODE_ID")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| format!("runtime:{}", uuid::Uuid::now_v7()));
        let lease_ttl_seconds = std::env::var("HPA_EXECUTION_LEASE_TTL_SECONDS")
            .ok()
            .and_then(|value| value.parse::<i32>().ok())
            .filter(|value| (1..=3600).contains(value))
            .unwrap_or(30);
        Self {
            db,
            running: Arc::new(Mutex::new(HashSet::new())),
            holder_id,
            lease_ttl_seconds,
        }
    }

    pub fn trigger(&self, execution_id: String) {
        let manager = self.clone();
        let parent = hpa_local::telemetry::current_context();
        let span = tracing::info_span!(
            "workflow.trigger",
            workflow.execution.id = %execution_id
        );
        tokio::spawn(
            async move {
                loop {
                    let acquired = {
                        let mut running = manager.running.lock().await;
                        running.insert(execution_id.clone())
                    };
                    if acquired {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                let lease = match manager
                    .db
                    .acquire_execution_lease(
                        &execution_id,
                        &manager.holder_id,
                        manager.lease_ttl_seconds,
                    )
                    .await
                {
                    Ok(Some(lease)) => lease,
                    Ok(None) => {
                        metrics().inc("hpa_execution_lease_skipped_total", &[]);
                        let mut running = manager.running.lock().await;
                        running.remove(&execution_id);
                        return;
                    }
                    Err(error) => {
                        error!(execution_id, error = %error, "cannot acquire execution lease");
                        let mut running = manager.running.lock().await;
                        running.remove(&execution_id);
                        return;
                    }
                };
                manager.drive(execution_id.clone(), &lease).await;
                if let Err(error) = manager
                    .db
                    .release_execution_lease(&execution_id, &lease.holder_id, lease.fencing_token)
                    .await
                {
                    warn!(execution_id, error = %error, "cannot release execution lease");
                }
            }
            .instrument(span)
            .with_context(parent),
        );
    }

    pub async fn recover(&self) -> Result<usize, crate::db::DbError> {
        let rows = self.db.running_executions().await?;
        let count = rows.len();
        for row in rows {
            self.trigger(row.id);
        }
        if count > 0 {
            info!(count, "resuming persisted executions");
        }
        Ok(count)
    }

    pub fn start_approval_monitor(&self) {
        let manager = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            loop {
                interval.tick().await;
                if let Err(message) = manager.expire_approvals().await {
                    warn!(error = %message, "approval SLA scan failed");
                }
            }
        });
    }

    pub fn start_recovery_monitor(&self) {
        let manager = self.clone();
        tokio::spawn(async move {
            let interval_ms = std::env::var("HPA_EXECUTION_RECOVERY_TICK_MS")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .filter(|value| *value >= 100)
                .unwrap_or(5_000);
            let mut interval = tokio::time::interval(Duration::from_millis(interval_ms));
            loop {
                interval.tick().await;
                match manager.db.running_executions().await {
                    Ok(rows) => {
                        for row in rows {
                            manager.trigger(row.id);
                        }
                    }
                    Err(error) => {
                        warn!(%error, "execution recovery scan failed");
                    }
                }
            }
        });
    }

    async fn expire_approvals(&self) -> Result<(), String> {
        let rows = self
            .db
            .waiting_local_executions()
            .await
            .map_err(|error| error.to_string())?;
        for row in rows {
            let mut state = match parse_state(&row) {
                Ok(state) => state,
                Err(error) => {
                    warn!(execution_id = %row.id, error = %error, "cannot parse waiting execution");
                    continue;
                }
            };
            let Some(request) = state.waiting.clone() else {
                continue;
            };
            if !request.is_overdue(OffsetDateTime::now_utc()) {
                continue;
            }
            let workflow: Workflow = match serde_json::from_str(&row.workflow_json) {
                Ok(workflow) => workflow,
                Err(error) => {
                    warn!(execution_id = %row.id, error = %error, "cannot parse workflow snapshot");
                    continue;
                }
            };
            let compilation = match compile_workflow(&workflow) {
                Ok(compilation) => compilation,
                Err(issues) => {
                    warn!(execution_id = %row.id, issues = ?issues, "cannot compile workflow for approval timeout");
                    continue;
                }
            };
            let Some(lease) = self
                .db
                .acquire_execution_lease(&row.id, &self.holder_id, self.lease_ttl_seconds)
                .await
                .map_err(|error| error.to_string())?
            else {
                continue;
            };
            let events = match expire_approval(
                &mut state,
                &compilation.plan,
                &request.node_id,
                json!({"automatic": true, "reason": "sla"}),
            ) {
                Ok(events) => events,
                Err(error) => {
                    warn!(execution_id = %row.id, error = %error, "cannot expire approval");
                    let _ = self
                        .db
                        .release_execution_lease(&row.id, &lease.holder_id, lease.fencing_token)
                        .await;
                    continue;
                }
            };
            if let Err(error) = self
                .db
                .save_execution_fenced(&state, &events, &lease.holder_id, lease.fencing_token)
                .await
            {
                warn!(execution_id = %row.id, error = %error, "cannot persist approval timeout");
                let _ = self
                    .db
                    .release_execution_lease(&row.id, &lease.holder_id, lease.fencing_token)
                    .await;
                continue;
            }
            let _ = self
                .db
                .release_execution_lease(&row.id, &lease.holder_id, lease.fencing_token)
                .await;
            info!(
                execution_id = %row.id,
                node_id = %request.node_id,
                "approval SLA expired"
            );
            self.trigger(row.id);
        }
        Ok(())
    }

    async fn drive(&self, execution_id: String, lease: &ExecutionLeaseRecord) {
        let span = tracing::info_span!(
            "workflow.execute",
            workflow.execution.id = %execution_id
        );
        let result = self
            .drive_inner(&execution_id, lease)
            .instrument(span)
            .await;
        if let Err(message) = &result {
            error!(execution_id = %execution_id, error = %message, "execution driver failed");
        }
        let mut running = self.running.lock().await;
        running.remove(&execution_id);
    }

    async fn drive_inner(
        &self,
        execution_id: &str,
        lease: &ExecutionLeaseRecord,
    ) -> Result<(), String> {
        let Some(row) = self
            .db
            .get_execution(execution_id)
            .await
            .map_err(|error| error.to_string())?
        else {
            return Ok(());
        };
        let mut state = parse_state(&row).map_err(|error| error.to_string())?;
        if !state.status.is_terminal()
            && self
                .db
                .execution_cancel_requested(execution_id)
                .await
                .map_err(|error| error.to_string())?
        {
            let events =
                cancel_execution(&mut state).map_err(|error| format!("cannot cancel: {error}"))?;
            self.db
                .save_execution_fenced(&state, &events, &lease.holder_id, lease.fencing_token)
                .await
                .map_err(|error| format!("cannot persist cancellation: {error}"))?;
        }
        if state.status.is_terminal() || state.status == ExecutionStatus::WaitingApproval {
            return Ok(());
        }
        let workflow: Workflow = serde_json::from_str(&row.workflow_json)
            .map_err(|error| format!("cannot parse workflow snapshot: {error}"))?;
        let compilation = compile_workflow(&workflow).map_err(|issues| {
            format!(
                "workflow snapshot does not compile: {}",
                serde_json::to_string(&issues).unwrap_or_default()
            )
        })?;
        if !compilation.issues.is_empty() {
            warn!(
                execution_id = %execution_id,
                warnings = ?compilation.issues,
                "workflow has validation warnings"
            );
        }

        let node_types: HashMap<&str, &str> = workflow
            .nodes
            .iter()
            .map(|node| (node.id.as_str(), node.node_type.as_str()))
            .collect();

        loop {
            if state.status.is_terminal() || state.status == ExecutionStatus::WaitingApproval {
                break;
            }
            if self
                .db
                .execution_cancel_requested(execution_id)
                .await
                .map_err(|error| error.to_string())?
            {
                let events = cancel_execution(&mut state)
                    .map_err(|error| format!("cannot cancel execution: {error}"))?;
                self.db
                    .save_execution_fenced(&state, &events, &lease.holder_id, lease.fencing_token)
                    .await
                    .map_err(|error| format!("cannot persist cancellation: {error}"))?;
                break;
            }
            let started = Instant::now();
            let span = tracing::info_span!(
                "workflow.step",
                workflow.execution.id = %execution_id,
                workflow.step.number = state.step_count + 1
            );
            let renewed = self
                .db
                .renew_execution_lease(
                    execution_id,
                    &lease.holder_id,
                    lease.fencing_token,
                    self.lease_ttl_seconds,
                )
                .await
                .map_err(|error| error.to_string())?;
            if !renewed {
                return Err("execution lease was lost or expired".to_string());
            }
            let result = step(&workflow, &compilation.plan, &mut state)
                .instrument(span)
                .await;
            let elapsed = started.elapsed().as_secs_f64();
            record_node_metrics(&result.events, &node_types, elapsed);
            if let Err(error) = self
                .db
                .save_execution_fenced(
                    &state,
                    &result.events,
                    &lease.holder_id,
                    lease.fencing_token,
                )
                .await
            {
                return Err(format!("cannot persist execution state: {error}"));
            }
            match result.status {
                StepStatus::Continue => {}
                StepStatus::Retry { delay_ms } => {
                    tokio::time::sleep(Duration::from_millis(delay_ms.min(60_000))).await;
                }
                StepStatus::WaitingApproval | StepStatus::Succeeded | StepStatus::Failed => break,
            }
        }

        if state.status.is_terminal() {
            let status = status_text(state.status);
            metrics().inc("hpa_workflow_executions_total", &[("status", status)]);
            let duration = ((now_ms() - state.started_ms as i64).max(0) as f64) / 1000.0;
            metrics().observe("hpa_workflow_execution_seconds", &[], duration);
            metrics().observe(
                "hpa_workflow_execution_seconds",
                &[("status", status)],
                duration,
            );
        }

        if state.status.is_terminal()
            && let Err(error) = self.db.save_output_artifact(&state).await
        {
            warn!(execution_id = %execution_id, error = %error, "cannot save output artifact");
        }
        info!(
            execution_id = %execution_id,
            status = status_text(state.status),
            steps = state.step_count,
            "execution finished"
        );
        Ok(())
    }
}

fn record_node_metrics(
    events: &[hpa_core::runtime::ExecutionEvent],
    node_types: &HashMap<&str, &str>,
    elapsed: f64,
) {
    let mut seen: HashSet<&str> = HashSet::new();
    for event in events {
        let Some(node_id) = event.node_id.as_deref() else {
            continue;
        };
        let status = match event.kind.as_str() {
            "node.succeeded" => "succeeded",
            "node.failed" => "failed",
            "node.skipped" => "skipped",
            _ => continue,
        };
        if !seen.insert(node_id) {
            continue;
        }
        let node_type = node_types.get(node_id).copied().unwrap_or("unknown");
        metrics().inc(
            "hpa_node_executions_total",
            &[("type", node_type), ("status", status)],
        );
        metrics().observe("hpa_node_seconds", &[("type", node_type)], elapsed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hpa_core::runtime::{ExecutionEvent, new_state, step};
    use serde_json::json;

    use crate::db::{Db, ExecutionRow};

    #[tokio::test]
    async fn approval_sla_expiry_resumes_execution() {
        let path = std::env::temp_dir().join(format!("hpa-driver-{}.sqlite", uuid::Uuid::now_v7()));
        let db = Db::open(&path).await.expect("open db");
        let workflow: Workflow =
            serde_json::from_str(include_str!("../../../fixtures/08_approval.json"))
                .expect("approval fixture");
        let compilation = compile_workflow(&workflow).expect("compile workflow");
        let mut state = new_state(
            &workflow,
            &compilation.plan,
            "exec_sla_test",
            json!({}),
            Default::default(),
            "local",
        )
        .expect("execution state");
        for _ in 0..20 {
            if state.status == ExecutionStatus::WaitingApproval {
                break;
            }
            let _ = step(&workflow, &compilation.plan, &mut state).await;
        }
        assert_eq!(state.status, ExecutionStatus::WaitingApproval);
        state
            .waiting
            .as_mut()
            .expect("approval request")
            .requested_at = "2026-01-01T00:00:00Z".to_string();

        let start_event = ExecutionEvent {
            id: "evt_start".to_string(),
            execution_id: state.id.clone(),
            sequence: 0,
            kind: "execution.started".to_string(),
            at: state.started_at.clone(),
            node_id: None,
            graph_id: None,
            attempt: None,
            data: json!({}),
            message: None,
        };
        let row = ExecutionRow {
            id: state.id.clone(),
            workflow_id: workflow.id.clone(),
            workflow_version: workflow.workflow_version.clone(),
            status: "waiting_approval".to_string(),
            state_json: serde_json::to_string(&state).expect("serialize state"),
            workflow_json: workflow.to_json_string(false).expect("serialize workflow"),
            output_json: None,
            error_json: None,
            started_at: state.started_at.clone(),
            updated_at: state.updated_at.clone(),
        };
        db.insert_execution(&row, &start_event)
            .await
            .expect("insert execution");

        let manager = RuntimeManager::new(db.clone());
        manager.expire_approvals().await.expect("expire approvals");

        let mut finished = None;
        for _ in 0..100 {
            let row = db
                .get_execution(&state.id)
                .await
                .expect("load execution")
                .expect("execution exists");
            if row.status == "succeeded" || row.status == "failed" {
                finished = Some(row);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let row = finished.expect("execution should finish after SLA expiry");
        assert_eq!(row.status, "succeeded");
        let saved: hpa_core::runtime::ExecutionState =
            serde_json::from_str(&row.state_json).expect("parse final state");
        assert_eq!(
            saved.node_outputs["rejected"],
            json!({"decision": "rejected"})
        );
        assert_eq!(saved.node_outputs["approve"]["timedOut"], json!(true));
        let events = db
            .list_events(&state.id, -1, 100)
            .await
            .expect("load events");
        assert!(
            events.iter().any(|event| event.kind == "approval.timeout"),
            "missing approval.timeout event"
        );
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn remote_executions_are_not_recovered_by_local_runtime() {
        let path = std::env::temp_dir().join(format!("hpa-driver-{}.sqlite", uuid::Uuid::now_v7()));
        let db = Db::open(&path).await.expect("open db");
        let workflow: Workflow =
            serde_json::from_str(include_str!("../../../fixtures/01_sequence.json"))
                .expect("sequence fixture");
        let compilation = compile_workflow(&workflow).expect("compile workflow");
        let state = new_state(
            &workflow,
            &compilation.plan,
            "exec_remote_recover",
            json!({"value": 1}),
            Default::default(),
            "local",
        )
        .expect("execution state");
        let start = ExecutionEvent {
            id: "evt_remote_start".to_string(),
            execution_id: state.id.clone(),
            sequence: 0,
            kind: "execution.started".to_string(),
            at: state.started_at.clone(),
            node_id: None,
            graph_id: None,
            attempt: None,
            data: json!({}),
            message: None,
        };
        let row = ExecutionRow {
            id: state.id.clone(),
            workflow_id: workflow.id.clone(),
            workflow_version: workflow.workflow_version.clone(),
            status: "running".to_string(),
            state_json: serde_json::to_string(&state).expect("serialize state"),
            workflow_json: workflow.to_json_string(false).expect("serialize workflow"),
            output_json: None,
            error_json: None,
            started_at: state.started_at.clone(),
            updated_at: state.updated_at.clone(),
        };
        db.insert_execution(&row, &start)
            .await
            .expect("insert execution");
        db.mark_remote_execution(&state.id, "worker_test")
            .await
            .expect("mark remote");
        let manager = RuntimeManager::new(db.clone());
        assert_eq!(manager.recover().await.expect("recover"), 0);
        tokio::time::sleep(Duration::from_millis(100)).await;
        let saved = db
            .get_execution(&state.id)
            .await
            .expect("load execution")
            .expect("execution exists");
        assert_eq!(saved.status, "running");
        drop(db);
        let _ = std::fs::remove_file(&path);
    }
}
