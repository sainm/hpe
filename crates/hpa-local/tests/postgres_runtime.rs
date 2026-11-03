use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use reqwest::Client;
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations/postgres");

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hpa")
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("address")
        .port()
}

async fn wait_for_health(client: &Client, port: u16) {
    for _ in 0..200 {
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
    panic!("runtime on port {port} did not become healthy");
}

struct ServerGuard(Option<Child>);

impl ServerGuard {
    fn start(data_dir: &Path, postgres_url: &str) -> (Self, u16) {
        let port = free_port();
        let child = Command::new(binary())
            .arg("--data-dir")
            .arg(data_dir)
            .args(["serve", "--port", &port.to_string()])
            .env("HPA_POSTGRES_RUNTIME", "1")
            .env("HPA_POSTGRES_URL", postgres_url)
            .env("HPA_SCHEDULER_TICK_MS", "200")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn PostgreSQL runtime");
        (Self(Some(child)), port)
    }

    fn stop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn wait_execution(client: &Client, port: u16, execution_id: &str) -> Value {
    for _ in 0..200 {
        if let Ok(response) = client
            .get(format!(
                "http://127.0.0.1:{port}/api/executions/{execution_id}"
            ))
            .send()
            .await
            && response.status().is_success()
        {
            let body: Value = response.json().await.expect("execution JSON");
            if body["summary"]["status"] == "succeeded" {
                return body;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("execution `{execution_id}` did not succeed");
}

#[tokio::test]
async fn server_routes_execution_events_and_artifacts_to_postgres_across_restart() {
    let Ok(postgres_url) = std::env::var("HPA_POSTGRES_TEST_URL") else {
        eprintln!("skipped: HPA_POSTGRES_TEST_URL is not set");
        return;
    };
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&postgres_url)
        .await
        .expect("connect PostgreSQL");
    MIGRATOR.run(&pool).await.expect("apply migrations");

    let suffix = Uuid::now_v7().simple().to_string();
    let data_dir: PathBuf = std::env::temp_dir().join(format!("hpa-pg-runtime-{suffix}"));
    let client = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("runtime client");
    let (mut runtime, runtime_port) = ServerGuard::start(&data_dir, &postgres_url);
    wait_for_health(&client, runtime_port).await;

    let workflow: Value = serde_json::from_str(include_str!("../../../fixtures/01_sequence.json"))
        .expect("sequence workflow");
    let created = client
        .post(format!("http://127.0.0.1:{runtime_port}/api/workflows"))
        .json(&json!({"ir": workflow}))
        .send()
        .await
        .expect("create workflow");
    assert_eq!(created.status(), 200);
    let published = client
        .post(format!(
            "http://127.0.0.1:{}/api/workflows/wf_fixture_sequence/publish",
            runtime_port
        ))
        .send()
        .await
        .expect("publish workflow");
    assert_eq!(published.status(), 200);
    let spoofed_tenant = client
        .post(format!(
            "http://127.0.0.1:{runtime_port}/api/workflows/wf_fixture_sequence/executions"
        ))
        .json(&json!({
            "input": {"value": 11},
            "tenantId": "forged-tenant"
        }))
        .send()
        .await
        .expect("spoofed tenant execution");
    assert_eq!(spoofed_tenant.status(), 400);
    let spoofed_tenant: Value = spoofed_tenant.json().await.expect("tenant error JSON");
    assert_eq!(spoofed_tenant["error"]["code"], "TENANT_MISMATCH");
    let started = client
        .post(format!(
            "http://127.0.0.1:{}/api/workflows/wf_fixture_sequence/executions",
            runtime_port
        ))
        .json(&json!({"input": {"value": 11}}))
        .send()
        .await
        .expect("start workflow");
    assert_eq!(started.status(), 200);
    let started: Value = started.json().await.expect("start response");
    let execution_id = started["summary"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("execution id missing: {started}"))
        .to_string();
    let completed = wait_execution(&client, runtime_port, &execution_id).await;
    assert_eq!(completed["summary"]["workflowId"], "wf_fixture_sequence");

    let idempotent_start = || {
        client
            .post(format!(
                "http://127.0.0.1:{runtime_port}/api/workflows/wf_fixture_sequence/executions"
            ))
            .header("Idempotency-Key", "pg-start-idempotent")
            .json(&json!({"input": {"value": 12}}))
            .send()
    };
    let first: Value = idempotent_start()
        .await
        .expect("first idempotent start")
        .json()
        .await
        .expect("first idempotent response");
    let replay: Value = idempotent_start()
        .await
        .expect("replayed idempotent start")
        .json()
        .await
        .expect("replayed idempotent response");
    assert_eq!(first["summary"]["id"], replay["summary"]["id"]);
    let conflict = client
        .post(format!(
            "http://127.0.0.1:{runtime_port}/api/workflows/wf_fixture_sequence/executions"
        ))
        .header("Idempotency-Key", "pg-start-idempotent")
        .json(&json!({"input": {"value": 99}}))
        .send()
        .await
        .expect("conflicting idempotent start");
    assert_eq!(conflict.status(), 409);
    let conflict: Value = conflict.json().await.expect("conflict response");
    assert_eq!(conflict["error"]["code"], "IDEMPOTENCY_CONFLICT");
    let replay_id = first["summary"]["id"]
        .as_str()
        .expect("idempotent execution id");
    let _ = wait_execution(&client, runtime_port, replay_id).await;
    let retry = || {
        client
            .post(format!(
                "http://127.0.0.1:{runtime_port}/api/executions/{replay_id}/retry"
            ))
            .header("Idempotency-Key", "pg-retry-idempotent")
            .send()
    };
    let first_retry: Value = retry()
        .await
        .expect("first retry")
        .json()
        .await
        .expect("first retry response");
    let replay_retry: Value = retry()
        .await
        .expect("replayed retry")
        .json()
        .await
        .expect("replayed retry response");
    assert_eq!(first_retry["summary"]["id"], replay_retry["summary"]["id"]);
    assert_ne!(
        first_retry["summary"]["id"],
        Value::String(replay_id.to_string())
    );

    let stored: bool = sqlx::query_scalar(
        "SELECT EXISTS(
             SELECT 1 FROM hpa_tenant.executions
             WHERE tenant_id = 'local' AND id = $1 AND status = 'succeeded'
         )",
    )
    .bind(&execution_id)
    .fetch_one(&pool)
    .await
    .expect("verify PostgreSQL execution");
    assert!(stored);
    let event_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM hpa_tenant.events
         WHERE tenant_id = 'local' AND execution_id = $1",
    )
    .bind(&execution_id)
    .fetch_one(&pool)
    .await
    .expect("count PostgreSQL events");
    assert!(event_count >= 2);
    let artifact_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM hpa_tenant.artifacts
         WHERE tenant_id = 'local' AND execution_id = $1",
    )
    .bind(&execution_id)
    .fetch_one(&pool)
    .await
    .expect("count PostgreSQL artifacts");
    assert_eq!(artifact_count, 1);

    let trigger_workflow_id = format!("wf_pg_event_{suffix}");
    let event_topic = format!("orders.created.{suffix}");
    let mut trigger_workflow: Value =
        serde_json::from_str(include_str!("../../../fixtures/01_sequence.json"))
            .expect("trigger workflow fixture");
    trigger_workflow["id"] = Value::String(trigger_workflow_id.clone());
    trigger_workflow["name"] = Value::String("PostgreSQL event workflow".to_string());
    trigger_workflow["triggers"] = json!([{
        "id": "orders",
        "type": "event",
        "config": {"topic": event_topic}
    }]);
    assert_eq!(
        client
            .post(format!("http://127.0.0.1:{runtime_port}/api/workflows"))
            .json(&json!({"ir": trigger_workflow}))
            .send()
            .await
            .expect("create trigger workflow")
            .status(),
        200
    );
    assert_eq!(
        client
            .post(format!(
                "http://127.0.0.1:{runtime_port}/api/workflows/{trigger_workflow_id}/publish"
            ))
            .send()
            .await
            .expect("publish trigger workflow")
            .status(),
        200
    );
    let versions = client
        .get(format!(
            "http://127.0.0.1:{runtime_port}/api/workflows/{trigger_workflow_id}/versions"
        ))
        .send()
        .await
        .expect("workflow versions");
    assert_eq!(versions.status(), 200);
    let versions: Vec<Value> = versions.json().await.expect("workflow versions JSON");
    assert!(
        versions.iter().any(|version| version["version"] == "1.0.0"),
        "published version missing: {versions:?}"
    );
    let sqlite = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&format!("sqlite://{}", data_dir.join("hpa.db").display()))
        .await
        .expect("open runtime SQLite");
    let sqlite_version: Option<String> = sqlx::query_scalar(
        "SELECT version FROM workflow_versions WHERE workflow_id = ? AND version = '1.0.0'",
    )
    .bind(&trigger_workflow_id)
    .fetch_optional(&sqlite)
    .await
    .expect("read SQLite workflow version");
    assert_eq!(sqlite_version, None);
    sqlite.close().await;
    let event_id = format!("event-{suffix}");
    let event = client
        .post(format!(
            "http://127.0.0.1:{runtime_port}/api/events/{event_topic}"
        ))
        .header("x-hpa-event-id", &event_id)
        .json(&json!({"value": 12, "orderId": "order-pg"}))
        .send()
        .await
        .expect("publish topic event");
    let event_status = event.status();
    let event: Value = event.json().await.expect("event response");
    assert_eq!(event_status, 202, "event response: {event}");
    let event_execution_id = event["executions"][0]["executionId"]
        .as_str()
        .expect("event execution id")
        .to_string();
    wait_execution(&client, runtime_port, &event_execution_id).await;
    let trigger_state_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM hpa_tenant.trigger_state
         WHERE tenant_id = 'local' AND workflow_id = $1 AND kind = 'event'",
    )
    .bind(&trigger_workflow_id)
    .fetch_one(&pool)
    .await
    .expect("count PostgreSQL trigger state");
    assert_eq!(trigger_state_count, 1);
    let fire_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM hpa_tenant.trigger_fires
         WHERE tenant_id = 'local' AND workflow_id = $1 AND event_id = $2",
    )
    .bind(&trigger_workflow_id)
    .bind(&event_id)
    .fetch_one(&pool)
    .await
    .expect("count PostgreSQL trigger fire");
    assert_eq!(fire_count, 1);
    let duplicate = client
        .post(format!(
            "http://127.0.0.1:{runtime_port}/api/events/{event_topic}"
        ))
        .header("x-hpa-event-id", &event_id)
        .json(&json!({"value": 12, "orderId": "order-pg"}))
        .send()
        .await
        .expect("duplicate topic event");
    assert_eq!(duplicate.status(), 409);
    let fire_count_after: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM hpa_tenant.trigger_fires
         WHERE tenant_id = 'local' AND workflow_id = $1 AND event_id = $2",
    )
    .bind(&trigger_workflow_id)
    .bind(&event_id)
    .fetch_one(&pool)
    .await
    .expect("duplicate PostgreSQL trigger fire");
    assert_eq!(fire_count_after, 1);

    runtime.stop();
    let (mut restarted, restarted_port) = ServerGuard::start(&data_dir, &postgres_url);
    wait_for_health(&client, restarted_port).await;
    let reloaded = client
        .get(format!(
            "http://127.0.0.1:{}/api/executions/{execution_id}",
            restarted_port
        ))
        .send()
        .await
        .expect("reload execution");
    assert_eq!(reloaded.status(), 200);
    let reloaded: Value = reloaded.json().await.expect("reloaded execution JSON");
    assert_eq!(reloaded["summary"]["status"], "succeeded");
    let events = client
        .get(format!(
            "http://127.0.0.1:{restarted_port}/api/executions/{execution_id}/events?after=-1&limit=100"
        ))
        .send()
        .await
        .expect("reload events");
    assert_eq!(events.status(), 200);
    let events: Vec<Value> = events.json().await.expect("events JSON");
    assert!(!events.is_empty());

    restarted.stop();
    sqlx::query(
        "DELETE FROM hpa_tenant.executions
         WHERE tenant_id = 'local' AND id = $1",
    )
    .bind(&execution_id)
    .execute(&pool)
    .await
    .expect("clean PostgreSQL execution");
    sqlx::query(
        "DELETE FROM hpa_tenant.executions
         WHERE tenant_id = 'local' AND id = $1",
    )
    .bind(&event_execution_id)
    .execute(&pool)
    .await
    .expect("clean PostgreSQL event execution");
    sqlx::query(
        "DELETE FROM hpa_tenant.workflows
         WHERE tenant_id = 'local' AND id = $1",
    )
    .bind(&trigger_workflow_id)
    .execute(&pool)
    .await
    .expect("clean PostgreSQL trigger workflow");
    pool.close().await;
    let _ = std::fs::remove_dir_all(&data_dir);
}
