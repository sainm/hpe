//! Worker registry and job API must be PostgreSQL-authoritative when the
//! control plane runs with `HPA_POSTGRES_RUNTIME=1`: the SQLite mirror must not
//! receive registry rows. Skipped when `HPA_POSTGRES_TEST_URL` is absent.

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
    let path = std::env::temp_dir().join(format!("hpa-pg-worker-{label}-{}", uuid::Uuid::now_v7()));
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

struct Server {
    child: Child,
    port: u16,
    client: reqwest::Client,
}

impl Server {
    async fn start(data_dir: &Path, database_url: &str) -> Self {
        let port = free_port();
        let child = Command::new(binary())
            .arg("--data-dir")
            .arg(data_dir)
            .args(["serve", "--port", &port.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .env("HPA_POSTGRES_URL", database_url)
            .env("HPA_POSTGRES_RUNTIME", "1")
            .env("HPA_ENTERPRISE_MODE", "required")
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
        "id": "wf_pg_worker",
        "name": "PG worker demo",
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
async fn worker_registry_and_jobs_skip_sqlite_when_postgres_is_active() {
    let Ok(database_url) = std::env::var("HPA_POSTGRES_TEST_URL") else {
        eprintln!("skipped: HPA_POSTGRES_TEST_URL is not set");
        return;
    };
    let data_dir = temp_dir("registry");

    let migrated = run_cli(&data_dir, &["db", "migrate"], &database_url);
    assert!(
        migrated.contains("\"database\""),
        "migrate output: {migrated}"
    );

    let bootstrap: Value =
        serde_json::from_str(&run_cli(&data_dir, &["enterprise", "init"], &database_url))
            .expect("init json");
    let admin_token = bootstrap["token"]
        .as_str()
        .expect("admin token")
        .to_string();

    let server = Server::start(&data_dir, &database_url).await;

    // Register a worker through the HTTP API.
    let registered = server
        .client
        .post(server.url("/api/enterprise/workers"))
        .bearer_auth(&admin_token)
        .json(&json!({
            "tenantId": "local",
            "name": "pg-worker",
            "kind": "runner",
            "capabilities": ["workflow"],
            "maxConcurrency": 1,
        }))
        .send()
        .await
        .expect("register worker");
    let registration_status = registered.status();
    let worker: Value = registered.json().await.expect("worker json");
    assert_eq!(
        registration_status, 200,
        "worker registration must succeed: {worker}"
    );
    let worker_id = worker["id"].as_str().expect("worker id").to_string();
    let worker_token = worker["workerToken"]
        .as_str()
        .expect("worker token")
        .to_string();
    assert!(worker_token.starts_with("hpa_wrk_"));

    // The registry row must live in PostgreSQL, not in the SQLite mirror.
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect postgres");
    let in_postgres: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM hpa_control.workers WHERE id = $1 AND tenant_id = 'local'",
    )
    .bind(&worker_id)
    .fetch_one(&pool)
    .await
    .expect("count workers");
    assert_eq!(in_postgres, 1, "worker row must exist in PostgreSQL");

    let control_db = data_dir.join("control.db");
    assert_eq!(
        sqlite_count(&control_db, "enterprise_workers").await,
        0,
        "SQLite must not keep a worker registry copy in PostgreSQL mode"
    );

    // Publish a workflow and enqueue a worker job.
    let created = server
        .client
        .post(server.url("/api/workflows"))
        .bearer_auth(&admin_token)
        .json(&json!({ "ir": workflow_ir() }))
        .send()
        .await
        .expect("create workflow");
    assert_eq!(created.status(), 200, "workflow create must succeed");
    let published = server
        .client
        .post(server.url("/api/workflows/wf_pg_worker/publish"))
        .bearer_auth(&admin_token)
        .send()
        .await
        .expect("publish workflow");
    assert_eq!(published.status(), 200, "publish must succeed");

    let job_response = server
        .client
        .post(server.url("/api/enterprise/worker-jobs"))
        .bearer_auth(&admin_token)
        .json(&json!({
            "tenantId": "local",
            "workflowId": "wf_pg_worker",
            "workerId": worker_id,
            "input": {},
        }))
        .send()
        .await
        .expect("create job");
    assert_eq!(job_response.status(), 200, "job creation must succeed");
    let job: Value = job_response.json().await.expect("job json");
    let job_id = job["id"].as_str().expect("job id").to_string();

    let jobs_in_postgres: i64 =
        sqlx::query_scalar("SELECT count(*) FROM hpa_control.worker_jobs WHERE id = $1")
            .bind(&job_id)
            .fetch_one(&pool)
            .await
            .expect("count jobs");
    assert_eq!(jobs_in_postgres, 1, "job row must exist in PostgreSQL");
    assert_eq!(
        sqlite_count(&control_db, "enterprise_worker_jobs").await,
        0,
        "SQLite must not keep a worker job copy in PostgreSQL mode"
    );

    // Lease requires an exact runtime-version match, like the SQLite path.
    let job_version: String =
        sqlx::query_scalar("SELECT runtime_version FROM hpa_control.worker_jobs WHERE id = $1")
            .bind(&job_id)
            .fetch_one(&pool)
            .await
            .expect("job runtime version");

    // The worker leases the job using its one-time token.
    let lease = server
        .client
        .post(server.url("/api/worker/lease"))
        .bearer_auth(&worker_token)
        .json(&json!({"slots": 1, "version": job_version, "capabilities": ["workflow"]}))
        .send()
        .await
        .expect("lease");
    assert_eq!(lease.status(), 200, "lease must succeed");
    let lease_body: Value = lease.json().await.expect("lease json");
    let leased = lease_body["jobs"]
        .as_array()
        .expect("jobs array")
        .iter()
        .any(|entry| entry["jobId"] == json!(job_id));
    assert!(
        leased,
        "leased jobs must include the enqueued job: {lease_body}"
    );
    let status: String =
        sqlx::query_scalar("SELECT status FROM hpa_control.worker_jobs WHERE id = $1")
            .bind(&job_id)
            .fetch_one(&pool)
            .await
            .expect("job status");
    assert_eq!(status, "leased");

    // Disabling the worker must be PostgreSQL-authoritative as well.
    let disabled = server
        .client
        .post(server.url(&format!("/api/enterprise/workers/{worker_id}/disable")))
        .bearer_auth(&admin_token)
        .send()
        .await
        .expect("disable worker");
    assert_eq!(disabled.status(), 200, "disable must succeed");
    let status: String = sqlx::query_scalar("SELECT status FROM hpa_control.workers WHERE id = $1")
        .bind(&worker_id)
        .fetch_one(&pool)
        .await
        .expect("worker status");
    assert_eq!(status, "disabled");
    assert_eq!(
        sqlite_count(&control_db, "enterprise_workers").await,
        0,
        "disable must not create a SQLite row"
    );

    pool.close().await;
    let _ = std::fs::remove_dir_all(&data_dir);
}
