use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use hpa_local::postgres_mcp_meta::{McpAuditRecord, McpServerRecord, PostgresMcpMetaStore};
use serde_json::{Value, json};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hpa")
}

fn temp_dir(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("hpa-pg-mcp-{label}-{}", uuid::Uuid::now_v7()));
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

fn offset_rfc3339(minutes: i64) -> String {
    (time::OffsetDateTime::now_utc() + time::Duration::minutes(minutes))
        .format(&time::format_description::well_known::Rfc3339)
        .expect("timestamp")
}

fn modern_meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name": "cross-node", "version": "1"},
    })
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
async fn mcp_metadata_is_authoritative_in_postgres_across_nodes() {
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

    let created = server_a
        .client
        .post(server_a.url("/api/mcp/tokens"))
        .bearer_auth(&admin)
        .json(&json!({"name": "cross-node token"}))
        .send()
        .await
        .expect("create mcp token");
    assert_eq!(created.status(), 200);
    let created: Value = created.json().await.expect("token json");
    let mcp_token = created["token"].as_str().expect("mcp token").to_string();
    let token_id = created["record"]["id"]
        .as_str()
        .expect("token id")
        .to_string();
    let listed = server_b
        .client
        .get(server_b.url("/api/mcp/tokens"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("list tokens");
    let listed: Value = listed.json().await.expect("tokens json");
    assert!(
        listed["tokens"]
            .as_array()
            .expect("tokens")
            .iter()
            .any(|token| token["id"] == json!(token_id)),
        "node B must see the MCP token created on node A"
    );
    let discover = server_b
        .client
        .post(server_b.url("/mcp"))
        .bearer_auth(&mcp_token)
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "server/discover")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "server/discover",
            "params": {"_meta": modern_meta()}
        }))
        .send()
        .await
        .expect("mcp discover");
    assert_eq!(
        discover.status(),
        200,
        "node B must authenticate the MCP token created on node A"
    );
    let audit = server_a
        .client
        .get(server_a.url("/api/mcp/audit?limit=20"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("list audit");
    let audit: Value = audit.json().await.expect("audit json");
    assert!(
        audit["audit"]
            .as_array()
            .expect("audit entries")
            .iter()
            .any(|entry| entry["tokenId"] == json!(token_id)),
        "the request served by node B must be audited in PostgreSQL for node A"
    );
    let revoked = server_b
        .client
        .delete(server_b.url(&format!("/api/mcp/tokens/{token_id}")))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("revoke token");
    let revoked: Value = revoked.json().await.expect("revoke json");
    assert_eq!(revoked["revoked"], json!(true));
    let listed = server_a
        .client
        .get(server_a.url("/api/mcp/tokens"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("list tokens after revoke");
    let listed: Value = listed.json().await.expect("tokens json");
    let token = listed["tokens"]
        .as_array()
        .expect("tokens")
        .iter()
        .find(|token| token["id"] == json!(token_id))
        .expect("token on node A");
    assert!(token["revokedAt"].is_string());
    let rejected = server_a
        .client
        .post(server_a.url("/mcp"))
        .bearer_auth(&mcp_token)
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "server/discover")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "server/discover",
            "params": {"_meta": modern_meta()}
        }))
        .send()
        .await
        .expect("mcp discover after revoke");
    assert_eq!(
        rejected.status(),
        401,
        "revocation issued on node B must reject the token on node A"
    );

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .expect("connect postgres");
    let store_a = PostgresMcpMetaStore::new(pool.clone(), "local");
    let store_b = PostgresMcpMetaStore::new(pool.clone(), "local");
    let now = now_rfc3339();
    let approval = store_a
        .create_approval(
            &token_id,
            "workflow.execute",
            "{\"workflowId\":\"wf\"}",
            "args-hash",
            "state-cross-node",
            "high",
            &now,
            &offset_rfc3339(10),
        )
        .await
        .expect("create approval");
    let pending = server_b
        .client
        .get(server_b.url("/api/mcp/approvals?status=pending"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("list approvals");
    let pending: Value = pending.json().await.expect("approvals json");
    assert!(
        pending["approvals"]
            .as_array()
            .expect("approvals")
            .iter()
            .any(|item| item["id"] == json!(approval.id)),
        "node B must see the MCP approval created on node A"
    );
    let (pending_count, _, _) = store_b.stats().await.expect("stats before decision");
    assert_eq!(pending_count, 1);
    let by_state = store_b
        .get_approval_by_state("state-cross-node")
        .await
        .expect("approval by state")
        .expect("approval state exists");
    assert_eq!(by_state.id, approval.id);
    assert!(
        store_b
            .decide_approval(&approval.id, "approved", &now_rfc3339())
            .await
            .expect("decide approval")
    );
    assert!(
        store_b
            .consume_approval(&approval.id, &now_rfc3339())
            .await
            .expect("consume approval")
    );
    assert!(
        !store_b
            .consume_approval(&approval.id, &now_rfc3339())
            .await
            .expect("re-consume approval")
    );
    let decided = store_a
        .get_approval(&approval.id)
        .await
        .expect("get approval")
        .expect("approval exists");
    assert_eq!(decided.status, "approved");
    assert!(decided.used_at.is_some());

    let expired = store_a
        .create_approval(
            &token_id,
            "workflow.execute",
            "{}",
            "args-hash-2",
            "state-expired",
            "high",
            "2020-01-01T00:00:00Z",
            "2020-01-01T00:01:00Z",
        )
        .await
        .expect("create expired approval");
    assert_eq!(
        store_b
            .expire_approvals(&now_rfc3339())
            .await
            .expect("expire approvals"),
        1
    );
    let expired = store_a
        .get_approval(&expired.id)
        .await
        .expect("get expired approval")
        .expect("approval exists");
    assert_eq!(expired.status, "expired");

    let audit = McpAuditRecord {
        id: format!("mcpaudit_{}", uuid::Uuid::now_v7()),
        at: now.clone(),
        token_id: Some(token_id.clone()),
        method: "tools/call".to_string(),
        tool_name: Some("workflow.execute".to_string()),
        status: "ok".to_string(),
        latency_ms: 12,
        detail_json: Some("{\"transport\":\"http\"}".to_string()),
    };
    store_a.insert_audit(&audit).await.expect("insert audit");
    let audit_b = store_b.list_audit(10).await.expect("list audit");
    assert!(audit_b.iter().any(|entry| entry.id == audit.id));

    let server_id = format!("mcpsrv_{}", uuid::Uuid::now_v7());
    store_a
        .upsert_server(&McpServerRecord {
            id: server_id.clone(),
            name: "cross-node server".to_string(),
            transport: "http".to_string(),
            command_json: "[]".to_string(),
            url: Some("http://127.0.0.1:9/mcp".to_string()),
            allowed_tools_json: "[\"workflow_list\"]".to_string(),
            network_json: "[]".to_string(),
            credential_env: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        })
        .await
        .expect("upsert server");
    let servers = server_b
        .client
        .get(server_b.url("/api/mcp/servers"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("list servers");
    let servers: Value = servers.json().await.expect("servers json");
    assert!(
        servers["servers"]
            .as_array()
            .expect("servers")
            .iter()
            .any(|server| server["id"] == json!(server_id))
    );
    assert!(
        store_b
            .delete_server(&server_id)
            .await
            .expect("delete server")
    );
    assert!(
        store_a
            .get_server(&server_id)
            .await
            .expect("get server")
            .is_none()
    );

    let (pending, tools, requests) = store_b.stats().await.expect("stats");
    assert_eq!(pending, 0);
    assert_eq!(tools, 0);
    assert!(requests >= 1);

    for dir in [&data_dir_a, &data_dir_b] {
        for table in ["mcp_tokens", "mcp_approvals", "mcp_audit", "mcp_servers"] {
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
