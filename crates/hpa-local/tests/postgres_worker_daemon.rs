//! The Hybrid Worker daemon must complete jobs against a PostgreSQL control
//! plane: lease, execute, and write the execution back to PostgreSQL without
//! touching the SQLite mirror.
//!
//! Skipped when `HPA_POSTGRES_TEST_URL` is absent.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};
use sqlx::Row;
use sqlx::sqlite::SqlitePoolOptions;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hpa")
}

fn temp_dir(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("hpa-pg-daemon-{label}-{}", uuid::Uuid::now_v7()));
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

fn run_cli(data_dir: &Path, args: &[&str], database_url: &str) -> String {
    let output = Command::new(binary())
        .arg("--data-dir")
        .arg(data_dir)
        .args(args)
        .env("HPA_POSTGRES_URL", database_url)
        .output()
        .expect("run cli");
    assert!(
        output.status.success(),
        "cli `{args:?}` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn workflow_ir() -> Value {
    json!({
        "$schema": "https://hpa.dev/schema/ir/v0.1/workflow.schema.json",
        "schemaVersion": "hpa.dev/v1",
        "id": "wf_pg_daemon",
        "name": "PG daemon demo",
        "workflowVersion": "1.0.0",
        "nodes": [
            {"id": "start", "type": "start", "typeVersion": "1.0.0", "name": "Start", "config": {}},
            {"id": "end", "type": "end", "typeVersion": "1.0.0", "name": "End", "config": {}}
        ],
        "edges": [
            {"id": "e1", "source": "start", "sourcePort": "out", "target": "end", "targetPort": "in"}
        ]
    })
}

async fn sqlite_count(path: &Path, table: &str) -> i64 {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&format!("sqlite://{}", path.display()))
        .await
        .expect("open sqlite");
    let query = format!("SELECT count(*) FROM {table}");
    let count: i64 = sqlx::query(sqlx::AssertSqlSafe(query))
        .fetch_one(&pool)
        .await
        .expect("count rows")
        .get(0);
    pool.close().await;
    count
}

