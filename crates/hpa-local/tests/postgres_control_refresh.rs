//! SCIM writes still land in SQLite, so the periodic control-plane mirror must
//! carry them into PostgreSQL without a restart.
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
    let path =
        std::env::temp_dir().join(format!("hpa-pg-refresh-{label}-{}", uuid::Uuid::now_v7()));
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
            // Aggressive mirror interval so the test does not sleep for seconds.
            .env("HPA_CONTROL_SYNC_INTERVAL_MS", "100")
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
async fn periodic_mirror_carries_scim_members_into_postgres() {
    let Ok(database_url) = std::env::var("HPA_POSTGRES_TEST_URL") else {
        eprintln!("skipped: HPA_POSTGRES_TEST_URL is not set");
        return;
    };
    let data_dir = temp_dir("node");
    run_cli(&data_dir, &["db", "migrate"], &database_url);
    let bootstrap: Value =
        serde_json::from_str(&run_cli(&data_dir, &["enterprise", "init"], &database_url))
            .expect("init json");
    let admin_token = bootstrap["token"]
        .as_str()
        .expect("admin token")
        .to_string();

    let server = Server::start(&data_dir, &database_url).await;
    let tenant_id = format!("rf_{}", uuid::Uuid::now_v7().simple());
    let tenant = server
        .client
        .post(server.url("/api/enterprise/tenants"))
        .bearer_auth(&admin_token)
        .json(&json!({"name": tenant_id}))
        .send()
        .await
        .expect("create tenant");
    assert_eq!(tenant.status(), 200);
    let tenant: Value = tenant.json().await.expect("tenant json");
    let tenant_id = tenant["id"].as_str().expect("tenant id").to_string();

    // SCIM writes the member through the SQLite path only.
    let user_name = format!("mirror-{}@example.test", uuid::Uuid::now_v7().simple());
    let created = server
        .client
        .post(server.url("/scim/v2/Users"))
        .bearer_auth(&admin_token)
        .header("content-type", "application/scim+json")
        .json(&json!({
            "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
            "userName": user_name,
            "displayName": "Mirror User",
            "emails": [{"value": user_name, "type": "work", "primary": true}],
            "active": true,
            "urn:hpa:params:scim:schemas:extension:role:1.0": {"role": "auditor"},
        }))
        .send()
        .await
        .expect("scim create user");
    assert_eq!(created.status(), 201, "SCIM user creation must succeed");

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect postgres");

    // The periodic mirror should carry the row in within a few intervals.
    let mut mirrored = false;
    for _ in 0..100 {
        let found: Option<String> =
            sqlx::query_scalar("SELECT id FROM hpa_control.members WHERE lower(email) = lower($1)")
                .bind(&user_name)
                .fetch_optional(&pool)
                .await
                .expect("query mirrored member");
        if found.is_some() {
            mirrored = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        mirrored,
        "the SCIM member must reach PostgreSQL without a restart"
    );

    let _ = server
        .client
        .delete(server.url(&format!("/api/enterprise/tenants/{tenant_id}")))
        .bearer_auth(&admin_token)
        .json(&json!({"confirm": true}))
        .send()
        .await;
    pool.close().await;
    let _ = std::fs::remove_dir_all(&data_dir);
}
