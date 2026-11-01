use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};

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

    let mut child: Child = Command::new(binary())
        .arg("--data-dir")
        .arg(&data_dir)
        .args(["serve", "--port", &port.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");
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

    let origin = client
        .post(&endpoint)
        .bearer_auth(&token)
        .header("Origin", "https://evil.example")
        .json(&json!({}))
        .send()
        .await
        .expect("origin");
    assert_eq!(origin.status(), 403);

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
