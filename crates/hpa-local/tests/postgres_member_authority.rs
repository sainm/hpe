use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use hpa_local::postgres_control::{PostgresControlStore, SessionRecord};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hpa")
}

fn temp_dir(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("hpa-pg-member-{label}-{}", uuid::Uuid::now_v7()));
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
            // This suite verifies PostgreSQL authority, not cache refresh timing.
            // A short interval legitimately repopulates SQLite and makes the
            // zero-row assertions race with the periodic synchronizer.
            .env("HPA_CONTROL_SYNC_INTERVAL_MS", "60000")
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

async fn sqlite_count(path: &Path, table: &str, column: &str, value: &str) -> i64 {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&format!("sqlite://{}", path.display()))
        .await
        .expect("open sqlite");
    let query = format!("SELECT count(*) FROM {table} WHERE {column} = ?");
    let count: i64 = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(query))
        .bind(value)
        .fetch_one(&pool)
        .await
        .expect("count rows");
    pool.close().await;
    count
}

#[tokio::test]
async fn member_and_tenant_authority_crosses_nodes() {
    let Ok(database_url) = std::env::var("HPA_POSTGRES_TEST_URL") else {
        eprintln!("skipped: HPA_POSTGRES_TEST_URL is not set");
        return;
    };
    let data_dir_a = temp_dir("a");
    run_cli(&data_dir_a, &["db", "migrate"], &database_url);
    let bootstrap: Value = serde_json::from_str(&run_cli(
        &data_dir_a,
        &["enterprise", "init"],
        &database_url,
    ))
    .expect("init json");
    let admin = bootstrap["token"]
        .as_str()
        .expect("admin token")
        .to_string();
    let server_a = Server::start(&data_dir_a, &database_url).await;
    let data_dir_b = temp_dir("b");
    let server_b = Server::start(&data_dir_b, &database_url).await;

    let tenant = server_a
        .client
        .post(server_a.url("/api/enterprise/tenants"))
        .bearer_auth(&admin)
        .json(&json!({"name": "cross-node members"}))
        .send()
        .await
        .expect("create tenant");
    assert_eq!(tenant.status(), 200);
    let tenant: Value = tenant.json().await.expect("tenant json");
    let tenant_id = tenant["id"].as_str().expect("tenant id").to_string();

    let listed = server_b
        .client
        .get(server_b.url("/api/enterprise/tenants"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("list tenants");
    let listed: Value = listed.json().await.expect("tenants json");
    assert!(
        listed["tenants"]
            .as_array()
            .expect("tenants")
            .iter()
            .any(|item| item["id"] == json!(tenant_id)),
        "node B must see the tenant created on node A"
    );

    let member = server_a
        .client
        .post(server_a.url("/api/enterprise/members"))
        .bearer_auth(&admin)
        .json(&json!({
            "tenantId": tenant_id,
            "subject": "sub-member",
            "displayName": "Cross Node",
            "email": "cross-node@example.test",
            "role": "viewer"
        }))
        .send()
        .await
        .expect("create member");
    assert_eq!(member.status(), 200);
    let member: Value = member.json().await.expect("member json");
    let member_id = member["id"].as_str().expect("member id").to_string();

    let listed = server_b
        .client
        .get(server_b.url(&format!("/api/enterprise/members?tenantId={tenant_id}")))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("list members");
    let listed: Value = listed.json().await.expect("members json");
    assert!(
        listed["members"]
            .as_array()
            .expect("members")
            .iter()
            .any(|item| item["id"] == json!(member_id) && item["role"] == json!("viewer")),
        "node B must see the member created on node A"
    );

    let updated = server_b
        .client
        .put(server_b.url(&format!("/api/enterprise/members/{member_id}")))
        .bearer_auth(&admin)
        .json(&json!({"role": "developer"}))
        .send()
        .await
        .expect("update member");
    assert_eq!(updated.status(), 200);
    let updated: Value = updated.json().await.expect("member json");
    assert_eq!(updated["role"], json!("developer"));

    let session_token = format!("member-session-{}", uuid::Uuid::now_v7());
    let csrf = "member-csrf".to_string();
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
            token_hash: sha256_hex(&session_token),
            csrf_token: csrf.clone(),
            tenant_id: tenant_id.clone(),
            subject: "sub-member".to_string(),
            email: Some("cross-node@example.test".to_string()),
            display_name: Some("Cross Node".to_string()),
            role: "owner".to_string(),
            platform_admin: false,
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
    let cookie = format!("hpa_session={session_token}");

    let session = server_b
        .client
        .get(server_b.url("/api/auth/session"))
        .header("cookie", &cookie)
        .send()
        .await
        .expect("session");
    let session: Value = session.json().await.expect("session json");
    assert_eq!(session["authenticated"], json!(true));
    assert_eq!(
        session["principal"]["role"],
        json!("developer"),
        "the member directory role must override the stored session role on node B"
    );

    let disabled = server_a
        .client
        .put(server_a.url(&format!("/api/enterprise/members/{member_id}")))
        .bearer_auth(&admin)
        .json(&json!({"status": "disabled"}))
        .send()
        .await
        .expect("disable member");
    assert_eq!(disabled.status(), 200);
    let session = server_b
        .client
        .get(server_b.url("/api/auth/session"))
        .header("cookie", &cookie)
        .send()
        .await
        .expect("session after disable");
    let session: Value = session.json().await.expect("session json");
    assert_eq!(session["authenticated"], json!(false));

    let enabled = server_a
        .client
        .put(server_a.url(&format!("/api/enterprise/members/{member_id}")))
        .bearer_auth(&admin)
        .json(&json!({"status": "active"}))
        .send()
        .await
        .expect("enable member");
    assert_eq!(enabled.status(), 200);
    let session = server_b
        .client
        .get(server_b.url("/api/auth/session"))
        .header("cookie", &cookie)
        .send()
        .await
        .expect("session after enable");
    let session: Value = session.json().await.expect("session json");
    assert_eq!(session["authenticated"], json!(true));

    let suspended = server_a
        .client
        .put(server_a.url(&format!("/api/enterprise/tenants/{tenant_id}")))
        .bearer_auth(&admin)
        .json(&json!({"status": "suspended"}))
        .send()
        .await
        .expect("suspend tenant");
    assert_eq!(suspended.status(), 200);
    let session = server_b
        .client
        .get(server_b.url("/api/auth/session"))
        .header("cookie", &cookie)
        .send()
        .await
        .expect("session after suspend");
    let session: Value = session.json().await.expect("session json");
    assert_eq!(session["authenticated"], json!(false));

    assert_eq!(
        sqlite_count(
            &data_dir_a.join("control.db"),
            "enterprise_members",
            "tenant_id",
            &tenant_id,
        )
        .await,
        0,
        "members must not be written to the node A SQLite control plane"
    );
    assert_eq!(
        sqlite_count(
            &data_dir_a.join("control.db"),
            "enterprise_tenants",
            "id",
            &tenant_id,
        )
        .await,
        0,
        "tenants must not be written to the node A SQLite control plane"
    );
    assert_eq!(
        sqlite_count(
            &data_dir_b.join("control.db"),
            "enterprise_members",
            "tenant_id",
            &tenant_id,
        )
        .await,
        0,
        "node B must not cache members in SQLite before the cache refresh"
    );

    pool.close().await;
    let _ = std::fs::remove_dir_all(&data_dir_a);
    let _ = std::fs::remove_dir_all(&data_dir_b);
}
