//! Browser sessions move to PostgreSQL in runtime mode, so a cookie issued by
//! one node is accepted by every node and revocation is shared.
//!
//! Skipped when `HPA_POSTGRES_TEST_URL` is absent.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use hpa_local::postgres_control::{PostgresControlStore, SessionRecord};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::Row;
use sqlx::sqlite::SqlitePoolOptions;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hpa")
}

fn temp_dir(label: &str) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("hpa-pg-session-{label}-{}", uuid::Uuid::now_v7()));
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

fn sha256_hex(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
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

    async fn session_with_cookie(&self, token: &str) -> Value {
        self.client
            .get(self.url("/api/auth/session"))
            .header("cookie", format!("hpa_session={token}"))
            .send()
            .await
            .expect("auth session")
            .json()
            .await
            .expect("auth session json")
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
async fn browser_session_is_shared_across_nodes() {
    let Ok(database_url) = std::env::var("HPA_POSTGRES_TEST_URL") else {
        eprintln!("skipped: HPA_POSTGRES_TEST_URL is not set");
        return;
    };
    let data_dir = temp_dir("a");
    run_cli(&data_dir, &["db", "migrate"], &database_url);
    run_cli(&data_dir, &["enterprise", "init"], &database_url);

    let server_a = Server::start(&data_dir, &database_url).await;

    // Insert a browser session the same way the SSO callback would.
    let token = format!("session-token-{}", uuid::Uuid::now_v7());
    let token_hash = sha256_hex(&token);
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect postgres");
    let control = PostgresControlStore::new(pool.clone());
    let now = time::OffsetDateTime::now_utc().unix_timestamp_nanos() as i64 / 1_000_000;
    control
        .create_session(&SessionRecord {
            id: format!("sess_{}", uuid::Uuid::now_v7()),
            token_hash: token_hash.clone(),
            csrf_token: "csrf-token".to_string(),
            tenant_id: "local".to_string(),
            subject: "sub-shared".to_string(),
            email: Some("shared@example.test".to_string()),
            display_name: Some("Shared User".to_string()),
            role: "owner".to_string(),
            platform_admin: true,
            created_at: now,
            expires_at: now + 3_600_000,
            revoked_at: None,
            last_seen_at: Some(now),
            refresh_token_ciphertext: None,
            refresh_token_version: 0,
            refresh_token_expires_at: None,
            id_token_ciphertext: None,
            access_token_expires_at: None,
            last_refresh_at: None,
        })
        .await
        .expect("create session");

    let on_a = server_a.session_with_cookie(&token).await;
    assert_eq!(on_a["authenticated"], serde_json::json!(true));
    assert_eq!(
        on_a["principal"]["subject"],
        serde_json::json!("sub-shared")
    );

    // The node that created nothing must accept the same cookie.
    drop(server_a);
    let fresh_dir = temp_dir("b");
    let server_b = Server::start(&fresh_dir, &database_url).await;
    let on_b = server_b.session_with_cookie(&token).await;
    assert_eq!(
        on_b["authenticated"],
        serde_json::json!(true),
        "the shared session must authenticate on another node: {on_b}"
    );
    assert_eq!(on_b["principal"]["tenantId"], serde_json::json!("local"));
    assert_eq!(on_b["principal"]["platformAdmin"], serde_json::json!(true));
    assert_eq!(
        sqlite_count(&fresh_dir.join("control.db"), "enterprise_sessions").await,
        0,
        "sessions must not be written to the local SQLite store"
    );

    // Logout on the second node revokes the row for every node.
    let logout = server_b
        .client
        .post(server_b.url("/api/auth/logout"))
        .header("cookie", format!("hpa_session={token}"))
        .header("x-hpa-csrf", "csrf-token")
        .send()
        .await
        .expect("logout");
    assert_eq!(logout.status(), 200, "logout must succeed");
    let after = server_b.session_with_cookie(&token).await;
    assert_eq!(
        after["authenticated"],
        serde_json::json!(false),
        "the revoked session must be rejected"
    );
    let revoked_at: Option<i64> =
        sqlx::query_scalar("SELECT revoked_at FROM hpa_control.sessions WHERE token_hash = $1")
            .bind(&token_hash)
            .fetch_one(&pool)
            .await
            .expect("revoked_at");
    assert!(
        revoked_at.is_some(),
        "revocation must be persisted in PostgreSQL"
    );

    pool.close().await;
    let _ = std::fs::remove_dir_all(&data_dir);
    let _ = std::fs::remove_dir_all(&fresh_dir);
}
