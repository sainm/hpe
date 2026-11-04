use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{Signer, SigningKey};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::RwLock;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hpa")
}

fn temp_dir(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("hpa-pg-remote-{label}-{}", uuid::Uuid::now_v7()));
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

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("timestamp")
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
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&format!("sqlite://{}", path.display()))
        .await
        .expect("open sqlite");
    let count: i64 =
        sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
            .fetch_one(&pool)
            .await
            .expect("count rows");
    pool.close().await;
    count
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IndexPackage<'a> {
    plugin_id: &'a str,
    version: &'a str,
    path: &'a str,
    sha256: &'a str,
    publisher_id: &'a str,
    signer_key_id: &'a str,
    review_status: &'a str,
    published_at: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IndexWithdrawal<'a> {
    plugin_id: &'a str,
    version: &'a str,
    reason: &'a str,
    at: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IndexPayload<'a> {
    schema_version: &'a str,
    source_id: &'a str,
    sequence: i64,
    generated_at: &'a str,
    packages: &'a [IndexPackage<'a>],
    withdrawals: &'a [IndexWithdrawal<'a>],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IndexSignature {
    algorithm: &'static str,
    key_id: &'static str,
    public_key: String,
    value: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IndexDocument<'a> {
    schema_version: &'a str,
    source_id: &'a str,
    sequence: i64,
    generated_at: &'a str,
    packages: &'a [IndexPackage<'a>],
    withdrawals: &'a [IndexWithdrawal<'a>],
    signature: IndexSignature,
}

fn signed_index(
    signing: &SigningKey,
    source_id: &str,
    sequence: i64,
    packages: &[IndexPackage<'_>],
    withdrawals: &[IndexWithdrawal<'_>],
) -> Vec<u8> {
    let generated_at = now_rfc3339();
    let payload = IndexPayload {
        schema_version: "hpa.marketplace.index/v1",
        source_id,
        sequence,
        generated_at: &generated_at,
        packages,
        withdrawals,
    };
    let payload_bytes = serde_json::to_vec(&payload).expect("payload bytes");
    let signature = signing.sign(&payload_bytes);
    let document = IndexDocument {
        schema_version: "hpa.marketplace.index/v1",
        source_id,
        sequence,
        generated_at: &generated_at,
        packages,
        withdrawals,
        signature: IndexSignature {
            algorithm: "ed25519",
            key_id: "remote-source-key",
            public_key: BASE64.encode(signing.verifying_key().to_bytes()),
            value: BASE64.encode(signature.to_bytes()),
        },
    };
    serde_json::to_vec(&document).expect("index bytes")
}

struct HttpFixture {
    index_url: String,
    index: Arc<RwLock<Vec<u8>>>,
    task: tokio::task::JoinHandle<()>,
}

impl HttpFixture {
    async fn start(package: Vec<u8>, index: Vec<u8>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fixture");
        let port = listener.local_addr().expect("fixture addr").port();
        let index = Arc::new(RwLock::new(index));
        let task_index = index.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let index = task_index.clone();
                let package = package.clone();
                tokio::spawn(async move {
                    let mut buffer = [0u8; 2048];
                    let Ok(read) = socket.read(&mut buffer).await else {
                        return;
                    };
                    let request = String::from_utf8_lossy(&buffer[..read]);
                    let path = request
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("/");
                    let (status, body) = if path == "/index.json" {
                        ("200 OK", index.read().await.clone())
                    } else if path == "/plugin.hplugin" {
                        ("200 OK", package)
                    } else {
                        ("404 Not Found", Vec::new())
                    };
                    let header = format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(header.as_bytes()).await;
                    let _ = socket.write_all(&body).await;
                });
            }
        });
        Self {
            index_url: format!("http://127.0.0.1:{port}/index.json"),
            index,
            task,
        }
    }

    async fn replace_index(&self, index: Vec<u8>) {
        *self.index.write().await = index;
    }
}

