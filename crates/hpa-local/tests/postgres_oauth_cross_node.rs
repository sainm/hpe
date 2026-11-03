use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hpa_local::postgres_control::{PostgresControlStore, SessionRecord};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;
use sqlx::sqlite::SqlitePoolOptions;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hpa")
}

fn temp_dir(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("hpa-pg-oauth-{label}-{}", uuid::Uuid::now_v7()));
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

fn query_value(location: &str, key: &str) -> Option<String> {
    let query = location.split_once('?')?.1;
    for pair in query.split('&') {
        let (name, value) = pair.split_once('=')?;
        if name == key {
            return Some(value.to_string());
        }
    }
    None
}

#[tokio::test]
async fn oauth_code_issued_on_one_node_redeems_on_another() {
    let Ok(database_url) = std::env::var("HPA_POSTGRES_TEST_URL") else {
        eprintln!("skipped: HPA_POSTGRES_TEST_URL is not set");
        return;
    };
    let data_dir = temp_dir("a");
    run_cli(&data_dir, &["db", "migrate"], &database_url);
    run_cli(&data_dir, &["enterprise", "init"], &database_url);

    let server_a = Server::start(&data_dir, &database_url).await;

    let session_token = format!("oauth-session-{}", uuid::Uuid::now_v7());
    let csrf = "oauth-csrf".to_string();
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
            tenant_id: "local".to_string(),
            subject: "sub-oauth".to_string(),
            email: Some("oauth@example.test".to_string()),
            display_name: Some("OAuth User".to_string()),
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
    let cookie = format!("hpa_session={session_token}");

    let registered = server_a
        .client
        .post(server_a.url("/oauth/register"))
        .json(&json!({
            "client_name": "cross-node client",
            "redirect_uris": ["https://client.example.test/cb"],
            "token_endpoint_auth_method": "none",
        }))
        .send()
        .await
        .expect("register client");
    assert_eq!(registered.status(), 201, "registration must succeed");
    let registered: Value = registered.json().await.expect("registration json");
    let client_id = registered["client_id"]
        .as_str()
        .expect("client id")
        .to_string();

    let verifier = "a".repeat(64);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let redirect_uri = "https://client.example.test/cb";
    let authorize = server_a
        .client
        .get(server_a.url(&format!(
            "/oauth/authorize?response_type=code&client_id={client_id}&redirect_uri={}&scope=mcp:read&code_challenge={challenge}&code_challenge_method=S256&state=xyz",
            urlencoding(redirect_uri),
        )))
        .header("cookie", &cookie)
        .send()
        .await
        .expect("authorize");
    assert_eq!(authorize.status(), 303, "authorize must redirect");
    let consent_location = authorize
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        consent_location.contains("/consent"),
        "expected a consent redirect, got {consent_location}"
    );
    let request_id = query_value(&consent_location, "request")
        .or_else(|| query_value(&consent_location, "request_id"))
        .expect("request id in consent redirect");

    let decision = server_a
        .client
        .post(server_a.url(&format!("/api/oauth/requests/{request_id}/decision")))
        .header("cookie", &cookie)
        .header("x-hpa-csrf", &csrf)
        .json(&json!({"decision": "allow", "remember": true}))
        .send()
        .await
        .expect("decision");
    assert_eq!(decision.status(), 200, "consent must be accepted");
    let decision: Value = decision.json().await.expect("decision json");
    let location = decision["redirect"]
        .as_str()
        .or_else(|| decision["location"].as_str())
        .expect("redirect in decision response");
    let code = query_value(location, "code").expect("authorization code");

    drop(server_a);
    let fresh_dir = temp_dir("b");
    let server_b = Server::start(&fresh_dir, &database_url).await;
    let token = server_b
        .client
        .post(server_b.url("/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", redirect_uri),
            ("client_id", client_id.as_str()),
            ("code_verifier", verifier.as_str()),
        ])
        .send()
        .await
        .expect("token");
    assert_eq!(
        token.status(),
        200,
        "code redemption must succeed on node B"
    );
    let token: Value = token.json().await.expect("token json");
    let access_token = token["access_token"]
        .as_str()
        .expect("access token")
        .to_string();
    let refresh_token = token["refresh_token"]
        .as_str()
        .expect("refresh token")
        .to_string();

    let header = access_token.split('.').next().expect("jwt header");
    let header: Value = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(header)
            .expect("decode access token header"),
    )
    .expect("access token header json");
    let kid = header["kid"].as_str().expect("kid").to_string();
    let jwks = server_b
        .client
        .get(server_b.url("/oauth/jwks"))
        .send()
        .await
        .expect("jwks");
    assert_eq!(jwks.status(), 200);
    let jwks: Value = jwks.json().await.expect("jwks json");
    let kids = jwks["keys"]
        .as_array()
        .expect("jwks keys")
        .iter()
        .filter_map(|key| key["kid"].as_str())
        .collect::<Vec<_>>();
    assert!(
        kids.contains(&kid.as_str()),
        "node B must publish the signing key created on node A"
    );

    let repeated = server_b
        .client
        .get(server_b.url(&format!(
            "/oauth/authorize?response_type=code&client_id={client_id}&redirect_uri={}&scope=mcp:read&code_challenge={challenge}&code_challenge_method=S256&state=repeat",
            urlencoding(redirect_uri),
        )))
        .header("cookie", &cookie)
        .send()
        .await
        .expect("authorize with consent");
    assert_eq!(repeated.status(), 303);
    let repeated_location = repeated
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        repeated_location.starts_with(redirect_uri) && repeated_location.contains("code="),
        "remembered consent must skip the consent page on node B, got {repeated_location}"
    );

    let introspect = server_b
        .client
        .post(server_b.url("/oauth/introspect"))
        .form(&[("token", access_token.as_str())])
        .send()
        .await
        .expect("introspect");
    assert_eq!(introspect.status(), 200);
    let introspect: Value = introspect.json().await.expect("introspection json");
    assert_eq!(introspect["active"], json!(true), "token must be active");
    assert_eq!(introspect["client_id"], json!(client_id));

    let refreshed = server_b
        .client
        .post(server_b.url("/oauth/token"))
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.as_str()),
            ("client_id", client_id.as_str()),
        ])
        .send()
        .await
        .expect("refresh");
    assert_eq!(refreshed.status(), 200, "refresh must succeed on node B");
    let refreshed: Value = refreshed.json().await.expect("refresh json");
    let access_token_2 = refreshed["access_token"]
        .as_str()
        .expect("refreshed access token")
        .to_string();
    let refresh_token_2 = refreshed["refresh_token"]
        .as_str()
        .expect("rotated refresh token")
        .to_string();
    assert_ne!(access_token_2, access_token);

    let revoked = server_b
        .client
        .post(server_b.url("/oauth/revoke"))
        .form(&[
            ("token", access_token_2.as_str()),
            ("client_id", client_id.as_str()),
        ])
        .send()
        .await
        .expect("revoke");
    assert_eq!(revoked.status(), 200);
    let inactive = server_b
        .client
        .post(server_b.url("/oauth/introspect"))
        .form(&[("token", access_token_2.as_str())])
        .send()
        .await
        .expect("introspect revoked");
    let inactive: Value = inactive.json().await.expect("introspection json");
    assert_eq!(inactive["active"], json!(false));

    let replay = server_b
        .client
        .post(server_b.url("/oauth/token"))
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.as_str()),
            ("client_id", client_id.as_str()),
        ])
        .send()
        .await
        .expect("refresh replay");
    assert_eq!(replay.status(), 400, "refresh replay must be rejected");
    let family_gone = server_b
        .client
        .post(server_b.url("/oauth/token"))
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token_2.as_str()),
            ("client_id", client_id.as_str()),
        ])
        .send()
        .await
        .expect("refresh after reuse");
    assert_eq!(
        family_gone.status(),
        400,
        "reuse detection must revoke the whole family"
    );

    for dir in [&data_dir, &fresh_dir] {
        for table in [
            "oauth_tokens",
            "oauth_clients",
            "oauth_requests",
            "oauth_codes",
            "oauth_consents",
            "oauth_signing_keys",
        ] {
            assert_eq!(
                sqlite_count(&dir.join("control.db"), table).await,
                0,
                "{table} must live in PostgreSQL"
            );
        }
    }

    pool.close().await;
    let _ = std::fs::remove_dir_all(&data_dir);
    let _ = std::fs::remove_dir_all(&fresh_dir);
}

fn urlencoding(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            ':' => "%3A".to_string(),
            '/' => "%2F".to_string(),
            other => other.to_string(),
        })
        .collect()
}
