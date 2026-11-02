use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::routing::post;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use prost::Message;
use serde_json::json;
use tokio::sync::mpsc;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hpa")
}

fn temp_dir() -> PathBuf {
    let path = std::env::temp_dir().join(format!("hpa-otel-metrics-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&path).expect("data dir");
    path
}

async fn wait_for_health(port: u16) {
    let client = reqwest::Client::new();
    for _ in 0..100 {
        if client
            .get(format!("http://127.0.0.1:{port}/api/health"))
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("server did not become healthy");
}

#[derive(Clone)]
struct CollectorState {
    metrics: mpsc::UnboundedSender<ExportMetricsServiceRequest>,
    logs: mpsc::UnboundedSender<ExportLogsServiceRequest>,
}

async fn collect_metrics(
    State(state): State<CollectorState>,
    body: Bytes,
) -> (
    StatusCode,
    [(axum::http::HeaderName, &'static str); 1],
    Bytes,
) {
    if let Ok(request) = ExportMetricsServiceRequest::decode(body.as_ref()) {
        let _ = state.metrics.send(request);
    }
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/x-protobuf")],
        Bytes::new(),
    )
}

async fn collect_logs(
    State(state): State<CollectorState>,
    body: Bytes,
) -> (
    StatusCode,
    [(axum::http::HeaderName, &'static str); 1],
    Bytes,
) {
    if let Ok(request) = ExportLogsServiceRequest::decode(body.as_ref()) {
        let _ = state.logs.send(request);
    }
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/x-protobuf")],
        Bytes::new(),
    )
}

#[tokio::test]
async fn exports_otlp_metrics_and_logs_without_blocking_runtime() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("collector listener");
    let collector_port = listener.local_addr().expect("collector").port();
    let (metric_sender, mut metric_receiver) = mpsc::unbounded_channel();
    let (log_sender, mut log_receiver) = mpsc::unbounded_channel();
    let collector = Router::new()
        .route("/v1/metrics", post(collect_metrics))
        .route("/v1/logs", post(collect_logs))
        .with_state(CollectorState {
            metrics: metric_sender,
            logs: log_sender,
        });
    let collector_task = tokio::spawn(async move {
        axum::serve(listener, collector).await.expect("collector");
    });

    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("HPA port")
        .local_addr()
        .expect("HPA address")
        .port();
    let data_dir = temp_dir();
    let mut child: Child = Command::new(binary())
        .arg("--data-dir")
        .arg(&data_dir)
        .args(["serve", "--port", &port.to_string()])
        .env("HPA_OTEL_ENABLED", "1")
        .env(
            "HPA_OTEL_ENDPOINT",
            format!("http://127.0.0.1:{collector_port}"),
        )
        .env("HPA_OTEL_METRIC_INTERVAL_SECONDS", "1")
        .env("HPA_OTEL_QUEUE_SIZE", "256")
        .env("RUST_LOG", "hpa=info")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start HPA");
    wait_for_health(port).await;

    let client = reqwest::Client::new();
    let workflow: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/01_sequence.json")).expect("workflow");
    assert!(
        client
            .post(format!("http://127.0.0.1:{port}/api/workflows"))
            .json(&json!({"ir": workflow}))
            .send()
            .await
            .expect("create")
            .status()
            .is_success()
    );
    assert!(
        client
            .post(format!(
                "http://127.0.0.1:{port}/api/workflows/wf_fixture_sequence/publish"
            ))
            .send()
            .await
            .expect("publish")
            .status()
            .is_success()
    );
    assert!(
        client
            .post(format!(
                "http://127.0.0.1:{port}/api/workflows/wf_fixture_sequence/executions"
            ))
            .json(&json!({"input": {"value": 3}}))
            .send()
            .await
            .expect("execute")
            .status()
            .is_success()
    );

    let mut metric_names = Vec::new();
    let mut log_bodies = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(12);
    while tokio::time::Instant::now() < deadline {
        tokio::select! {
            Some(request) = metric_receiver.recv() => {
                for resource in request.resource_metrics {
                    for scope in resource.scope_metrics {
                        metric_names.extend(scope.metrics.into_iter().map(|metric| metric.name));
                    }
                }
            }
            Some(request) = log_receiver.recv() => {
                for resource in request.resource_logs {
                    for scope in resource.scope_logs {
                        for record in scope.log_records {
                            if let Some(body) = record.body
                                && let Some(
                                    opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(text)
                                ) = body.value
                            {
                                log_bodies.push(text);
                            }
                        }
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
        if metric_names
            .iter()
            .any(|name| name == "hpa_workflow_executions_total")
            && log_bodies
                .iter()
                .any(|body| body.contains("execution finished"))
        {
            break;
        }
    }
    assert!(
        metric_names
            .iter()
            .any(|name| name == "hpa_http_requests_total"),
        "missing HTTP OTLP metric: {metric_names:?}"
    );
    assert!(
        metric_names
            .iter()
            .any(|name| name == "hpa_workflow_executions_total"),
        "missing workflow OTLP metric: {metric_names:?}"
    );
    assert!(
        log_bodies
            .iter()
            .any(|body| body.contains("execution finished")),
        "missing execution OTLP log: {log_bodies:?}"
    );

    let metric_shutdown_started = tokio::time::Instant::now();
    let _ = child.kill();
    let _ = child.wait();
    assert!(metric_shutdown_started.elapsed() < Duration::from_secs(5));
    collector_task.abort();
    let _ = std::fs::remove_dir_all(data_dir);
}
