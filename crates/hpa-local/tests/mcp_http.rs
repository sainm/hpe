use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;

static POSTGRES_MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations/postgres");

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hpa")
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/skills/invoice-audit")
}

fn run_cli(data_dir: &Path, args: &[&str]) -> String {
    let output = Command::new(binary())
        .arg("--data-dir")
        .arg(data_dir)
        .args(args)
        .output()
        .expect("run cli");
    assert!(
        output.status.success(),
        "cli `{args:?}` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

async fn wait_for_health(port: u16) {
    let client = reqwest::Client::new();
    for _ in 0..100 {
        if let Ok(response) = client
            .get(format!("http://127.0.0.1:{port}/api/health"))
            .send()
            .await
            && response.status().is_success()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("server did not become healthy");
}

fn modern_meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name": "integration", "version": "1"},
    })
}

#[tokio::test]
async fn http_gateway_supports_modern_legacy_and_auth() {
    let postgres_url = std::env::var("HPA_POSTGRES_TEST_URL").ok();
    if let Some(url) = postgres_url.as_deref() {
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(url)
            .await
            .expect("connect PostgreSQL for MCP HTTP test");
        POSTGRES_MIGRATOR
            .run(&pool)
            .await
            .expect("apply PostgreSQL migrations for MCP HTTP test");
        sqlx::query(
            "INSERT INTO hpa_control.tenants (id, name)
             VALUES ('local', 'Local')
             ON CONFLICT (id) DO NOTHING",
        )
        .execute(&pool)
        .await
        .expect("ensure local PostgreSQL tenant");
        pool.close().await;
    }
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("addr")
        .port();
    let data_dir = std::env::temp_dir().join(format!("hpa-mcp-http-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&data_dir).expect("data dir");
    run_cli(
        &data_dir,
        &["skill", "import", fixture().to_str().expect("fixture")],
    );
    run_cli(&data_dir, &["skill", "publish", "skill_invoice_audit"]);
    let token_json: Value = serde_json::from_str(&run_cli(
        &data_dir,
        &["mcp", "token", "create", "integration"],
    ))
    .expect("token json");
    let token = token_json["token"].as_str().expect("token").to_string();
    let other_token_json: Value = serde_json::from_str(&run_cli(
        &data_dir,
        &["mcp", "token", "create", "other-integration"],
    ))
    .expect("other token json");
    let other_token = other_token_json["token"]
        .as_str()
        .expect("other token")
        .to_string();

    let mut server = Command::new(binary());
    server
        .arg("--data-dir")
        .arg(&data_dir)
        .args(["serve", "--port", &port.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(url) = postgres_url.as_deref() {
        server
            .env("HPA_POSTGRES_MCP", "1")
            .env("HPA_POSTGRES_URL", url)
            .env("HPA_MCP_NOTIFICATION_MAX_LAG", "20");
    }
    let mut child: Child = server.spawn().expect("spawn server");
    wait_for_health(port).await;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .expect("http client");
    let endpoint = format!("http://127.0.0.1:{port}/mcp");

    let missing = client
        .post(&endpoint)
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "server/discover")
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "server/discover", "params": {"_meta": modern_meta()}}))
        .send()
        .await
        .expect("missing auth");
    assert_eq!(missing.status(), 401);
    assert!(missing.headers().contains_key("www-authenticate"));

    let discover = client
        .post(&endpoint)
        .bearer_auth(&token)
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "server/discover")
        .json(&json!({"jsonrpc": "2.0", "id": 2, "method": "server/discover", "params": {"_meta": modern_meta()}}))
        .send()
        .await
        .expect("discover");
    assert_eq!(discover.status(), 200);
    let discover_body: Value = discover.json().await.expect("discover json");
    assert_eq!(discover_body["result"]["resultType"], "complete");

    let mismatch = client
        .post(&endpoint)
        .bearer_auth(&token)
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/list")
        .json(&json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "workflow.list", "arguments": {}, "_meta": modern_meta()}}))
        .send()
        .await
        .expect("mismatch");
    assert_eq!(mismatch.status(), 400);

    let call = client
        .post(&endpoint)
        .bearer_auth(&token)
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/call")
        .header("Mcp-Name", "skill.invoice-audit.audit")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "skill.invoice-audit.audit",
                "arguments": {"invoiceId": "inv_http", "items": [{"amount": 3.5}]},
                "_meta": modern_meta()
            }
        }))
        .send()
        .await
        .expect("call");
    assert_eq!(call.status(), 200);
    let call_body: Value = call.json().await.expect("call json");
    assert_eq!(call_body["result"]["isError"], false);
    assert_eq!(
        call_body["result"]["structuredContent"]["total"],
        json!(3.5)
    );

    let legacy = client
        .post(&endpoint)
        .bearer_auth(&token)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "initialize",
            "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "legacy", "version": "1"}}
        }))
        .send()
        .await
        .expect("legacy init");
    assert_eq!(legacy.status(), 200);
    let session = legacy
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .expect("session")
        .to_string();
    let legacy_body: Value = legacy.json().await.expect("legacy json");
    assert_eq!(legacy_body["result"]["protocolVersion"], "2025-11-25");
    if let Some(url) = postgres_url.as_deref() {
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(url)
            .await
            .expect("connect persistent MCP verifier");
        let stored: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM hpa_control.mcp_sessions
                WHERE id = $1 AND expires_at > now() AND revoked_at IS NULL
             )",
        )
        .bind(&session)
        .fetch_one(&pool)
        .await
        .expect("verify persistent MCP session");
        assert!(stored);

        let notifier =
            hpa_local::persistent_mcp::PostgresMcpStore::new(pool.clone(), "local", "fixture");
        let mut sequences = Vec::new();
        for index in 0..25 {
            sequences.push(
                notifier
                    .publish_notification("tools", &json!({"fixture": index}))
                    .await
                    .expect("publish backpressure fixture"),
            );
        }

        let mut slow = client
            .get(&endpoint)
            .bearer_auth(&token)
            .header("MCP-Session-Id", &session)
            .header("Last-Event-ID", (sequences[0] - 1).to_string())
            .send()
            .await
            .expect("slow consumer stream");
        assert_eq!(slow.status(), 200);
        assert!(
            slow.chunk()
                .await
                .expect("slow consumer initial chunk")
                .is_some(),
            "SSE should start before enforcing backlog"
        );
        assert!(
            tokio::time::timeout(Duration::from_secs(2), slow.chunk())
                .await
                .expect("slow consumer must be disconnected")
                .expect("slow consumer stream error")
                .is_none(),
            "a consumer above the backlog limit must be disconnected"
        );
        drop(slow);

        let mut replay = client
            .get(&endpoint)
            .bearer_auth(&token)
            .header("MCP-Session-Id", &session)
            .header("Last-Event-ID", sequences[22].to_string())
            .send()
            .await
            .expect("replay stream");
        assert_eq!(replay.status(), 200);
        let mut replayed = String::new();
        for _ in 0..10 {
            let chunk = tokio::time::timeout(Duration::from_secs(2), replay.chunk())
                .await
                .expect("replay chunk timeout")
                .expect("replay stream ended before the retained window");
            let Some(chunk) = chunk else {
                break;
            };
            replayed.push_str(&String::from_utf8_lossy(&chunk));
            if replayed.contains(&format!("id: {}", sequences[23]))
                && replayed.contains(&format!("id: {}", sequences[24]))
            {
                break;
            }
        }
        assert!(
            replayed.contains(&format!("id: {}", sequences[23]))
                && replayed.contains(&format!("id: {}", sequences[24])),
            "retained notifications must replay after reconnect: {replayed}"
        );
        drop(replay);
        pool.close().await;
    }

    let cross_token_session = client
        .post(&endpoint)
        .bearer_auth(&other_token)
        .header("MCP-Session-Id", &session)
        .json(&json!({"jsonrpc": "2.0", "id": 60, "method": "tools/list", "params": {}}))
        .send()
        .await
        .expect("cross-token session");
    assert_eq!(cross_token_session.status(), 404);

    let list = client
        .post(&endpoint)
        .bearer_auth(&token)
        .header("MCP-Session-Id", &session)
        .json(&json!({"jsonrpc": "2.0", "id": 6, "method": "tools/list", "params": {}}))
        .send()
        .await
        .expect("legacy list");
    let list_body: Value = list.json().await.expect("list json");
    assert!(
        list_body["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .any(|tool| tool["name"] == "skill.invoice-audit.audit")
    );
    assert!(
        list_body["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .all(|tool| !tool["name"]
                .as_str()
                .unwrap_or_default()
                .starts_with("test_")),
        "conformance fixture tools must not exist outside HPA_MCP_CONFORMANCE mode"
    );

    let legacy_stream = client
        .get(&endpoint)
        .bearer_auth(&token)
        .header("MCP-Session-Id", &session)
        .send()
        .await
        .expect("legacy stream");
    assert_eq!(legacy_stream.status(), 200);
    assert_eq!(
        legacy_stream
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default(),
        "text/event-stream"
    );
    drop(legacy_stream);

    let mut subscription = client
        .post(&endpoint)
        .bearer_auth(&token)
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "subscriptions/listen")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "subscriptions/listen",
            "params": {
                "_meta": modern_meta(),
                "subscriptions": {
                    "toolsListChanged": true,
                    "resourcesListChanged": true
                }
            }
        }))
        .send()
        .await
        .expect("subscription");
    assert_eq!(subscription.status(), 200);
    assert_eq!(
        subscription
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default(),
        "text/event-stream"
    );
    let first_chunk = subscription
        .chunk()
        .await
        .expect("read subscription")
        .expect("subscription chunk");
    assert!(
        String::from_utf8_lossy(&first_chunk).contains("subscriptionId"),
        "subscription event must contain a handle"
    );
    let ir: Value =
        serde_json::from_str(include_str!("../../../fixtures/01_sequence.json")).expect("workflow");
    let created = client
        .post(format!("http://127.0.0.1:{port}/api/workflows"))
        .json(&json!({"ir": ir}))
        .send()
        .await
        .expect("create workflow");
    assert_eq!(created.status(), 200);
    let published = client
        .post(format!(
            "http://127.0.0.1:{port}/api/workflows/wf_fixture_sequence/publish"
        ))
        .send()
        .await
        .expect("publish workflow");
    assert_eq!(published.status(), 200);
    let mut observed = String::new();
    for _ in 0..6 {
        let notification = tokio::time::timeout(Duration::from_secs(2), subscription.chunk())
            .await
            .expect("subscription notification timeout")
            .expect("subscription notification")
            .expect("notification chunk");
        observed.push_str(&String::from_utf8_lossy(&notification));
        if observed.contains("notifications/tools/list_changed") {
            break;
        }
    }
    assert!(
        observed.contains("notifications/tools/list_changed"),
        "catalog mutation must notify subscribers: {observed}"
    );
    drop(subscription);

    let origin = client
        .post(&endpoint)
        .bearer_auth(&token)
        .header("Origin", "https://evil.example")
        .json(&json!({}))
        .send()
        .await
        .expect("origin");
    assert_eq!(origin.status(), 403);

    let source: Value = client
        .get(format!(
            "http://127.0.0.1:{port}/api/skills/skill_invoice_audit/versions/1.0.0/source"
        ))
        .send()
        .await
        .expect("skill source")
        .json()
        .await
        .expect("skill source JSON");
    let mut skill_md = source["files"]["SKILL.md"]
        .as_str()
        .expect("SKILL.md")
        .to_string();
    skill_md.push_str("\nEdited through HTTP.\n");
    let draft = client
        .post(format!(
            "http://127.0.0.1:{port}/api/skills/skill_invoice_audit/versions/1.0.0/draft"
        ))
        .json(&json!({
            "version": "1.1.0",
            "files": {"SKILL.md": skill_md}
        }))
        .send()
        .await
        .expect("skill draft");
    assert_eq!(draft.status(), 200, "{}", draft.text().await.unwrap());
    let signed: Value = client
        .post(format!(
            "http://127.0.0.1:{port}/api/skills/skill_invoice_audit/versions/1.1.0/sign"
        ))
        .send()
        .await
        .expect("skill sign")
        .json()
        .await
        .expect("signature JSON");
    assert_eq!(signed["signature"]["verified"], true);
    let published = client
        .post(format!(
            "http://127.0.0.1:{port}/api/skills/skill_invoice_audit/versions/1.1.0/publish"
        ))
        .send()
        .await
        .expect("publish edited skill");
    assert_eq!(
        published.status(),
        200,
        "{}",
        published.text().await.unwrap()
    );

    let metadata = client
        .get(format!(
            "http://127.0.0.1:{port}/.well-known/oauth-protected-resource"
        ))
        .send()
        .await
        .expect("metadata");
    assert_eq!(metadata.status(), 200);
    let metadata_body: Value = metadata.json().await.expect("metadata json");
    assert_eq!(
        metadata_body["resource"],
        json!(format!("http://127.0.0.1:{port}/mcp"))
    );

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&data_dir);
}
