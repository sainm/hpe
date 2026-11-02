use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use hpa_core::compiler::compile_workflow;
use hpa_core::ir::Workflow;
use hpa_core::runtime::{new_state, start_event};
use hpa_local::postgres_execution::{ExecutionRecord, PostgresExecutionStore};
use reqwest::Client;
use serde_json::json;
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
    panic!("recovery runtime on port {port} did not become healthy");
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
            .env("HPA_NODE_ID", "recovery-node")
            .env("HPA_EXECUTION_LEASE_TTL_SECONDS", "1")
            .env("HPA_EXECUTION_RECOVERY_TICK_MS", "100")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn recovery runtime");
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

#[tokio::test]
async fn empty_node_recovers_postgres_execution_and_releases_lease() {
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
    sqlx::query(
        "INSERT INTO hpa_control.tenants (id, name)
         VALUES ('local', 'Local') ON CONFLICT (id) DO NOTHING",
    )
    .execute(&pool)
    .await
    .expect("ensure local tenant");

    let suffix = Uuid::now_v7().simple().to_string();
    let workflow: Workflow =
        serde_json::from_str(include_str!("../../../fixtures/01_sequence.json"))
            .expect("sequence workflow");
    let compilation = compile_workflow(&workflow).expect("compile workflow");
    let store = PostgresExecutionStore::new(pool.clone(), "local");
    store
        .sync_workflow(
            &workflow.to_json_string(false).unwrap(),
            &format!("recovery-{suffix}"),
            "registry-recovery",
            "2026-11-02T00:00:00Z",
            "2026-11-02T00:00:01Z",
        )
        .await
        .expect("sync recovery workflow");
    let mut state = new_state(
        &workflow,
        &compilation.plan,
        format!("exec_recovery_{suffix}"),
        json!({"value": 31}),
        Default::default(),
        "local".to_string(),
    )
    .expect("recovery state");
    state.workflow_version = format!("recovery-{suffix}");
    let start = start_event(&mut state);
    let record = ExecutionRecord {
        id: state.id.clone(),
        workflow_id: workflow.id.clone(),
        workflow_version: state.workflow_version.clone(),
        status: "running".to_string(),
        state_json: serde_json::to_string(&state).unwrap(),
        workflow_json: workflow.to_json_string(false).unwrap(),
        output_json: None,
        error_json: None,
        started_at: state.started_at.clone(),
        updated_at: state.updated_at.clone(),
    };
    store
        .insert_execution(&record, &start)
        .await
        .expect("insert recoverable execution");

    let data_dir: PathBuf = std::env::temp_dir().join(format!("hpa-execution-recovery-{suffix}"));
    let client = Client::new();
    let (mut runtime, port) = ServerGuard::start(&data_dir, &postgres_url);
    wait_for_health(&client, port).await;
    for _ in 0..200 {
        let status: Option<String> = sqlx::query_scalar(
            "SELECT status FROM hpa_tenant.executions
             WHERE tenant_id = 'local' AND id = $1",
        )
        .bind(&state.id)
        .fetch_optional(&pool)
        .await
        .expect("read recovered execution");
        if status.as_deref() == Some("succeeded") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let status: String = sqlx::query_scalar(
        "SELECT status FROM hpa_tenant.executions
         WHERE tenant_id = 'local' AND id = $1",
    )
    .bind(&state.id)
    .fetch_one(&pool)
    .await
    .expect("recovered execution status");
    assert_eq!(status, "succeeded");
    let lease_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM hpa_tenant.execution_leases
         WHERE tenant_id = 'local' AND execution_id = $1",
    )
    .bind(&state.id)
    .fetch_one(&pool)
    .await
    .expect("recovered execution lease count");
    assert_eq!(lease_count, 0);

    runtime.stop();
    sqlx::query(
        "DELETE FROM hpa_tenant.executions
         WHERE tenant_id = 'local' AND id = $1",
    )
    .bind(&state.id)
    .execute(&pool)
    .await
    .expect("clean recovered execution");
    sqlx::query(
        "DELETE FROM hpa_tenant.workflow_versions
         WHERE tenant_id = 'local' AND workflow_id = $1 AND version = $2",
    )
    .bind(&workflow.id)
    .bind(&state.workflow_version)
    .execute(&pool)
    .await
    .expect("clean recovery workflow version");
    pool.close().await;
    let _ = std::fs::remove_dir_all(&data_dir);
}
