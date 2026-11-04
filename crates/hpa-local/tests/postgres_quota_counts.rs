use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hpa")
}

fn temp_dir(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("hpa-pg-quota-{label}-{}", uuid::Uuid::now_v7()));
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

fn unsigned_plugin_dir(root: &Path) -> PathBuf {
    let dir = root.join("quota-probe");
    std::fs::create_dir_all(&dir).expect("probe dir");
    let empty = format!("sha256:{:x}", Sha256::digest(Vec::<u8>::new()));
    let manifest = json!({
        "schemaVersion": "hpa.plugin/v1",
        "id": "plugin_quota_probe",
        "name": "Quota Probe",
        "version": "1.0.0",
        "license": "Apache-2.0",
        "compatibility": {"hpa": ">=0.1.0, <1.0.0", "os": []},
        "capabilities": [],
        "dependencies": [],
        "permissions": [],
        "integrity": {"algorithm": "sha256", "digest": empty}
    });
    std::fs::write(
        dir.join("hpa.plugin.json"),
        serde_json::to_vec_pretty(&manifest).expect("manifest"),
    )
    .expect("write manifest");
    dir
}

async fn set_quota(
    client: &reqwest::Client,
    base: &str,
    admin: &str,
    plugins: i64,
    connections: i64,
) {
    let response = client
        .put(format!("{base}/api/enterprise/quotas/local"))
        .bearer_auth(admin)
        .json(&json!({
            "maxWorkflows": 100,
            "maxExecutionsPerDay": 1000,
            "maxSkills": 100,
            "maxAgents": 100,
            "maxPlugins": plugins,
            "maxConnections": connections,
            "maxConcurrentRuns": 8
        }))
        .send()
        .await
        .expect("set quota");
    assert_eq!(response.status(), 200);
}

#[tokio::test]
async fn plugin_and_connection_quotas_count_postgres_across_nodes() {
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
    set_quota(&server_a.client, &server_a.url(""), &admin, 0, 0).await;
    for path in [
        "/api/plugins/sample",
        "/api/marketplace/plugin_sample_kit/versions/1.0.0/install",
        "/api/marketplace/remote/source_cross/plugin_sample_kit/versions/1.0.0/install",
    ] {
        let response = server_b
            .client
            .post(server_b.url(path))
            .bearer_auth(&admin)
            .send()
            .await
            .expect("plugin quota route");
        assert_eq!(response.status(), 429, "{path} must be quota-gated");
    }
    let sample_connection = server_b
        .client
        .post(server_b.url("/api/connectors/sample"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("sample connection at zero quota");
    assert_eq!(sample_connection.status(), 429);
    set_quota(&server_a.client, &server_a.url(""), &admin, 1, 1).await;

    let sample = server_a
        .client
        .post(server_a.url("/api/plugins/sample"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("sample plugin");
    assert_eq!(sample.status(), 200);
    let sample_connection = server_a
        .client
        .post(server_a.url("/api/connectors/sample"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("sample connection");
    assert_eq!(sample_connection.status(), 200);

    let probe_dir = unsigned_plugin_dir(&data_dir_a);
    let probe = server_b
        .client
        .post(server_b.url("/api/plugins/install"))
        .bearer_auth(&admin)
        .json(&json!({
            "path": probe_dir.to_string_lossy(),
            "allowUnsigned": true
        }))
        .send()
        .await
        .expect("probe plugin install");
    assert_eq!(probe.status(), 429);
    let probe_body: Value = probe.json().await.expect("probe body");
    assert_eq!(probe_body["error"]["code"], json!("TENANT_QUOTA"));

    let workspace = data_dir_b.join("quota-workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let connection = server_b
        .client
        .post(server_b.url("/api/connections"))
        .bearer_auth(&admin)
        .json(&json!({
            "id": "conn_quota_probe",
            "name": "Quota Probe",
            "connectorId": "file",
            "connectorVersion": "1.0.0",
            "config": {"root": workspace.to_string_lossy(), "allowWrite": true}
        }))
        .send()
        .await
        .expect("probe connection");
    assert_eq!(connection.status(), 429);
    let connection_body: Value = connection.json().await.expect("connection body");
    assert_eq!(connection_body["error"]["code"], json!("TENANT_QUOTA"));

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect postgres");
    let plugins: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM hpa_tenant.plugins
         WHERE tenant_id = 'local' AND status != 'uninstalled'",
    )
    .fetch_one(&pool)
    .await
    .expect("plugin count");
    let connections: i64 =
        sqlx::query_scalar("SELECT count(*) FROM hpa_tenant.connections WHERE tenant_id = 'local'")
            .fetch_one(&pool)
            .await
            .expect("connection count");
    assert_eq!(plugins, 1);
    assert_eq!(connections, 1);

    set_quota(&server_a.client, &server_a.url(""), &admin, 2, 2).await;
    let probe = server_b
        .client
        .post(server_b.url("/api/plugins/install"))
        .bearer_auth(&admin)
        .json(&json!({
            "path": probe_dir.to_string_lossy(),
            "allowUnsigned": true
        }))
        .send()
        .await
        .expect("probe plugin install after raise");
    assert_eq!(probe.status(), 200);
    let connection = server_b
        .client
        .post(server_b.url("/api/connections"))
        .bearer_auth(&admin)
        .json(&json!({
            "id": "conn_quota_probe",
            "name": "Quota Probe",
            "connectorId": "file",
            "connectorVersion": "1.0.0",
            "config": {"root": workspace.to_string_lossy(), "allowWrite": true}
        }))
        .send()
        .await
        .expect("probe connection after raise");
    assert_eq!(connection.status(), 200);
    let plugins: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM hpa_tenant.plugins
         WHERE tenant_id = 'local' AND status != 'uninstalled'",
    )
    .fetch_one(&pool)
    .await
    .expect("plugin count after raise");
    let connections: i64 =
        sqlx::query_scalar("SELECT count(*) FROM hpa_tenant.connections WHERE tenant_id = 'local'")
            .fetch_one(&pool)
            .await
            .expect("connection count after raise");
    assert_eq!(plugins, 2);
    assert_eq!(connections, 2);

    for dir in [&data_dir_a, &data_dir_b] {
        for table in ["plugins", "plugin_packages", "connections"] {
            assert_eq!(
                sqlite_count(&dir.join("hpa.db"), table).await,
                0,
                "{table} must live in PostgreSQL"
            );
        }
    }

    pool.close().await;
    let _ = std::fs::remove_dir_all(&data_dir_a);
    let _ = std::fs::remove_dir_all(&data_dir_b);
}
