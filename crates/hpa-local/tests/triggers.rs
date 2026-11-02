use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

const SECRET: &str = "0123456789abcdef";

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hpa")
}

fn temp_dir(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("hpa-triggers-{label}-{}", uuid::Uuid::now_v7()));
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

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs() as i64
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn sign(secret: &str, timestamp: i64, body: &[u8]) -> String {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes());
    let mut message = format!("{timestamp}.").into_bytes();
    message.extend_from_slice(body);
    hex(ring::hmac::sign(&key, &message).as_ref())
}

fn workflow_ir() -> Value {
    json!({
        "$schema": "https://hpa.dev/schema/ir/v0.1/workflow.schema.json",
        "schemaVersion": "hpa.dev/v1",
        "id": "wf_triggers",
        "name": "Triggered",
        "workflowVersion": "1.0.0",
        "nodes": [
            {"id": "start", "type": "start", "typeVersion": "1.0.0", "name": "Start", "config": {}},
            {"id": "end", "type": "end", "typeVersion": "1.0.0", "name": "End", "config": {}}
        ],
        "edges": [
            {"id": "e1", "source": "start", "sourcePort": "out", "target": "end", "targetPort": "in"}
        ],
        "triggers": [
            {"id": "nightly", "type": "schedule", "config": {"cron": "0 9 * * *", "timezone": "+08:00"}},
            {"id": "hook", "type": "webhook", "config": {"secret": SECRET}},
            {"id": "invoice-created", "type": "event", "config": {"topic": "invoice.created"}}
        ]
    })
}

struct Server {
    child: Child,
    port: u16,
    client: reqwest::Client,
}

