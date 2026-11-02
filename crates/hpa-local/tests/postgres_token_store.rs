//! Platform tokens and audit events must be PostgreSQL-authoritative when the
//! control plane runs with `HPA_POSTGRES_RUNTIME=1`, so a node can lose its
//! local `control.db` and still authenticate the same admin token.
//!
//! Skipped when `HPA_POSTGRES_TEST_URL` is absent.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::Value;
use sqlx::Row;
use sqlx::sqlite::SqlitePoolOptions;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hpa")
}

fn temp_dir(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("hpa-pg-token-{label}-{}", uuid::Uuid::now_v7()));
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
async fn admin_token_and_audit_survive_a_lost_data_dir() {
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
    // The bootstrap token must be created in PostgreSQL, not only in SQLite.
    assert_eq!(
        sqlite_count(&data_dir.join("control.db"), "enterprise_tokens").await,
        0,
        "enterprise init must not leave the bootstrap token in SQLite"
    );

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect postgres");
    let stored: i64 = sqlx::query_scalar("SELECT count(*) FROM hpa_control.tokens")
        .fetch_one(&pool)
        .await
        .expect("count tokens");
    assert!(stored >= 1, "bootstrap token must exist in PostgreSQL");

    let server_a = Server::start(&data_dir, &database_url).await;
    let authorized = server_a
        .client
        .get(server_a.url("/api/enterprise/tenants"))
        .bearer_auth(&admin_token)
        .send()
        .await
        .expect("list tenants");
    assert_eq!(authorized.status(), 200, "token works on the first node");

    let audit_rows: i64 = sqlx::query_scalar("SELECT count(*) FROM hpa_control.audit")
        .fetch_one(&pool)
        .await
        .expect("count audit");
    assert!(audit_rows >= 1, "audit events must land in PostgreSQL");

    // Create a tenant-scoped token through the API and confirm SQLite stays empty.
    let tenant_response = server_a
        .client
        .post(server_a.url("/api/enterprise/tenants"))
        .bearer_auth(&admin_token)
        .json(&serde_json::json!({"name": format!("tk_{}", uuid::Uuid::now_v7().simple())}))
        .send()
        .await
        .expect("create tenant");
    assert_eq!(tenant_response.status(), 200);
    let tenant: Value = tenant_response.json().await.expect("tenant json");
    let tenant_id = tenant["id"].as_str().expect("tenant id").to_string();

    let token_response = server_a
        .client
        .post(server_a.url("/api/enterprise/tokens"))
        .bearer_auth(&admin_token)
        .json(&serde_json::json!({
            "tenantId": tenant_id,
            "name": "pg-token",
            "role": "developer",
        }))
        .send()
        .await
        .expect("create token");
    assert_eq!(token_response.status(), 200, "token creation must succeed");
    let created: Value = token_response.json().await.expect("token json");
    let tenant_token = created["token"].as_str().expect("token value").to_string();
    let token_id = created["id"].as_str().expect("token id").to_string();

    let listed = server_a
        .client
        .get(server_a.url(&format!("/api/enterprise/tokens?tenantId={tenant_id}")))
        .bearer_auth(&admin_token)
        .send()
        .await
        .expect("list tokens");
    assert_eq!(listed.status(), 200);
    let listed: Value = listed.json().await.expect("token list");
    assert!(
        listed["tokens"].as_array().is_some_and(|tokens| tokens
            .iter()
            .any(|entry| entry["id"] == serde_json::json!(token_id))),
        "listed tokens must include the new token: {listed}"
    );

    // Simulate a node losing its local state: drop the whole data dir.
    drop(server_a);
    std::fs::remove_dir_all(&data_dir).expect("remove data dir");

    let fresh_dir = temp_dir("b");
    let server_b = Server::start(&fresh_dir, &database_url).await;
    let after_loss = server_b
        .client
        .get(server_b.url("/api/enterprise/tenants"))
        .bearer_auth(&admin_token)
        .send()
        .await
        .expect("list tenants on node b");
    assert_eq!(
        after_loss.status(),
        200,
        "the same admin token must authenticate on a node with an empty data dir"
    );
    let scoped = server_b
        .client
        .get(server_b.url("/api/enterprise/tokens"))
        .bearer_auth(&tenant_token)
        .send()
        .await
        .expect("scoped token probe");
    assert!(
        matches!(scoped.status().as_u16(), 200 | 403),
        "a tenant-scoped token is recognized (RBAC may deny): {}",
        scoped.status()
    );
    assert_eq!(
        sqlite_count(&fresh_dir.join("control.db"), "enterprise_tokens").await,
        0,
        "the fresh node must not grow a SQLite token table"
    );

    // Revocation is shared as well.
    let revoked = server_b
        .client
        .delete(server_b.url(&format!("/api/enterprise/tokens/{token_id}")))
        .bearer_auth(&admin_token)
        .send()
        .await
        .expect("revoke token");
    assert_eq!(revoked.status(), 200);
    let rejected = server_b
        .client
        .get(server_b.url("/api/enterprise/tokens"))
        .bearer_auth(&tenant_token)
        .send()
        .await
        .expect("revoked token probe");
    assert_eq!(rejected.status(), 401, "a revoked token must be rejected");

    pool.close().await;
    let _ = std::fs::remove_dir_all(&fresh_dir);
}
