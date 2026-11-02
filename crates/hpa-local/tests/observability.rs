use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hpa")
}

fn temp_dir(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "hpa-observability-{label}-{}",
        uuid::Uuid::now_v7()
    ));
    std::fs::create_dir_all(&path).expect("temp dir");
    path
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("addr")
        .port()
}

fn run_cli(data_dir: &Path, args: &[&str]) -> String {
    let output = Command::new(binary())
        .arg("--data-dir")
        .arg(data_dir)
        .args(args)
        .output()
        .expect("run cli");
    assert!(
        output.status.success(),
        "cli `{args:?}` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

struct Server {
    child: Child,
    port: u16,
    client: reqwest::Client,
}

impl Server {
    async fn start(data_dir: &Path, enterprise: bool) -> Self {
        let port = free_port();
        let mut command = Command::new(binary());
        command
            .arg("--data-dir")
            .arg(data_dir)
            .args(["serve", "--port", &port.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .env("HPA_SCHEDULER_TICK_MS", "200");
        if enterprise {
            command.env("HPA_ENTERPRISE_MODE", "required");
        } else {
            command.env_remove("HPA_ENTERPRISE_MODE");
        }
        let child = command.spawn().expect("spawn server");
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("http client");
        let mut healthy = false;
        for _ in 0..400 {
            if let Ok(response) = client
                .get(format!("http://127.0.0.1:{port}/api/health"))
                .send()
                .await
                && response.status().is_success()
            {
                healthy = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if !healthy {
            let mut child = child;
            let _ = child.kill();
            let _ = child.wait();
            panic!("server did not become healthy");
        }
        Self {
            child,
            port,
            client,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn workflow_ir() -> Value {
    json!({
        "$schema": "https://hpa.dev/schema/ir/v0.1/workflow.schema.json",
        "schemaVersion": "hpa.dev/v1",
        "id": "wf_obs",
        "name": "Observable",
        "workflowVersion": "1.0.0",
        "nodes": [
            {"id": "start", "type": "start", "typeVersion": "1.0.0", "name": "Start", "config": {}},
            {"id": "map", "type": "transform", "typeVersion": "1.0.0", "name": "Add one",
             "config": {"mapping": {"value": {"$expr": "1 + 1"}}}},
            {"id": "end", "type": "end", "typeVersion": "1.0.0", "name": "End", "config": {}}
        ],
        "edges": [
            {"id": "e1", "source": "start", "sourcePort": "out", "target": "map", "targetPort": "in"},
            {"id": "e2", "source": "map", "sourcePort": "out", "target": "end", "targetPort": "in"}
        ]
    })
}

#[tokio::test]
async fn metrics_and_readiness_reflect_runtime_activity() {
    let data_dir = temp_dir("local");
    let server = Server::start(&data_dir, false).await;

    // Readiness becomes true once the scheduler has ticked.
    let mut ready = false;
    let mut body = Value::Null;
    for _ in 0..100 {
        let response = server
            .client
            .get(server.url("/api/ready"))
            .send()
            .await
            .expect("ready");
        let status = response.status();
        body = response.json().await.unwrap_or(Value::Null);
        if status.is_success() {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(ready, "readiness never became true: {body}");
    assert_eq!(body["ready"], json!(true));
    assert_eq!(body["checks"]["controlDb"], json!("ok"));
    assert_eq!(body["checks"]["tenantDb"], json!("ok"));
    assert_eq!(body["checks"]["signingKey"], json!("ok"));
    assert_eq!(body["checks"]["scheduler"], json!("ok"));

    // Run one workflow so the counters have something to report.
    server
        .client
        .post(server.url("/api/workflows"))
        .json(&json!({"ir": workflow_ir()}))
        .send()
        .await
        .expect("create");
    server
        .client
        .post(server.url("/api/workflows/wf_obs/publish"))
        .send()
        .await
        .expect("publish");
    let started: Value = server
        .client
        .post(server.url("/api/workflows/wf_obs/executions"))
        .json(&json!({"input": {}}))
        .send()
        .await
        .expect("execute")
        .json()
        .await
        .expect("execution json");
    let execution_id = started["summary"]["id"]
        .as_str()
        .expect("execution id")
        .to_string();
    let mut finished = false;
    for _ in 0..200 {
        let detail: Value = server
            .client
            .get(server.url(&format!("/api/executions/{execution_id}")))
            .send()
            .await
            .expect("detail")
            .json()
            .await
            .expect("detail json");
        let status = detail["summary"]["status"].as_str().unwrap_or_default();
        if status == "succeeded" || status == "failed" {
            finished = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(finished, "execution did not finish");

    let response = server
        .client
        .get(server.url("/metrics"))
        .send()
        .await
        .expect("metrics");
    assert_eq!(response.status(), 200);
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        content_type.starts_with("text/plain; version=0.0.4"),
        "unexpected content type {content_type}"
    );
    let text = response.text().await.expect("metrics text");

    for expected in [
        "# TYPE hpa_workflow_executions_total counter",
        "hpa_workflow_executions_total{status=\"succeeded\"}",
        "# TYPE hpa_workflow_execution_seconds histogram",
        "hpa_workflow_execution_seconds_bucket{le=\"+Inf\"}",
        "hpa_node_executions_total{status=\"succeeded\",type=\"transform\"}",
        "hpa_node_seconds_bucket{type=\"transform\",le=\"+Inf\"}",
        "hpa_http_requests_total{",
        "route=\"/api/health\"",
        "hpa_workflows_total 1",
        "hpa_scheduler_ticks_total",
        "hpa_scheduler_last_tick_timestamp_seconds",
        "hpa_build_info{version=",
    ] {
        assert!(text.contains(expected), "missing `{expected}` in:\n{text}");
    }

    let ready_again: Value = server
        .client
        .get(server.url("/api/ready"))
        .send()
        .await
        .expect("ready again")
        .json()
        .await
        .expect("ready json");
    assert_eq!(ready_again["ready"], json!(true));
}

#[tokio::test]
async fn enterprise_mode_protects_metrics() {
    let data_dir = temp_dir("enterprise");
    let bootstrap: Value =
        serde_json::from_str(&run_cli(&data_dir, &["enterprise", "init"])).expect("init json");
    let admin_token = bootstrap["token"]
        .as_str()
        .expect("admin token")
        .to_string();
    let server = Server::start(&data_dir, true).await;

    let anonymous = server
        .client
        .get(server.url("/metrics"))
        .send()
        .await
        .expect("anonymous metrics");
    assert_eq!(
        anonymous.status(),
        401,
        "metrics must require a platform token"
    );

    let authorized = server
        .client
        .get(server.url("/metrics"))
        .bearer_auth(&admin_token)
        .send()
        .await
        .expect("authorized metrics");
    assert_eq!(authorized.status(), 200);
    let text = authorized.text().await.expect("metrics text");
    assert!(text.contains("hpa_build_info"));

    let ready = server
        .client
        .get(server.url("/api/ready"))
        .send()
        .await
        .expect("ready");
    assert_eq!(ready.status(), 200, "readiness stays open for probes");
}
