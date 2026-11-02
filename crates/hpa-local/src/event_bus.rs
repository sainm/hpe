use std::time::Duration;

use hpa_local::distributed_runtime::{OutboxMessageInput, RuntimeError, TenantRuntime};
use serde_json::{Value, json};
use sqlx::postgres::PgPool;
use tracing::Instrument as _;
use uuid::Uuid;

use crate::enterprise::EnterpriseState;
use crate::metrics::metrics;

const EVENT_TOPIC: &str = "hpa.event.topic";
const DEFAULT_TICK_MS: u64 = 200;

#[derive(Debug, thiserror::Error)]
pub enum EventBusError {
    #[error("event bus runtime error: {0}")]
    Runtime(#[from] RuntimeError),
    #[error("event `{event_id}` was already queued with different content")]
    IdempotencyConflict { event_id: String },
}

#[derive(Debug, Clone)]
pub struct PublishOutcome {
    pub queued: bool,
    pub message_id: String,
}

#[derive(Clone)]
pub struct EventBusPublisher {
    runtime: TenantRuntime,
}

impl EventBusPublisher {
    pub fn new(pool: PgPool, tenant_id: impl Into<String>) -> Self {
        let tenant_id = tenant_id.into();
        Self {
            runtime: TenantRuntime::new(
                pool,
                tenant_id,
                format!("event-publisher:{}", Uuid::now_v7()),
            ),
        }
    }

    pub async fn publish(
        &self,
        topic: &str,
        event_id: &str,
        input: &Value,
    ) -> Result<PublishOutcome, EventBusError> {
        hpa_core::trigger::validate_event_topic(topic)
            .map_err(|error| RuntimeError::Invalid(error.to_string()))?;
        if event_id.is_empty() || event_id.len() > 256 || event_id.chars().any(char::is_control) {
            return Err(RuntimeError::Invalid(
                "event id must be 1-256 bytes without control characters".to_string(),
            )
            .into());
        }
        let message_id = format!("eventmsg_{}", Uuid::now_v7());
        let idempotency_key = format!("topic-event:{event_id}");
        let payload = json!({"topic": topic, "input": input});
        let inserted = self
            .runtime
            .enqueue(&OutboxMessageInput {
                id: message_id.clone(),
                topic: EVENT_TOPIC.to_string(),
                event_type: "topic.published".to_string(),
                aggregate_type: "event".to_string(),
                aggregate_id: event_id.to_string(),
                payload: payload.clone(),
                headers: json!({"source": "http"}),
                idempotency_key: idempotency_key.clone(),
                max_attempts: 10,
                trace_id: hpa_local::telemetry::current_trace_id(),
            })
            .await?;
        if inserted {
            metrics().inc("hpa_event_bus_enqueued_total", &[]);
            return Ok(PublishOutcome {
                queued: true,
                message_id,
            });
        }
        let existing = self
            .runtime
            .by_idempotency_key(&idempotency_key)
            .await?
            .ok_or_else(|| RuntimeError::Invalid("queued event disappeared".to_string()))?;
        if existing.topic != EVENT_TOPIC || existing.payload_json != payload {
            return Err(EventBusError::IdempotencyConflict {
                event_id: event_id.to_string(),
            });
        }
        Ok(PublishOutcome {
            queued: false,
            message_id: existing.id,
        })
    }
}

pub struct EventBusRuntime {
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl EventBusRuntime {
    pub async fn shutdown(mut self) {
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

pub fn spawn(
    enterprise: EnterpriseState,
    pool: PgPool,
    node_id: impl Into<String>,
) -> EventBusRuntime {
    let node_id = node_id.into();
    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let tick = std::env::var("HPA_EVENT_BUS_TICK_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| (100..=60_000).contains(value))
            .unwrap_or(DEFAULT_TICK_MS);
        let mut interval = tokio::time::interval(Duration::from_millis(tick));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = &mut stop_rx => break,
                _ = interval.tick() => {}
            }
            let tenants = match enterprise.active_tenants().await {
                Ok(tenants) => tenants,
                Err(error) => {
                    tracing::warn!(error = %error.message, "event bus tenant scan failed");
                    continue;
                }
            };
            for tenant_id in tenants {
                let runtime = TenantRuntime::new(
                    pool.clone(),
                    &tenant_id,
                    format!("event-consumer:{}:{node_id}", Uuid::now_v7()),
                );
                let messages = match runtime.claim_topic(EVENT_TOPIC, 20, 30).await {
                    Ok(messages) => messages,
                    Err(error) => {
                        tracing::error!(tenant_id, %error, "event bus claim failed");
                        continue;
                    }
                };
                for message in messages {
                    let result = process_message(&enterprise, &tenant_id, &message).await;
                    match result {
                        Ok(()) => {
                            if let Err(error) =
                                runtime.complete(&message.id, message.fencing_token).await
                            {
                                tracing::warn!(
                                    tenant_id,
                                    message_id = message.id,
                                    %error,
                                    "event bus completion failed"
                                );
                            } else {
                                metrics().inc("hpa_event_bus_processed_total", &[]);
                            }
                        }
                        Err(error) => {
                            metrics().inc("hpa_event_bus_failures_total", &[]);
                            tracing::warn!(
                                tenant_id,
                                message_id = message.id,
                                %error,
                                "event bus delivery failed"
                            );
                            let _ = runtime
                                .fail(&message.id, message.fencing_token, &error.to_string(), 2)
                                .await;
                        }
                    }
                }
            }
        }
    });
    EventBusRuntime {
        stop: Some(stop_tx),
        task,
    }
}

async fn process_message(
    enterprise: &EnterpriseState,
    tenant_id: &str,
    message: &hpa_local::distributed_runtime::OutboxMessage,
) -> Result<(), String> {
    if message.event_type != "topic.published" || message.aggregate_type != "event" {
        return Err(format!(
            "unexpected event bus message {} / {}",
            message.event_type, message.aggregate_type
        ));
    }
    let topic = message
        .payload_json
        .get("topic")
        .and_then(Value::as_str)
        .ok_or_else(|| "event bus payload has no topic".to_string())?;
    let input = message
        .payload_json
        .get("input")
        .cloned()
        .unwrap_or(Value::Null);
    let event_id = message.aggregate_id.clone();
    let state = enterprise
        .tenant_api_state(tenant_id)
        .await
        .map_err(|error| error.message)?;
    let span = tracing::info_span!(
        "event.bus.deliver",
        event.id = %event_id,
        event.topic = %topic,
        event.outbox.id = %message.id,
        event.trace.id = message.trace_id.as_deref().unwrap_or("")
    );
    let result = crate::scheduler::fire_event(&state.trigger_host(), topic, input, Some(event_id))
        .instrument(span)
        .await;
    match result {
        Ok(_) => Ok(()),
        Err(crate::scheduler::TriggerError::Duplicate { .. })
        | Err(crate::scheduler::TriggerError::NoSubscribers { .. }) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}
