use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::routing::post;
use hpa_local::ai_worker_client::{AiWorkerConfig, AiWorkerProcess, AiWorkerStart};
use opentelemetry::trace::FutureExt as _;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use prost::Message;
use serde_json::json;
use tokio::sync::mpsc;

async fn collect_trace(
    State(sender): State<mpsc::UnboundedSender<ExportTraceServiceRequest>>,
    body: Bytes,
) -> (
    StatusCode,
    [(axum::http::HeaderName, &'static str); 1],
    Bytes,
) {
    if let Ok(request) = ExportTraceServiceRequest::decode(body.as_ref()) {
        let _ = sender.send(request);
    }
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/x-protobuf")],
        Bytes::new(),
    )
}

async fn accept_other_otlp_signal() -> (
    StatusCode,
    [(axum::http::HeaderName, &'static str); 1],
    Bytes,
) {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/x-protobuf")],
        Bytes::new(),
    )
}

#[tokio::test]
async fn rust_parent_context_appears_in_python_spans() {
    if std::env::var_os("HPA_AI_WORKER_BIN").is_none() {
        eprintln!("skipped: HPA_AI_WORKER_BIN is not set");
        return;
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("collector listener");
    let endpoint = format!(
        "http://127.0.0.1:{}/v1/traces",
        listener.local_addr().expect("collector address").port()
    );
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let collector = Router::new()
        .route("/v1/traces", post(collect_trace))
        .route("/v1/metrics", post(accept_other_otlp_signal))
        .route("/v1/logs", post(accept_other_otlp_signal))
        .with_state(sender);
    let collector_task = tokio::spawn(async move {
        axum::serve(listener, collector)
            .await
            .expect("serve collector");
    });
    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
    let trace_id = "4bf92f3577b34da6a3ce929d0e0e4736";
    let parent = hpa_local::telemetry::trace_context(&format!("00-{trace_id}-00f067aa0ba902b7-01"));
    let environment = BTreeMap::from([
        ("HPA_AI_PLANNER".to_string(), "deterministic".to_string()),
        (
            "HPA_DETERMINISTIC_TOOL".to_string(),
            "workflow.execute".to_string(),
        ),
        (
            "HPA_DETERMINISTIC_ARGUMENTS".to_string(),
            r#"{"workflowId":"wf_1"}"#.to_string(),
        ),
        (
            "HPA_DETERMINISTIC_OUTPUT".to_string(),
            r#"{"ok":true}"#.to_string(),
        ),
        (
            "HPA_DETERMINISTIC_TOOL_RESULTS".to_string(),
            json!({
                "workflow.execute": {
                    "callId": "tc_1",
                    "status": "succeeded",
                    "result": {"executionId": "exec_1"}
                }
            })
            .to_string(),
        ),
        ("HPA_OTEL_ENABLED".to_string(), "1".to_string()),
        ("HPA_OTEL_ENDPOINT".to_string(), endpoint.clone()),
        ("HPA_OTEL_QUEUE_SIZE".to_string(), "64".to_string()),
    ]);
    let config = AiWorkerConfig {
        executable: PathBuf::from(std::env::var("HPA_AI_WORKER_BIN").expect("HPA_AI_WORKER_BIN")),
        args: Vec::new(),
        request_token: "otel-worker-token".to_string(),
        environment,
    };
    let mut worker = AiWorkerProcess::spawn(config)
        .await
        .expect("spawn Python worker");
    let tools = vec![json!({
        "type": "function",
        "function": {
            "name": "workflow.execute",
            "description": "Agent tool workflow.execute",
            "parameters": {
                "type": "object",
                "properties": {"workflowId": {"type": "string"}},
                "required": ["workflowId"]
            },
            "x-hpa": {
                "risk": "read",
                "approvalRequired": false,
                "idempotencyKeyTemplate": "{sessionId}/{step}/{callId}"
            }
        }
    })];
    async {
        worker
            .start(AiWorkerStart {
                session_id: "session-otel-cross-process",
                tenant_id: "tenant-1",
                agent_version_id: "agent-1",
                input: json!({"message": "run"}),
                instructions: None,
                tools: &tools,
                budget: json!({
                    "maxSteps": 4,
                    "maxToolCalls": 2,
                    "timeoutSeconds": 30,
                    "maxTokens": 1000,
                    "maxCostUsd": 0.1
                }),
                tool_token: None,
                model: None,
            })
            .await
            .expect("start Python worker");
        for _ in 0..20 {
            let event = worker
                .next_event(Duration::from_secs(5))
                .await
                .expect("worker event");
            if event["type"] == "session.result" {
                break;
            }
        }
    }
    .with_context(parent)
    .await;
    worker.shutdown().await.expect("shutdown Python worker");

    let expected = hex::decode(trace_id).expect("trace ID");
    let mut names = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let Ok(Some(request)) =
            tokio::time::timeout(remaining.min(Duration::from_secs(2)), receiver.recv()).await
        else {
            continue;
        };
        for resource in request.resource_spans {
            for scope in resource.scope_spans {
                for span in scope.spans {
                    if span.trace_id == expected {
                        names.push(span.name);
                    }
                }
            }
        }
        if names.iter().any(|name| name == "agent.session") {
            break;
        }
    }
    assert!(
        names.iter().any(|name| name == "agent.session"),
        "{names:?}"
    );
    while let Ok(Some(_)) = tokio::time::timeout(Duration::from_millis(100), receiver.recv()).await
    {
    }
    collector_task.abort();
}
