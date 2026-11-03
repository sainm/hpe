//! `hpa db verify-mirror` must report real drift between the authoritative
//! PostgreSQL control plane and the rebuildable SQLite cache, and stay quiet
//! when both agree.
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
    let path = std::env::temp_dir().join(format!("hpa-pg-verify-{label}-{}", uuid::Uuid::now_v7()));
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

fn run_cli(data_dir: &Path, args: &[&str], database_url: &str) -> (bool, String) {
    let output = Command::new(binary())
        .arg("--data-dir")
        .arg(data_dir)
        .args(args)
        .env("HPA_POSTGRES_URL", database_url)
        .output()
        .expect("run cli");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).to_string(),
    )
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
async fn verify_mirror_detects_and_clears_drift() {
    let Ok(database_url) = std::env::var("HPA_POSTGRES_TEST_URL") else {
        eprintln!("skipped: HPA_POSTGRES_TEST_URL is not set");
        return;
    };
    let data_dir = temp_dir("node");
    run_cli(&data_dir, &["db", "migrate"], &database_url);
    let (ok, stdout) = run_cli(&data_dir, &["enterprise", "init"], &database_url);
    assert!(ok, "enterprise init failed: {stdout}");
    let admin_token = serde_json::from_str::<Value>(&stdout).expect("init json")["token"]
        .as_str()
        .expect("admin token")
        .to_string();

    let server = Server::start(&data_dir, &database_url).await;
    let tenant = server
        .client
        .post(server.url("/api/enterprise/tenants"))
        .bearer_auth(&admin_token)
        .json(&json!({"name": format!("verify_{}", uuid::Uuid::now_v7().simple())}))
        .send()
        .await
        .expect("create tenant");
    let tenant: Value = tenant.json().await.expect("tenant json");
    let tenant_id = tenant["id"].as_str().expect("tenant id").to_string();
    let member = server
        .client
        .post(server.url("/api/enterprise/members"))
        .bearer_auth(&admin_token)
        .json(&json!({
            "tenantId": tenant_id,
            "subject": "sub-verify",
            "displayName": "Verify User",
            "role": "developer",
        }))
        .send()
        .await
        .expect("create member");
    assert_eq!(member.status(), 200);
    let member: Value = member.json().await.expect("member json");
    let member_id = member["id"].as_str().expect("member id").to_string();

    // PostgreSQL is authoritative; the SQLite cache converges on the next refresh.
    let mut in_sync = None;
    for _ in 0..100 {
        let (ok, stdout) = run_cli(&data_dir, &["db", "verify-mirror"], &database_url);
        let report: Value = serde_json::from_str(&stdout).expect("verify json");
        if ok && report["drift"]["total"] == json!(0) {
            in_sync = Some(report);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let report = in_sync.unwrap_or_else(|| panic!("SQLite cache did not converge"));
    assert_eq!(report["counts"]["tenants"]["sqlite"], json!(2));

    // Stop the server first: the cache rebuilds from PostgreSQL every 100ms, so a
    // mutation made while it runs can be overwritten before verify-mirror reads it.
    drop(server);
    // A SQLite-cache change must be reported as drift.
    let control_db = data_dir.join("control.db");
    let sqlite = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&format!("sqlite://{}", control_db.display()))
        .await
        .expect("open sqlite");
    sqlx::query("UPDATE enterprise_members SET role = 'auditor' WHERE id = ?")
        .bind(&member_id)
        .execute(&sqlite)
        .await
        .expect("mutate sqlite member");
    sqlite.close().await;

    let (ok, stdout) = run_cli(&data_dir, &["db", "verify-mirror"], &database_url);
    let report: Value = serde_json::from_str(&stdout).expect("verify json");
    assert!(!ok, "drift must fail the command: {report}");
    assert!(
        report["drift"]["total"].as_i64().unwrap_or_default() >= 1,
        "drift must be counted: {report}"
    );
    let kinds = report["drift"]["items"]
        .as_array()
        .expect("drift items")
        .iter()
        .filter_map(|item| item["kind"].as_str())
        .collect::<Vec<_>>();
    assert!(
        kinds.contains(&"member.attrMismatch"),
        "expected a member attribute mismatch, got {kinds:?}"
    );

    // Restarting rebuilds the cache from PostgreSQL; drift returns to zero.
    let server = Server::start(&data_dir, &database_url).await;
    let mut repaired = false;
    for _ in 0..100 {
        let (ok, stdout) = run_cli(&data_dir, &["db", "verify-mirror"], &database_url);
        let report: Value = serde_json::from_str(&stdout).expect("verify json");
        if ok && report["drift"]["total"] == json!(0) {
            repaired = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(repaired, "the periodic mirror should repair the drift");

    let _ = server
        .client
        .delete(server.url(&format!("/api/enterprise/tenants/{tenant_id}")))
        .bearer_auth(&admin_token)
        .json(&json!({"confirm": true}))
        .send()
        .await;
    let _ = std::fs::remove_dir_all(&data_dir);
}
