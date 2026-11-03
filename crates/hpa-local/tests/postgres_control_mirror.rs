//! The control plane must mirror tenants, quotas and members into PostgreSQL so
//! that quota reads work on any node, not only the one that created the tenant.
//!
//! Skipped when `HPA_POSTGRES_TEST_URL` is absent.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hpa")
}

fn temp_dir(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("hpa-pg-mirror-{label}-{}", uuid::Uuid::now_v7()));
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

#[tokio::test]
async fn tenants_quotas_and_members_mirror_into_postgres() {
    let Ok(database_url) = std::env::var("HPA_POSTGRES_TEST_URL") else {
        eprintln!("skipped: HPA_POSTGRES_TEST_URL is not set");
        return;
    };
    let data_dir = temp_dir("a");
    run_cli(&data_dir, &["db", "migrate"], &database_url);
    let bootstrap: Value =
        serde_json::from_str(&run_cli(&data_dir, &["enterprise", "init"], &database_url))
            .expect("init json");
    let admin_token = bootstrap["token"]
        .as_str()
        .expect("admin token")
        .to_string();

    let server_a = Server::start(&data_dir, &database_url).await;

    // The local tenant and its quota are mirrored at startup.
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect postgres");
    let local_tenants: i64 =
        sqlx::query_scalar("SELECT count(*) FROM hpa_control.tenants WHERE id = 'local'")
            .fetch_one(&pool)
            .await
            .expect("count local tenant");
    assert_eq!(local_tenants, 1, "the local tenant must be mirrored");
    let local_quota: i64 =
        sqlx::query_scalar("SELECT count(*) FROM hpa_control.quotas WHERE tenant_id = 'local'")
            .fetch_one(&pool)
            .await
            .expect("count local quota");
    assert_eq!(local_quota, 1, "the local quota must be mirrored");

    // A tenant created through the API lands in PostgreSQL together with its quota.
    let tenant_response = server_a
        .client
        .post(server_a.url("/api/enterprise/tenants"))
        .bearer_auth(&admin_token)
        .json(&json!({"name": format!("mirror_{}", uuid::Uuid::now_v7().simple())}))
        .send()
        .await
        .expect("create tenant");
    assert_eq!(tenant_response.status(), 200);
    let tenant: Value = tenant_response.json().await.expect("tenant json");
    let tenant_id = tenant["id"].as_str().expect("tenant id").to_string();

    let quota_update = server_a
        .client
        .put(server_a.url(&format!("/api/enterprise/quotas/{tenant_id}")))
        .bearer_auth(&admin_token)
        .json(&json!({
            "maxWorkflows": 7,
            "maxExecutionsPerDay": 11,
            "maxSkills": 3,
            "maxAgents": 2,
            "maxPlugins": 1,
            "maxConnections": 4,
            "maxConcurrentRuns": 2,
        }))
        .send()
        .await
        .expect("update quota");
    assert_eq!(quota_update.status(), 200, "quota update must succeed");

    let member = server_a
        .client
        .post(server_a.url("/api/enterprise/members"))
        .bearer_auth(&admin_token)
        .json(&json!({
            "tenantId": tenant_id,
            "subject": "sub-mirror",
            "displayName": "Mirror User",
            "email": "mirror@example.test",
            "role": "developer",
        }))
        .send()
        .await
        .expect("create member");
    assert_eq!(member.status(), 200, "member creation must succeed");

    // The member and the updated quota are both present in PostgreSQL.
    let members: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM hpa_control.members WHERE tenant_id = $1 AND subject = 'sub-mirror'",
    )
    .bind(&tenant_id)
    .fetch_one(&pool)
    .await
    .expect("count members");
    assert_eq!(members, 1, "members must mirror into PostgreSQL");
    let mirrored_quota: i64 = sqlx::query_scalar(
        "SELECT max_executions_per_day FROM hpa_control.quotas WHERE tenant_id = $1",
    )
    .bind(&tenant_id)
    .fetch_one(&pool)
    .await
    .expect("quota row");
    assert_eq!(
        mirrored_quota, 11,
        "quota updates before a restart still reach PostgreSQL through the startup mirror"
    );

    // A node that never saw this tenant reads the quota from PostgreSQL.
    drop(server_a);
    std::fs::remove_dir_all(&data_dir).expect("remove data dir");
    let fresh_dir = temp_dir("b");
    let server_b = Server::start(&fresh_dir, &database_url).await;
    let quota_read = server_b
        .client
        .get(server_b.url(&format!("/api/enterprise/quotas/{tenant_id}")))
        .bearer_auth(&admin_token)
        .send()
        .await
        .expect("read quota");
    assert_eq!(
        quota_read.status(),
        200,
        "the fresh node must answer from PostgreSQL"
    );
    let quota: Value = quota_read.json().await.expect("quota json");
    assert_eq!(quota["maxWorkflows"], json!(7));
    assert_eq!(quota["maxExecutionsPerDay"], json!(11));

    // Tenant resolution reads PostgreSQL: a node that never saw this tenant can
    // register a worker for it, while unknown tenants still 404.
    let cross_node_worker = server_b
        .client
        .post(server_b.url("/api/enterprise/workers"))
        .bearer_auth(&admin_token)
        .json(&json!({
            "tenantId": tenant_id,
            "name": "mirror-node-worker",
            "kind": "runner",
            "capabilities": ["workflow"],
            "maxConcurrency": 1,
        }))
        .send()
        .await
        .expect("register worker on node b");
    assert_eq!(
        cross_node_worker.status(),
        200,
        "tenant resolved from PostgreSQL on the fresh node"
    );
    // Quota writes are PostgreSQL-authoritative: the fresh node can update a
    // tenant it never stored locally.
    let quota_write = server_b
        .client
        .put(server_b.url(&format!("/api/enterprise/quotas/{tenant_id}")))
        .bearer_auth(&admin_token)
        .json(&json!({
            "maxWorkflows": 21,
            "maxExecutionsPerDay": 34,
            "maxSkills": 3,
            "maxAgents": 2,
            "maxPlugins": 1,
            "maxConnections": 4,
            "maxConcurrentRuns": 2,
        }))
        .send()
        .await
        .expect("write quota on node b");
    assert_eq!(
        quota_write.status(),
        200,
        "quota writes must not require a local tenant row"
    );
    let mirrored: i64 = sqlx::query_scalar(
        "SELECT max_executions_per_day FROM hpa_control.quotas WHERE tenant_id = $1",
    )
    .bind(&tenant_id)
    .fetch_one(&pool)
    .await
    .expect("quota row after cross-node write");
    assert_eq!(mirrored, 34, "the other node's write must reach PostgreSQL");

    let unknown = server_b
        .client
        .post(server_b.url("/api/enterprise/workers"))
        .bearer_auth(&admin_token)
        .json(&json!({
            "tenantId": format!("missing_{}", uuid::Uuid::now_v7().simple()),
            "name": "ghost",
            "kind": "runner",
        }))
        .send()
        .await
        .expect("register worker for unknown tenant");
    assert_eq!(
        unknown.status(),
        404,
        "unknown tenants must still be rejected"
    );

    let _ = server_b
        .client
        .delete(server_b.url(&format!("/api/enterprise/tenants/{tenant_id}")))
        .bearer_auth(&admin_token)
        .json(&json!({"confirm": true}))
        .send()
        .await;
    pool.close().await;
    let _ = std::fs::remove_dir_all(&fresh_dir);
}
