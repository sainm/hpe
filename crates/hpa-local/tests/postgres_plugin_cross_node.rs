use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use hpa_local::postgres_mcp_meta::{McpPluginServerInput, PostgresMcpMetaStore};
use hpa_local::postgres_plugin::PostgresPluginStore;
use serde_json::{Value, json};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hpa")
}

fn temp_dir(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("hpa-pg-plugin-{label}-{}", uuid::Uuid::now_v7()));
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
async fn plugin_metadata_is_authoritative_in_postgres_across_nodes() {
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
    let status = sample.status();
    let body = sample.text().await.expect("sample body");
    assert_eq!(status, 200, "sample install failed: {body}");
    let sample: Value = serde_json::from_str(&body).expect("sample json");
    let plugin_id = sample["plugin"]["id"]
        .as_str()
        .expect("plugin id")
        .to_string();
    assert_eq!(plugin_id, "plugin_sample_kit");
    assert_eq!(sample["plugin"]["status"], json!("enabled"));
    assert_eq!(
        sample["capabilities"]
            .as_array()
            .expect("capabilities")
            .len(),
        3
    );

    let listed = server_b
        .client
        .get(server_b.url("/api/plugins"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("list plugins on b");
    let listed: Value = listed.json().await.expect("plugins b json");
    assert!(
        listed["plugins"]
            .as_array()
            .expect("plugins")
            .iter()
            .any(|plugin| plugin["id"] == json!(plugin_id)
                && plugin["installedVersion"] == json!("1.0.0"))
    );
    let detail = server_b
        .client
        .get(server_b.url(&format!("/api/plugins/{plugin_id}")))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("plugin detail on b");
    let detail: Value = detail.json().await.expect("detail json");
    assert_eq!(detail["plugin"]["status"], json!("enabled"));
    assert_eq!(
        detail["capabilities"]
            .as_array()
            .expect("capabilities")
            .len(),
        3
    );
    let connectors = server_b
        .client
        .get(server_b.url("/api/connectors"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("connectors on b");
    let connectors: Value = connectors.json().await.expect("connectors json");
    assert!(
        connectors["connectors"]
            .as_array()
            .expect("connectors")
            .iter()
            .any(|connector| connector["connectorId"] == json!("sample_http")
                && connector["pluginId"] == json!(plugin_id))
    );
    let keys = server_b
        .client
        .get(server_b.url("/api/plugins/keys"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("plugin keys on b");
    let keys: Value = keys.json().await.expect("keys json");
    assert!(!keys["keys"].as_array().expect("keys").is_empty());

    let disabled = server_b
        .client
        .post(server_b.url(&format!("/api/plugins/{plugin_id}/disable")))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("disable on b");
    assert_eq!(disabled.status(), 200);
    let disabled: Value = disabled.json().await.expect("disabled json");
    assert_eq!(disabled["plugin"]["status"], json!("disabled"));
    let detail = server_a
        .client
        .get(server_a.url(&format!("/api/plugins/{plugin_id}")))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("plugin detail on a");
    let detail: Value = detail.json().await.expect("detail a json");
    assert_eq!(detail["plugin"]["status"], json!("disabled"));
    let enabled = server_a
        .client
        .post(server_a.url(&format!("/api/plugins/{plugin_id}/enable")))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("enable on a");
    assert_eq!(enabled.status(), 200);

    let package_path = data_dir_a
        .join("plugins")
        .join("samples")
        .join("plugin_sample_kit-1.0.0.hplugin");
    let published = server_a
        .client
        .post(server_a.url("/api/marketplace/publish"))
        .bearer_auth(&admin)
        .json(&json!({"path": package_path.to_string_lossy()}))
        .send()
        .await
        .expect("publish marketplace");
    let status = published.status();
    let body = published.text().await.expect("publish body");
    assert_eq!(status, 200, "publish failed: {body}");
    let published: Value = serde_json::from_str(&body).expect("publish json");
    assert_eq!(published["reviewStatus"], json!("pending"));

    let listings = server_b
        .client
        .get(server_b.url("/api/marketplace"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("marketplace list on b");
    let listings: Value = listings.json().await.expect("marketplace json");
    assert!(
        listings["plugins"]
            .as_array()
            .expect("marketplace plugins")
            .iter()
            .any(|listing| listing["pluginId"] == json!(plugin_id)
                && listing["reviewStatus"] == json!("pending"))
    );
    let reviewed = server_b
        .client
        .post(server_b.url(&format!(
            "/api/marketplace/{plugin_id}/versions/1.0.0/review"
        )))
        .bearer_auth(&admin)
        .json(&json!({"status": "approved"}))
        .send()
        .await
        .expect("review on b");
    assert_eq!(reviewed.status(), 200);
    let detail = server_a
        .client
        .get(server_a.url(&format!("/api/marketplace/{plugin_id}/versions/1.0.0")))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("marketplace detail on a");
    let detail: Value = detail.json().await.expect("marketplace detail json");
    assert_eq!(detail["reviewStatus"], json!("approved"));
    let rated = server_b
        .client
        .post(server_b.url(&format!("/api/marketplace/{plugin_id}/versions/1.0.0/rate")))
        .bearer_auth(&admin)
        .json(&json!({"rating": 5, "comment": "cross-node"}))
        .send()
        .await
        .expect("rate on b");
    assert_eq!(rated.status(), 200);
    let detail = server_a
        .client
        .get(server_a.url(&format!("/api/marketplace/{plugin_id}/versions/1.0.0")))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("marketplace detail after rate");
    let detail: Value = detail.json().await.expect("detail after rate json");
    assert_eq!(detail["rating"]["count"], json!(1));
    assert_eq!(detail["rating"]["average"], json!(5.0));
    let withdrawn = server_b
        .client
        .post(server_b.url(&format!(
            "/api/marketplace/{plugin_id}/versions/1.0.0/withdraw"
        )))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("withdraw on b");
    assert_eq!(withdrawn.status(), 200);

    let key = server_a
        .client
        .get(server_a.url("/api/plugins/key"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("plugin key");
    let key: Value = key.json().await.expect("key json");
    let public_key = key["publicKey"].as_str().expect("public key").to_string();
    let publisher = server_a
        .client
        .post(server_a.url("/api/marketplace/publishers"))
        .bearer_auth(&admin)
        .json(&json!({"id": "publisher_cross", "name": "Cross Node Publisher"}))
        .send()
        .await
        .expect("add publisher");
    assert_eq!(publisher.status(), 200);
    let publisher_key = server_a
        .client
        .post(server_a.url("/api/marketplace/publishers/publisher_cross/keys"))
        .bearer_auth(&admin)
        .json(&json!({
            "keyId": "cross-publisher-key",
            "publicKey": public_key,
            "label": "Cross publisher key",
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
            "id": "source_cross",
            "label": "Cross Source",
            "indexUri": "https://example.com/marketplace.json",
            "keyId": "cross-source-key",
            "publicKey": public_key,
            "keyLabel": "Cross source key",
        }))
        .send()
        .await
        .expect("add source");
    assert_eq!(source.status(), 200);
    let publishers = server_b
        .client
        .get(server_b.url("/api/marketplace/publishers"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("publishers on b");
    let publishers: Value = publishers.json().await.expect("publishers json");
    assert!(
        publishers["publishers"]
            .as_array()
            .expect("publishers")
            .iter()
            .any(|publisher| publisher["id"] == json!("publisher_cross")
                && publisher["keys"]
                    .as_array()
                    .expect("publisher keys")
                    .iter()
                    .any(|key| key["keyId"] == json!("cross-publisher-key")))
    );
    let sources = server_b
        .client
        .get(server_b.url("/api/marketplace/sources"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("sources on b");
    let sources: Value = sources.json().await.expect("sources json");
    assert!(
        sources["sources"]
            .as_array()
            .expect("sources")
            .iter()
            .any(|source| source["id"] == json!("source_cross")
                && source["keys"]
                    .as_array()
                    .expect("source keys")
                    .iter()
                    .any(|key| key["keyId"] == json!("cross-source-key")))
    );
    let revoked = server_a
        .client
        .delete(server_a.url("/api/marketplace/publishers/publisher_cross/keys"))
        .bearer_auth(&admin)
        .json(&json!({"keyId": "cross-publisher-key"}))
        .send()
        .await
        .expect("revoke publisher key");
    assert_eq!(revoked.status(), 200);
    let publishers = server_b
        .client
        .get(server_b.url("/api/marketplace/publishers"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("publishers after revoke");
    let publishers: Value = publishers.json().await.expect("publishers json");
    assert!(
        publishers["publishers"]
            .as_array()
            .expect("publishers")
            .iter()
            .any(|publisher| publisher["id"] == json!("publisher_cross")
                && publisher["keys"]
                    .as_array()
                    .expect("publisher keys")
                    .iter()
                    .any(|key| key["keyId"] == json!("cross-publisher-key")
                        && key["revokedAt"].is_string()))
    );

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .expect("connect postgres");
    let store = PostgresPluginStore::new(pool.clone(), "local");
    store
        .upsert_remote_package(
            "source_cross",
            "plugin_remote_fixture",
            "1.0.0",
            "publisher_cross",
            "cross-publisher-key",
            &"a".repeat(64),
            "/tmp/plugin_remote_fixture.hplugin",
            "approved",
            7,
            Some("2026-11-03T00:00:00Z"),
            None,
            None,
        )
        .await
        .expect("upsert remote package");
    let remote = server_b
        .client
        .get(server_b.url("/api/marketplace/remote?source=source_cross"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("remote packages on b");
    let remote: Value = remote.json().await.expect("remote json");
    assert!(
        remote["packages"]
            .as_array()
            .expect("remote packages")
            .iter()
            .any(
                |package| package["pluginId"] == json!("plugin_remote_fixture")
                    && package["sequence"] == json!(7)
            )
    );
    store
        .update_source_sequence("source_cross", 7, "2026-11-03T00:00:01Z")
        .await
        .expect("update source sequence");
    let sources = server_a
        .client
        .get(server_a.url("/api/marketplace/sources"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("sources after sequence");
    let sources: Value = sources.json().await.expect("sources json");
    assert!(
        sources["sources"]
            .as_array()
            .expect("sources")
            .iter()
            .any(|source| source["id"] == json!("source_cross")
                && source["lastSequence"] == json!(7)
                && source["approvedCount"] == json!(1))
    );
    store
        .withdraw_remote_package(
            "source_cross",
            "plugin_remote_fixture",
            "1.0.0",
            "2026-11-03T00:00:02Z",
            "fixture",
        )
        .await
        .expect("withdraw remote package");
    let package = store
        .remote_package("source_cross", "plugin_remote_fixture", "1.0.0")
        .await
        .expect("remote package")
        .expect("remote package exists");
    assert!(package.withdrawn_at.is_some());

    let mcp_store = PostgresMcpMetaStore::new(pool.clone(), "local");
    mcp_store
        .upsert_plugin_server(&McpPluginServerInput {
            id: "mcp_plugin_fixture".to_string(),
            name: "Plugin MCP Fixture".to_string(),
            transport: "stdio".to_string(),
            command_json: "[\"echo\"]".to_string(),
            url: None,
            allowed_tools_json: "[]".to_string(),
            network_json: "[]".to_string(),
            credential_env: None,
            plugin_id: "plugin_fixture".to_string(),
            plugin_version: "1.0.0".to_string(),
            created_at: "2026-11-03T00:00:00Z".to_string(),
            updated_at: "2026-11-03T00:00:00Z".to_string(),
        })
        .await
        .expect("upsert plugin server");
    assert_eq!(
        mcp_store
            .plugin_server_owner("mcp_plugin_fixture")
            .await
            .expect("plugin server owner"),
        Some((
            Some("plugin_fixture".to_string()),
            Some("1.0.0".to_string())
        ))
    );
    assert_eq!(
        mcp_store
            .set_plugin_servers_enabled("plugin_fixture", "1.0.0", false)
            .await
            .expect("disable plugin server"),
        1
    );
    assert_eq!(
        mcp_store
            .delete_plugin_servers("plugin_fixture")
            .await
            .expect("delete plugin servers"),
        1
    );
    assert!(
        mcp_store
            .plugin_server_owner("mcp_plugin_fixture")
            .await
            .expect("owner after delete")
            .is_none()
    );

    let uninstalled = server_a
        .client
        .post(server_a.url(&format!("/api/plugins/{plugin_id}/uninstall")))
        .bearer_auth(&admin)
        .json(&json!({"force": true}))
        .send()
        .await
        .expect("uninstall on a");
    assert_eq!(uninstalled.status(), 200);
    let detail = server_b
        .client
        .get(server_b.url(&format!("/api/plugins/{plugin_id}")))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("plugin detail after uninstall");
    let detail: Value = detail.json().await.expect("detail json");
    assert_eq!(detail["plugin"]["status"], json!("uninstalled"));

    for dir in [&data_dir_a, &data_dir_b] {
        for table in [
            "plugins",
            "plugin_packages",
            "plugin_capabilities",
            "plugin_trusted_keys",
            "plugin_events",
            "plugin_market_listings",
            "plugin_reviews",
            "marketplace_event_stream",
            "plugin_publishers",
            "plugin_publisher_keys",
            "marketplace_sources",
            "marketplace_source_keys",
            "marketplace_remote_packages",
            "mcp_servers",
        ] {
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