impl Drop for HttpFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[tokio::test]
async fn remote_marketplace_installs_and_withdraws_across_nodes() {
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

    let sample = server_a
        .client
        .post(server_a.url("/api/plugins/sample"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("install sample plugin");
    assert_eq!(sample.status(), 200);
    let key = server_a
        .client
        .get(server_a.url("/api/plugins/key"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("plugin key");
    let key: Value = key.json().await.expect("key json");
    let publisher_key_id = key["keyId"].as_str().expect("key id").to_string();
    let publisher_public_key = key["publicKey"].as_str().expect("public key").to_string();
    let package_path = data_dir_a
        .join("plugins")
        .join("samples")
        .join("plugin_sample_kit-1.0.0.hplugin");
    let package = std::fs::read(&package_path).expect("sample package");
    let package_sha256 = format!("{:x}", Sha256::digest(&package));

    let source_signing = SigningKey::from_bytes(&[7u8; 32]);
    let published_at = now_rfc3339();
    let packages = vec![IndexPackage {
        plugin_id: "plugin_sample_kit",
        version: "1.0.0",
        path: "plugin.hplugin",
        sha256: &package_sha256,
        publisher_id: "local",
        signer_key_id: &publisher_key_id,
        review_status: "approved",
        published_at: Some(&published_at),
    }];
    let index = signed_index(&source_signing, "remote_cross", 1, &packages, &[]);
    let fixture = HttpFixture::start(package, index).await;

    let publisher = server_a
        .client
        .post(server_a.url("/api/marketplace/publishers"))
        .bearer_auth(&admin)
        .json(&json!({"id": "local", "name": "Local Workspace"}))
        .send()
        .await
        .expect("add publisher");
    assert_eq!(publisher.status(), 200);
    let publisher_key = server_a
        .client
        .post(server_a.url("/api/marketplace/publishers/local/keys"))
        .bearer_auth(&admin)
        .json(&json!({
            "keyId": publisher_key_id,
            "publicKey": publisher_public_key,
            "label": "Local publisher key",
        }))
        .send()
        .await
        .expect("add publisher key");
    assert_eq!(publisher_key.status(), 200);
    let source = server_a
        .client
        .post(server_a.url("/api/marketplace/sources"))
        .bearer_auth(&admin)
        .json(&json!({
            "id": "remote_cross",
            "label": "Remote Cross",
            "indexUri": fixture.index_url,
            "keyId": "remote-source-key",
            "publicKey": BASE64.encode(source_signing.verifying_key().to_bytes()),
            "keyLabel": "Remote source key",
        }))
        .send()
        .await
        .expect("add source");
    assert_eq!(source.status(), 200);

    let refreshed = server_b
        .client
        .post(server_b.url("/api/marketplace/sources/remote_cross/refresh"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("refresh on b");
    let status = refreshed.status();
    let body = refreshed.text().await.expect("refresh body");
    assert_eq!(status, 200, "refresh failed: {body}");
    let remote = server_a
        .client
        .get(server_a.url("/api/marketplace/remote?source=remote_cross"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("remote packages on a");
    let remote: Value = remote.json().await.expect("remote json");
    assert!(
        remote["packages"]
            .as_array()
            .expect("remote packages")
            .iter()
            .any(|package| package["pluginId"] == json!("plugin_sample_kit")
                && package["reviewStatus"] == json!("approved"))
    );

    let uninstalled = server_a
        .client
        .post(server_a.url("/api/plugins/plugin_sample_kit/uninstall"))
        .bearer_auth(&admin)
        .json(&json!({"force": true}))
        .send()
        .await
        .expect("uninstall sample on a");
    assert_eq!(uninstalled.status(), 200);
    let installed =
        server_a
            .client
            .post(server_a.url(
                "/api/marketplace/remote/remote_cross/plugin_sample_kit/versions/1.0.0/install",
            ))
            .bearer_auth(&admin)
            .send()
            .await
            .expect("remote install on a");
    let status = installed.status();
    let body = installed.text().await.expect("remote install body");
    assert_eq!(status, 200, "remote install failed: {body}");
    let detail = server_b
        .client
        .get(server_b.url("/api/plugins/plugin_sample_kit"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("detail on b");
    let detail: Value = detail.json().await.expect("detail json");
    assert_eq!(detail["plugin"]["status"], json!("enabled"));
    assert_eq!(detail["plugin"]["installedVersion"], json!("1.0.0"));

    let withdrawal_at = now_rfc3339();
    let withdrawal = vec![IndexWithdrawal {
        plugin_id: "plugin_sample_kit",
        version: "1.0.0",
        reason: "security",
        at: &withdrawal_at,
    }];
    let withdrawn_index = signed_index(&source_signing, "remote_cross", 2, &[], &withdrawal);
    fixture.replace_index(withdrawn_index).await;
    let refreshed = server_b
        .client
        .post(server_b.url("/api/marketplace/sources/remote_cross/refresh"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("withdraw refresh on b");
    assert_eq!(refreshed.status(), 200);
    let remote = server_a
        .client
        .get(server_a.url("/api/marketplace/remote?source=remote_cross"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("remote packages after withdraw");
    let remote: Value = remote.json().await.expect("remote json");
    assert!(
        remote["packages"]
            .as_array()
            .expect("remote packages")
            .iter()
            .any(|package| package["pluginId"] == json!("plugin_sample_kit")
                && package["withdrawnAt"].is_string()
                && package["revocationReason"] == json!("security"))
    );
    let detail = server_a
        .client
        .get(server_a.url("/api/plugins/plugin_sample_kit"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("detail after withdraw");
    let detail: Value = detail.json().await.expect("detail json");
    assert_eq!(detail["plugin"]["status"], json!("disabled"));
    let blocked =
        server_a
            .client
            .post(server_a.url(
                "/api/marketplace/remote/remote_cross/plugin_sample_kit/versions/1.0.0/install",
            ))
            .bearer_auth(&admin)
            .send()
            .await
            .expect("install withdrawn");
    assert_eq!(blocked.status(), 403);

    for dir in [&data_dir_a, &data_dir_b] {
        for table in [
            "marketplace_sources",
            "marketplace_source_keys",
            "marketplace_remote_packages",
            "plugin_publishers",
            "plugin_publisher_keys",
            "plugin_packages",
            "plugins",
        ] {
            assert_eq!(
                sqlite_count(&dir.join("hpa.db"), table).await,
                0,
                "{table} must live in PostgreSQL"
            );
        }
    }

    drop(fixture);
    let _ = std::fs::remove_dir_all(&data_dir_a);
    let _ = std::fs::remove_dir_all(&data_dir_b);
}
