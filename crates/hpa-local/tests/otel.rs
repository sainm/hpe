use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{StatusCode, Uri, header};
use axum::routing::post;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message;
use serde_json::json;
use tokio::sync::mpsc;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hpa")
}

fn temp_dir() -> PathBuf {
    let path = std::env::temp_dir().join(format!("hpa-otel-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&path).expect("data directory");
    path
}

async fn wait_for_health(port: u16) {
    let client = reqwest::Client::new();
    for _ in 0..100 {
        if let Ok(response) = client
            .get(format!("http://127.0.0.1:{port}/api/health"))
            .send()
            .await
            && response.status().is_success()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("server did not become healthy");
}

async fn collect_trace_request(
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

async fn collect_any_trace_request(
    State(sender): State<mpsc::UnboundedSender<ExportTraceServiceRequest>>,
    uri: Uri,
    body: Bytes,
) -> (
    StatusCode,
    [(axum::http::HeaderName, &'static str); 1],
    Bytes,
) {
    eprintln!("OTLP collector received unexpected path {}", uri.path());
    collect_trace_request(State(sender), body).await
}

#[tokio::test]
async fn exports_bounded_otlp_spans_and_propagates_w3c_context() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("collector listener");
    let collector_port = listener.local_addr().expect("collector address").port();
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let collector = Router::new()
        .route("/v1/traces", post(collect_trace_request))
        .fallback(collect_any_trace_request)
        .with_state(sender);
    let collector_task = tokio::spawn(async move {
        axum::serve(listener, collector)
            .await
            .expect("serve collector");
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
        .env("RUST_LOG", "trace")
        .env("HPA_OTEL_QUEUE_SIZE", "256")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start HPA server");
    wait_for_health(port).await;

    let trace_id = "4bf92f3577b34da6a3ce929d0e0e4736";
    let traceparent = format!("00-{trace_id}-00f067aa0ba902b7-01");
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("HTTP client");
    let health = client
        .get(format!("http://127.0.0.1:{port}/api/health"))
        .header("traceparent", &traceparent)
        .send()
        .await
        .expect("traced health");
    assert!(health.status().is_success());

    let workflow: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/01_sequence.json"))
            .expect("workflow fixture");
    let create = client
        .post(format!("http://127.0.0.1:{port}/api/workflows"))
        .header("traceparent", &traceparent)
        .json(&json!({"ir": workflow}))
        .send()
        .await
        .expect("create workflow");
    assert!(
        create.status().is_success(),
        "{}",
        create.text().await.unwrap()
    );
    let publish = client
        .post(format!(
            "http://127.0.0.1:{port}/api/workflows/wf_fixture_sequence/publish"
        ))
        .header("traceparent", &traceparent)
        .send()
        .await
        .expect("publish workflow");
    assert!(publish.status().is_success());
    let execution = client
        .post(format!(
            "http://127.0.0.1:{port}/api/workflows/wf_fixture_sequence/executions"
        ))
        .header("traceparent", &traceparent)
        .json(&json!({"input": {"value": 1}}))
        .send()
        .await
        .expect("start execution");
    assert!(execution.status().is_success());

    let expected_trace = hex::decode(trace_id).expect("trace ID bytes");
    let mut all_spans = Vec::new();
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
                all_spans.extend(scope.spans);
            }
        }
        if all_spans
            .iter()
            .any(|span| span.name == "workflow.execute" && span.trace_id == expected_trace)
        {
            break;
        }
    }
    assert!(
        all_spans
            .iter()
            .any(|span| { span.name == "http.server" && span.trace_id == expected_trace }),
        "missing HTTP server span: {:?}",
        all_spans
            .iter()
            .map(|span| (span.name.as_str(), hex::encode(span.trace_id.as_slice())))
            .collect::<Vec<_>>()
    );
    assert!(
        all_spans
            .iter()
            .any(|span| { span.name == "workflow.execute" && span.trace_id == expected_trace }),
        "missing workflow execution span: {:?}",
        all_spans
            .iter()
            .map(|span| span.name.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        all_spans
            .iter()
            .any(|span| span.name == "workflow.step" && span.trace_id == expected_trace)
    );

    let _ = child.kill();
    let _ = child.wait();
    collector_task.abort();
    let _ = std::fs::remove_dir_all(data_dir);
}

#[tokio::test]
async fn collector_failure_does_not_break_readiness() {
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
        .env("HPA_OTEL_ENDPOINT", "http://127.0.0.1:1")
        .env("HPA_OTEL_QUEUE_SIZE", "64")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start HPA server");
    wait_for_health(port).await;
    let client = reqwest::Client::new();
    for _ in 0..100 {
        let response = client
            .get(format!("http://127.0.0.1:{port}/api/nodes"))
            .send()
            .await
            .expect("request with unavailable collector");
        assert!(response.status().is_success());
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(data_dir);
}