#[tokio::test]
async fn worker_daemon_completes_a_job_through_postgres() {
    let Ok(database_url) = std::env::var("HPA_POSTGRES_TEST_URL") else {
        eprintln!("skipped: HPA_POSTGRES_TEST_URL is not set");
        return;
    };
    let data_dir = temp_dir("server");
    run_cli(&data_dir, &["db", "migrate"], &database_url);
    let bootstrap: Value =
        serde_json::from_str(&run_cli(&data_dir, &["enterprise", "init"], &database_url))
            .expect("init json");
    let admin_token = bootstrap["token"]
        .as_str()
        .expect("admin token")
        .to_string();

    let port = free_port();
    let base_url = format!("http://127.0.0.1:{port}");
    let server_log = data_dir.join("server.log");
    let server_stderr = std::fs::File::create(&server_log).expect("server log");
    let mut server = Command::new(binary())
        .arg("--data-dir")
        .arg(&data_dir)
        .args(["serve", "--port", &port.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::from(server_stderr))
        .env("HPA_POSTGRES_URL", &database_url)
        .env("HPA_POSTGRES_RUNTIME", "1")
        .env("HPA_ENTERPRISE_MODE", "required")
        .spawn()
        .expect("spawn server");
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("http client");
    let mut healthy = false;
    for _ in 0..400 {
        if let Ok(response) = client.get(format!("{base_url}/api/health")).send().await
            && response.status().is_success()
        {
            healthy = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(healthy, "server did not become healthy");

    let tenant_id = format!("wd_{}", uuid::Uuid::now_v7().simple());
    let tenant = client
        .post(format!("{base_url}/api/enterprise/tenants"))
        .bearer_auth(&admin_token)
        .json(&json!({"name": tenant_id}))
        .send()
        .await
        .expect("create tenant");
    assert_eq!(tenant.status(), 200);
    let tenant: Value = tenant.json().await.expect("tenant json");
    let tenant_id = tenant["id"].as_str().expect("tenant id").to_string();

    let token_response = client
        .post(format!("{base_url}/api/enterprise/tokens"))
        .bearer_auth(&admin_token)
        .json(&json!({"tenantId": tenant_id, "name": "daemon", "role": "owner"}))
        .send()
        .await
        .expect("create token");
    let token_body: Value = token_response.json().await.expect("token json");
    let tenant_token = token_body["token"]
        .as_str()
        .expect("tenant token")
        .to_string();

    let worker_response = client
        .post(format!("{base_url}/api/enterprise/workers"))
        .bearer_auth(&admin_token)
        .json(&json!({
            "tenantId": tenant_id,
            "name": "pg-daemon-worker",
            "kind": "runner",
            "capabilities": ["workflow"],
            "maxConcurrency": 1,
        }))
        .send()
        .await
        .expect("register worker");
    assert_eq!(worker_response.status(), 200);
    let worker: Value = worker_response.json().await.expect("worker json");
    let worker_id = worker["id"].as_str().expect("worker id").to_string();
    let worker_token = worker["workerToken"]
        .as_str()
        .expect("worker token")
        .to_string();

    let created = client
        .post(format!("{base_url}/api/workflows"))
        .bearer_auth(&tenant_token)
        .json(&json!({"ir": workflow_ir()}))
        .send()
        .await
        .expect("create workflow");
    assert_eq!(created.status(), 200, "workflow create must succeed");
    let published = client
        .post(format!("{base_url}/api/workflows/wf_pg_daemon/publish"))
        .bearer_auth(&tenant_token)
        .send()
        .await
        .expect("publish");
    assert_eq!(published.status(), 200);

    let job_response = client
        .post(format!("{base_url}/api/enterprise/worker-jobs"))
        .bearer_auth(&admin_token)
        .json(&json!({
            "tenantId": tenant_id,
            "workflowId": "wf_pg_daemon",
            "workerId": worker_id,
            "input": {},
        }))
        .send()
        .await
        .expect("create job");
    assert_eq!(job_response.status(), 200);
    let job: Value = job_response.json().await.expect("job json");
    let job_id = job["id"].as_str().expect("job id").to_string();
    let execution_id = job["executionId"]
        .as_str()
        .expect("execution id")
        .to_string();

    // The daemon leases and executes the job using the PostgreSQL control plane.
    // A dedicated data dir keeps the daemon's report outbox isolated from the
    // developer's ~/.hpa and from previous test runs.
    let daemon_dir = temp_dir("daemon-state");
    let mut daemon: Child = Command::new(binary())
        .arg("--data-dir")
        .arg(&daemon_dir)
        .args([
            "worker",
            "--control-url",
            &base_url,
            "--max-concurrency",
            "1",
            "--capabilities",
            "workflow",
        ])
        .env("HPA_WORKER_TOKEN", &worker_token)
        .stdout(Stdio::null())
        .stderr(Stdio::from(
            std::fs::File::create(data_dir.join("daemon.log")).expect("daemon log"),
        ))
        .spawn()
        .expect("spawn worker daemon");

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect postgres");

    let mut job_status = String::new();
    for _ in 0..300 {
        let status: Option<String> =
            sqlx::query_scalar("SELECT status FROM hpa_control.worker_jobs WHERE id = $1")
                .bind(&job_id)
                .fetch_optional(&pool)
                .await
                .expect("job status");
        job_status = status.unwrap_or_default();
        if matches!(job_status.as_str(), "succeeded" | "failed" | "cancelled") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let _ = daemon.kill();
    let _ = daemon.wait();
    if job_status != "succeeded" {
        let daemon_log = std::fs::read_to_string(data_dir.join("daemon.log")).unwrap_or_default();
        let server_log = std::fs::read_to_string(&server_log).unwrap_or_default();
        panic!(
            "daemon must complete the job (status: {job_status})\ndaemon:\n{daemon_log}\nserver:\n{server_log}"
        );
    }

    let execution_status: String = sqlx::query_scalar(
        "SELECT status FROM hpa_tenant.executions WHERE tenant_id = $1 AND id = $2",
    )
    .bind(&tenant_id)
    .bind(&execution_id)
    .fetch_one(&pool)
    .await
    .expect("execution status");
    assert_eq!(execution_status, "succeeded");

    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM hpa_tenant.events WHERE tenant_id = $1 AND execution_id = $2",
    )
    .bind(&tenant_id)
    .bind(&execution_id)
    .fetch_one(&pool)
    .await
    .expect("event count");
    assert!(
        events > 0,
        "execution events must be persisted in PostgreSQL"
    );

    assert_eq!(
        sqlite_count(&data_dir.join("control.db"), "enterprise_worker_jobs").await,
        0,
        "the SQLite mirror must stay empty in PostgreSQL mode"
    );

    let _ = client
        .delete(format!("{base_url}/api/enterprise/tenants/{tenant_id}"))
        .bearer_auth(&admin_token)
        .json(&json!({"confirm": true}))
        .send()
        .await;
    pool.close().await;
    let _ = server.kill();
    let _ = server.wait();
    let _ = std::fs::remove_dir_all(&data_dir);
    let _ = std::fs::remove_dir_all(&daemon_dir);
}
