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
    panic!("event bus runtime on port {port} did not become healthy");
}

struct ServerGuard(Option<Child>);

impl ServerGuard {
    fn start(data_dir: &Path, postgres_url: &str, consumer: bool) -> (Self, u16) {
        let port = free_port();
        let child = Command::new(binary())
            .arg("--data-dir")
            .arg(data_dir)
            .args(["serve", "--port", &port.to_string()])
            .env("HPA_POSTGRES_RUNTIME", "1")
            .env("HPA_POSTGRES_EVENT_BUS", "1")
            .env("HPA_EVENT_BUS_CONSUMER", if consumer { "1" } else { "0" })
            .env("HPA_POSTGRES_URL", postgres_url)
            .env("HPA_EVENT_BUS_TICK_MS", "100")
            .env("HPA_SCHEDULER_TICK_MS", "200")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn event bus runtime");
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

async fn wait_for_processed(pool: &sqlx::PgPool, idempotency_key: &str) {
    for _ in 0..200 {
        let status: Option<String> = sqlx::query_scalar(
            "SELECT status FROM hpa_control.outbox
             WHERE tenant_id = 'local' AND idempotency_key = $1",
        )
        .bind(idempotency_key)
        .fetch_optional(pool)
        .await
        .expect("read outbox status");
        if status.as_deref() == Some("processed") {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("outbox `{idempotency_key}` did not reach processed");
}

#[tokio::test]
async fn postgres_outbox_event_bus_is_idempotent_and_fail_closed() {
    let Ok(postgres_url) = std::env::var("HPA_POSTGRES_TEST_URL") else {
        eprintln!("skipped: HPA_POSTGRES_TEST_URL is not set");
        return;
    };
    let pool = PgPoolOptions::new()
        .max_connections(6)
        .connect(&postgres_url)
        .await
        .expect("connect PostgreSQL");
    MIGRATOR.run(&pool).await.expect("apply migrations");

    let suffix = Uuid::now_v7().simple().to_string();
    let workflow_id = format!("wf_event_bus_{suffix}");
    let topic = format!("event.bus.{suffix}");
    let event_id = format!("event-bus-{suffix}");
    let idempotency_key = format!("topic-event:{event_id}");
    let publisher_dir: PathBuf =
        std::env::temp_dir().join(format!("hpa-event-bus-publisher-{suffix}"));
    let consumer_dir: PathBuf =
        std::env::temp_dir().join(format!("hpa-event-bus-consumer-{suffix}"));
    let client = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("event bus client");
    let (mut publisher, port) = ServerGuard::start(&publisher_dir, &postgres_url, false);
    wait_for_health(&client, port).await;

    let mut workflow: Value =
        serde_json::from_str(include_str!("../../../fixtures/01_sequence.json"))
            .expect("event workflow fixture");
    workflow["id"] = Value::String(workflow_id.clone());
    workflow["name"] = Value::String("PostgreSQL event bus workflow".to_string());
    workflow["triggers"] = json!([{
        "id": "event",
        "type": "event",
        "config": {"topic": topic}
    }]);
    assert_eq!(
        client
            .post(format!("http://127.0.0.1:{port}/api/workflows"))
            .json(&json!({"ir": workflow}))
            .send()
            .await
            .expect("create event workflow")
            .status(),
        200
    );
    let (mut consumer, consumer_port) = ServerGuard::start(&consumer_dir, &postgres_url, true);
    wait_for_health(&client, consumer_port).await;
    assert_eq!(
        client
            .post(format!(
                "http://127.0.0.1:{port}/api/workflows/{workflow_id}/publish"
            ))
            .send()
            .await
            .expect("publish event workflow")
            .status(),
        200
    );

    let first = client
        .post(format!("http://127.0.0.1:{port}/api/events/{topic}"))
        .header("x-hpa-event-id", &event_id)
        .json(&json!({"value": 21, "source": "bus"}))
        .send()
        .await
        .expect("enqueue event");
    assert_eq!(first.status(), 202);
    let first: Value = first.json().await.expect("enqueue JSON");
    assert_eq!(first["queued"], true);
    assert_eq!(first["status"], "queued");
    let message_id = first["messageId"].as_str().expect("message id").to_string();
    wait_for_processed(&pool, &idempotency_key).await;

    let fire = loop {
        let row: Option<(String, String)> = sqlx::query_as(
            "SELECT id, execution_id FROM hpa_tenant.trigger_fires
             WHERE tenant_id = 'local' AND workflow_id = $1 AND event_id = $2",
        )
        .bind(&workflow_id)
        .bind(&event_id)
        .fetch_optional(&pool)
        .await
        .expect("read event fire");
        if let Some(row) = row {
            break row;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let execution_id = fire.1;
    for _ in 0..200 {
        let status: Option<String> = sqlx::query_scalar(
            "SELECT status FROM hpa_tenant.executions
             WHERE tenant_id = 'local' AND id = $1",
        )
        .bind(&execution_id)
        .fetch_optional(&pool)
        .await
        .expect("read event execution");
        if status.as_deref() == Some("succeeded") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let duplicate = client
        .post(format!("http://127.0.0.1:{port}/api/events/{topic}"))
        .header("x-hpa-event-id", &event_id)
        .json(&json!({"value": 21, "source": "bus"}))
        .send()
        .await
        .expect("duplicate event");
    assert_eq!(duplicate.status(), 202);
    let duplicate: Value = duplicate.json().await.expect("duplicate JSON");
    assert_eq!(duplicate["queued"], false);
    assert_eq!(duplicate["status"], "duplicate");
    assert_eq!(duplicate["messageId"], message_id.as_str());

    let conflict = client
        .post(format!("http://127.0.0.1:{port}/api/events/{topic}"))
        .header("x-hpa-event-id", &event_id)
        .json(&json!({"value": 22, "source": "bus"}))
        .send()
        .await
        .expect("conflicting event");
    assert_eq!(conflict.status(), 409);
    let conflict: Value = conflict.json().await.expect("conflict JSON");
    assert_eq!(conflict["error"]["code"], "EVENT_ID_CONFLICT");

    let fire_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM hpa_tenant.trigger_fires
         WHERE tenant_id = 'local' AND workflow_id = $1 AND event_id = $2",
    )
    .bind(&workflow_id)
    .bind(&event_id)
    .fetch_one(&pool)
    .await
    .expect("count event fire");
    assert_eq!(fire_count, 1);
    let execution_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM hpa_tenant.executions
         WHERE tenant_id = 'local' AND workflow_id = $1",
    )
    .bind(&workflow_id)
    .fetch_one(&pool)
    .await
    .expect("count event executions");
    assert_eq!(execution_count, 1);

    publisher.stop();
    consumer.stop();
    sqlx::query(
        "DELETE FROM hpa_control.outbox
         WHERE tenant_id = 'local' AND idempotency_key = $1",
    )
    .bind(&idempotency_key)
    .execute(&pool)
    .await
    .expect("clean event outbox");
    sqlx::query(
        "DELETE FROM hpa_tenant.executions
         WHERE tenant_id = 'local' AND workflow_id = $1",
    )
    .bind(&workflow_id)
    .execute(&pool)
    .await
    .expect("clean event executions");
    sqlx::query(
        "DELETE FROM hpa_tenant.workflows
         WHERE tenant_id = 'local' AND id = $1",
    )
    .bind(&workflow_id)
    .execute(&pool)
    .await
    .expect("clean event workflow");
    pool.close().await;
    let _ = std::fs::remove_dir_all(&publisher_dir);
    let _ = std::fs::remove_dir_all(&consumer_dir);
}