impl Server {
    async fn start(data_dir: &Path) -> Self {
        let port = free_port();
        let child = Command::new(binary())
            .arg("--data-dir")
            .arg(data_dir)
            .args(["serve", "--port", &port.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .env("HPA_SCHEDULER_TICK_MS", "200")
            .spawn()
            .expect("spawn server");
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

    async fn post_signed(
        &self,
        path: &str,
        body: &[u8],
        timestamp: i64,
        event_id: Option<&str>,
    ) -> reqwest::Response {
        let mut request = self
            .client
            .post(self.url(path))
            .header("x-hpa-timestamp", timestamp.to_string())
            .header(
                "x-hpa-signature",
                format!("sha256={}", sign(SECRET, timestamp, body)),
            )
            .header("content-type", "application/json")
            .body(body.to_vec());
        if let Some(event_id) = event_id {
            request = request.header("x-hpa-event-id", event_id);
        }
        request.send().await.expect("hook request")
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tokio::test]
async fn triggers_fire_from_webhooks_and_the_api() {
    let data_dir = temp_dir("flow");
    let server = Server::start(&data_dir).await;

    let created = server
        .client
        .post(server.url("/api/workflows"))
        .json(&json!({"ir": workflow_ir()}))
        .send()
        .await
        .expect("create workflow");
    assert_eq!(created.status(), 200, "workflow must be accepted");

    let published = server
        .client
        .post(server.url("/api/workflows/wf_triggers/publish"))
        .send()
        .await
        .expect("publish");
    assert_eq!(published.status(), 200, "workflow must publish");

    let list: Value = server
        .client
        .get(server.url("/api/triggers"))
        .send()
        .await
        .expect("list triggers")
        .json()
        .await
        .expect("triggers json");
    let triggers = list["triggers"].as_array().expect("triggers array");
    assert_eq!(triggers.len(), 3);
    let schedule = triggers
        .iter()
        .find(|row| row["triggerId"] == json!("nightly"))
        .expect("schedule trigger");
    assert_eq!(schedule["kind"], json!("schedule"));
    assert_eq!(schedule["enabled"], json!(true));
    assert_eq!(schedule["workflowVersion"], json!("1.0.0"));
    assert!(schedule["nextFireAt"].as_i64().unwrap_or_default() > 0);
    assert_eq!(schedule["schedule"]["timezone"], json!("+08:00"));
    let webhook = triggers
        .iter()
        .find(|row| row["triggerId"] == json!("hook"))
        .expect("webhook trigger");
    assert_eq!(webhook["webhook"]["hasSecret"], json!(true));
    let hook_url = webhook["webhook"]["url"].as_str().expect("hook url");
    assert!(
        hook_url.ends_with("/hooks/local/wf_triggers/hook"),
        "unexpected hook url {hook_url}"
    );
    let event = triggers
        .iter()
        .find(|row| row["triggerId"] == json!("invoice-created"))
        .expect("event trigger");
    assert_eq!(event["kind"], json!("event"));
    assert_eq!(event["enabled"], json!(true));

    let event_response = server
        .client
        .post(server.url("/api/events/invoice.created"))
        .header("x-hpa-event-id", "event-invoice-1")
        .json(&json!({"invoiceId": "inv_1", "amount": 42}))
        .send()
        .await
        .expect("publish event");
    assert_eq!(event_response.status(), 202);
    let event_body: Value = event_response.json().await.expect("event json");
    assert_eq!(event_body["executions"].as_array().unwrap().len(), 1);
    let duplicate_event = server
        .client
        .post(server.url("/api/events/invoice.created"))
        .header("x-hpa-event-id", "event-invoice-1")
        .json(&json!({"invoiceId": "inv_1"}))
        .send()
        .await
        .expect("duplicate event");
    assert_eq!(duplicate_event.status(), 409);

    // Valid delivery starts an execution.
    let body = br#"{"hello":"world"}"#;
    let accepted = server
        .post_signed(
            "/hooks/wf_triggers/hook",
            body,
            now_seconds(),
            Some("evt-1"),
        )
        .await;
    assert_eq!(accepted.status(), 202, "valid signature is accepted");
    let delivered: Value = accepted.json().await.expect("delivery json");
    let execution_id = delivered["executionId"]
        .as_str()
        .expect("execution id")
        .to_string();
    assert_eq!(delivered["workflowVersion"], json!("1.0.0"));
    let execution = server
        .client
        .get(server.url(&format!("/api/executions/{execution_id}")))
        .send()
        .await
        .expect("get execution");
    assert_eq!(execution.status(), 200, "execution is queryable");

    // The same event id must not start a second execution.
    let duplicate = server
        .post_signed(
            "/hooks/wf_triggers/hook",
            body,
            now_seconds(),
            Some("evt-1"),
        )
        .await;
    assert_eq!(duplicate.status(), 409);

    // Tenant-qualified path works as well.
    let tenant_path = server
        .post_signed("/hooks/local/wf_triggers/hook", body, now_seconds(), None)
        .await;
    assert_eq!(tenant_path.status(), 202);

    // Rejections.
    let unsigned = server
        .client
        .post(server.url("/hooks/wf_triggers/hook"))
        .body(body.to_vec())
        .send()
        .await
        .expect("unsigned");
    assert_eq!(unsigned.status(), 401);
    let stale = server
        .post_signed("/hooks/wf_triggers/hook", body, now_seconds() - 3_600, None)
        .await;
    assert_eq!(stale.status(), 401, "stale timestamp is rejected");
    let forged = server
        .client
        .post(server.url("/hooks/wf_triggers/hook"))
        .header("x-hpa-timestamp", now_seconds().to_string())
        .header("x-hpa-signature", format!("sha256={}", "0".repeat(64)))
        .body(body.to_vec())
        .send()
        .await
        .expect("forged");
    assert_eq!(forged.status(), 401, "bad signature is rejected");
    let unknown = server
        .post_signed("/hooks/wf_triggers/missing", body, now_seconds(), None)
        .await;
    assert_eq!(unknown.status(), 404);

    // Disable then re-enable.
    let disabled = server
        .client
        .post(server.url("/api/triggers/wf_triggers/hook/disable"))
        .send()
        .await
        .expect("disable");
    assert_eq!(disabled.status(), 200);
    let rejected = server
        .post_signed("/hooks/wf_triggers/hook", body, now_seconds(), None)
        .await;
    assert_eq!(rejected.status(), 409, "disabled hooks are rejected");
    let enabled = server
        .client
        .post(server.url("/api/triggers/wf_triggers/hook/enable"))
        .send()
        .await
        .expect("enable");
    assert_eq!(enabled.status(), 200);

    let event_disabled = server
        .client
        .post(server.url("/api/triggers/wf_triggers/invoice-created/disable"))
        .send()
        .await
        .expect("disable event");
    assert_eq!(event_disabled.status(), 200);
    let no_subscriber = server
        .client
        .post(server.url("/api/events/invoice.created"))
        .header("x-hpa-event-id", "event-invoice-disabled")
        .json(&json!({"invoiceId": "inv_2"}))
        .send()
        .await
        .expect("disabled event");
    assert_eq!(no_subscriber.status(), 404);

    // Manual fire through the API.
    let fired = server
        .client
        .post(server.url("/api/triggers/wf_triggers/hook/fire"))
        .json(&json!({"input": {"manual": true}}))
        .send()
        .await
        .expect("manual fire");
    let status = fired.status();
    let fired: Value = fired.json().await.expect("fire json");
    assert_eq!(status, 200, "manual fire failed: {fired}");
    assert!(fired["executionId"].as_str().is_some());

    let fires: Value = server
        .client
        .get(server.url("/api/trigger-fires?limit=10"))
        .send()
        .await
        .expect("fires")
        .json()
        .await
        .expect("fires json");
    assert!(
        fires["fires"].as_array().map(Vec::len).unwrap_or_default() >= 4,
        "each accepted delivery is recorded"
    );
}

#[tokio::test]
async fn invalid_trigger_configuration_fails_validation() {
    let data_dir = temp_dir("invalid");
    let server = Server::start(&data_dir).await;
    let mut ir = workflow_ir();
    ir["triggers"] = json!([
        {"id": "bad", "type": "schedule", "config": {"cron": "not a cron"}}
    ]);
    let response = server
        .client
        .post(server.url("/api/workflows"))
        .json(&json!({"ir": ir}))
        .send()
        .await
        .expect("create invalid");
    assert_eq!(response.status(), 422);
    let body: Value = response.json().await.expect("validation json");
    let details = body["error"]["details"].to_string();
    assert!(
        details.contains("TRIGGER_INVALID"),
        "validation must name the trigger issue: {details}"
    );

    let mut duplicate = workflow_ir();
    duplicate["triggers"] = json!([
        {"id": "same", "type": "schedule", "config": {"cron": "* * * * *"}},
        {"id": "same", "type": "webhook", "config": {}}
    ]);
    let response = server
        .client
        .post(server.url("/api/workflows"))
        .json(&json!({"ir": duplicate}))
        .send()
        .await
        .expect("create duplicate");
    assert_eq!(response.status(), 422);
    let body: Value = response.json().await.expect("validation json");
    assert!(
        body["error"]["details"]
            .to_string()
            .contains("TRIGGER_ID_DUPLICATE")
    );

    let mut invalid_event = workflow_ir();
    invalid_event["triggers"] = json!([
        {"id": "bad-event", "type": "event", "config": {"topic": "contains spaces"}}
    ]);
    let response = server
        .client
        .post(server.url("/api/workflows"))
        .json(&json!({"ir": invalid_event}))
        .send()
        .await
        .expect("create invalid event");
    assert_eq!(response.status(), 422);
    let body: Value = response.json().await.expect("invalid event json");
    assert!(
        body["error"]["details"]
            .to_string()
            .contains("TRIGGER_INVALID")
    );
}
