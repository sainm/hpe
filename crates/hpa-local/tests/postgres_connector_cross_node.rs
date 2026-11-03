use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use hpa_local::postgres_connector::PostgresConnectorStore;
use serde_json::{Value, json};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hpa")
}

fn temp_dir(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("hpa-pg-conn-{label}-{}", uuid::Uuid::now_v7()));
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

#[tokio::test]
async fn connector_metadata_is_authoritative_in_postgres_across_nodes() {
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

    let connectors = server_a
        .client
        .get(server_a.url("/api/connectors"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("list connectors");
    let connectors: Value = connectors.json().await.expect("connectors json");
    let connectors = connectors["connectors"].as_array().expect("connectors");
    assert!(
        connectors
            .iter()
            .any(|item| item["connectorId"] == json!("file")
                && item["version"] == json!("1.0.0")
                && item["status"] == json!("enabled"))
    );
    let connectors_b = server_b
        .client
        .get(server_b.url("/api/connectors"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("list connectors on b");
    let connectors_b: Value = connectors_b.json().await.expect("connectors b json");
    assert!(
        connectors_b["connectors"]
            .as_array()
            .expect("connectors b")
            .iter()
            .any(|item| item["connectorId"] == json!("file"))
    );

    let sample = server_a
        .client
        .post(server_a.url("/api/connectors/sample"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("create sample connection");
    assert_eq!(sample.status(), 200);
    let sample: Value = sample.json().await.expect("sample json");
    let connection_id = sample["id"].as_str().expect("connection id").to_string();
    let config = sample["config"].clone();

    let listed = server_b
        .client
        .get(server_b.url("/api/connections"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("list connections on b");
    let listed: Value = listed.json().await.expect("connections b json");
    assert!(
        listed["connections"]
            .as_array()
            .expect("connections")
            .iter()
            .any(|item| item["id"] == json!(connection_id) && item["config"] == config),
        "node B must read the connection created on node A from PostgreSQL"
    );

    let updated = server_b
        .client
        .post(server_b.url("/api/connections"))
        .bearer_auth(&admin)
        .json(&json!({
            "id": connection_id,
            "name": "Cross-node sample",
            "connectorId": "file",
            "connectorVersion": "1.0.0",
            "config": config,
        }))
        .send()
        .await
        .expect("update connection on b");
    let status = updated.status();
    let body = updated.text().await.expect("update body");
    assert_eq!(status, 200, "update failed: {body}");
    let updated: Value = serde_json::from_str(&body).expect("updated json");
    assert_eq!(updated["name"], json!("Cross-node sample"));
    let listed = server_a
        .client
        .get(server_a.url("/api/connections"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("list connections on a");
    let listed: Value = listed.json().await.expect("connections a json");
    assert!(
        listed["connections"]
            .as_array()
            .expect("connections")
            .iter()
            .any(|item| item["id"] == json!(connection_id)
                && item["name"] == json!("Cross-node sample"))
    );

    let tested = server_b
        .client
        .post(server_b.url(&format!("/api/connections/{connection_id}/test")))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("test connection on b");
    assert_eq!(tested.status(), 200);
    let audit = server_a
        .client
        .get(server_a.url("/api/connector-audit?limit=20"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("list connector audit");
    let audit: Value = audit.json().await.expect("audit json");
    assert!(
        audit["entries"]
            .as_array()
            .expect("audit entries")
            .iter()
            .any(|entry| entry["connectionId"] == json!(connection_id)
                && entry["operation"] == json!("test")),
        "the test executed on node B must be audited for node A"
    );

    let deleted = server_a
        .client
        .delete(server_a.url(&format!("/api/connections/{connection_id}")))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("delete connection");
    let deleted: Value = deleted.json().await.expect("delete json");
    assert_eq!(deleted["deleted"], json!(true));
    let listed = server_b
        .client
        .get(server_b.url("/api/connections"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("list connections after delete");
    let listed: Value = listed.json().await.expect("connections json");
    assert!(
        !listed["connections"]
            .as_array()
            .expect("connections")
            .iter()
            .any(|item| item["id"] == json!(connection_id))
    );

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .expect("connect postgres");
    let store = PostgresConnectorStore::new(pool.clone(), "local");
    let fixture_id = format!("connector_fixture_{}", uuid::Uuid::now_v7());
    let manifest = json!({"id": fixture_id.clone(), "version": "1.0.0", "name": "fixture"});
    assert!(
        store
            .upsert_definition(
                &manifest.to_string(),
                &fixture_id,
                "1.0.0",
                "fixture",
                "http",
                Some("plugin_fixture"),
                Some("1.0.0"),
                "enabled",
                "2026-11-03T00:00:00Z",
            )
            .await
            .expect("insert definition")
    );
    assert!(
        !store
            .upsert_definition(
                &manifest.to_string(),
                &fixture_id,
                "1.0.0",
                "fixture",
                "http",
                Some("plugin_fixture"),
                Some("1.0.0"),
                "enabled",
                "2026-11-03T00:00:00Z",
            )
            .await
            .expect("repeat definition")
    );
    assert!(
        store
            .upsert_definition(
                &json!({"id": fixture_id.clone(), "version": "1.0.0", "name": "changed"})
                    .to_string(),
                &fixture_id,
                "1.0.0",
                "fixture",
                "http",
                Some("plugin_fixture"),
                Some("1.0.0"),
                "enabled",
                "2026-11-03T00:00:00Z",
            )
            .await
            .is_err()
    );
    assert!(
        store
            .upsert_definition(
                &manifest.to_string(),
                &fixture_id,
                "1.0.0",
                "fixture",
                "http",
                Some("plugin_other"),
                Some("1.0.0"),
                "enabled",
                "2026-11-03T00:00:00Z",
            )
            .await
            .is_err()
    );
    assert_eq!(
        store
            .set_plugin_definitions_status("plugin_fixture", "1.0.0", "disabled")
            .await
            .expect("disable definition"),
        1
    );
    assert_eq!(
        store
            .delete_plugin_definitions("plugin_fixture")
            .await
            .expect("delete definition"),
        1
    );
    assert!(
        store
            .definition(&fixture_id, Some("1.0.0"))
            .await
            .expect("definition after delete")
            .is_none()
    );

    for dir in [&data_dir_a, &data_dir_b] {
        for table in ["connector_definitions", "connections", "connector_audit"] {
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
