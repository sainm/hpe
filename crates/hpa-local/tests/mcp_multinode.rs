use std::path::Path;
use std::process::{Child, Command as ProcessCommand, Stdio};
use std::time::Duration;

use reqwest::Client;
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations/postgres");
const MODERN: &str = "2026-07-28";

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hpa")
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("addr")
        .port()
}

fn run_cli(data_dir: &Path, args: &[&str]) -> String {
    let output = ProcessCommand::new(binary())
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

async fn wait_for_health(client: &Client, port: u16) {
    for _ in 0..200 {
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
    panic!("server on port {port} did not become healthy");
}

struct ServerGuard(Option<Child>);

impl ServerGuard {
    fn stop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        self.stop();
    }
}

fn start_server(
    data_dir: &Path,
    port: u16,
    node_id: Option<&str>,
    postgres_url: Option<&str>,
    task_tick_ms: u64,
) -> ServerGuard {
    let mut command = ProcessCommand::new(binary());
    command
        .arg("--data-dir")
        .arg(data_dir)
        .args(["serve", "--port", &port.to_string()])
        .env("HPA_APPROVAL_TICK_MS", "50")
        .env("HPA_MCP_TASK_TICK_MS", task_tick_ms.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(node_id) = node_id {
        command.env("HPA_NODE_ID", node_id);
    }
    if let Some(url) = postgres_url {
        command
            .env("HPA_POSTGRES_MCP", "1")
            .env("HPA_POSTGRES_URL", url);
    }
    ServerGuard(Some(command.spawn().expect("spawn server")))
}

fn copy_tree(source: &Path, target: &Path) {
    std::fs::create_dir_all(target).expect("create copied directory");
    for entry in std::fs::read_dir(source).expect("read source directory") {
        let entry = entry.expect("directory entry");
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&source_path, &target_path);
        } else {
            std::fs::copy(&source_path, &target_path).expect("copy file");
        }
    }
}

fn modern_meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": MODERN,
        "io.modelcontextprotocol/clientCapabilities": {
            "extensions": {"io.modelcontextprotocol/tasks": {}}
        },
        "io.modelcontextprotocol/clientInfo": {"name": "multi-node-test", "version": "1"}
    })
}

async fn mcp_post(
    client: &Client,
    port: u16,
    token: &str,
    method: &str,
    payload: Value,
) -> reqwest::Response {
    let mut builder = client
        .post(format!("http://127.0.0.1:{port}/mcp"))
        .bearer_auth(token)
        .header("MCP-Protocol-Version", MODERN)
        .header("Mcp-Method", method);
    let name_source = match method {
        "tools/call" => payload
            .get("params")
            .and_then(|params| params.get("name"))
            .and_then(Value::as_str),
        "tasks/get" | "tasks/update" | "tasks/cancel" => payload
            .get("params")
            .and_then(|params| params.get("taskId"))
            .and_then(Value::as_str),
        _ => None,
    };
    if let Some(name) = name_source {
        builder = builder.header("Mcp-Name", name);
    }
    builder.json(&payload).send().await.expect("MCP request")
}

async fn wait_task_status(
    client: &Client,
    port: u16,
    token: &str,
    task_id: &str,
    expected: &str,
) -> Value {
    for _ in 0..200 {
        let response = mcp_post(
            client,
            port,
            token,
            "tasks/get",
            json!({
                "jsonrpc": "2.0",
                "id": 100,
                "method": "tasks/get",
                "params": {"taskId": task_id, "_meta": modern_meta()}
            }),
        )
        .await;
        if response.status().is_success() {
            let body: Value = response.json().await.unwrap_or(Value::Null);
            if body["result"]["status"] == expected {
                return body;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("task `{task_id}` did not reach `{expected}`");
}

async fn wait_execution_status(client: &Client, port: u16, execution_id: &str, expected: &str) {
    for _ in 0..200 {
        let response = client
            .get(format!(
                "http://127.0.0.1:{port}/api/executions/{execution_id}"
            ))
            .send()
            .await
            .expect("execution status");
        if response.status().is_success() {
            let body: Value = response.json().await.unwrap_or(Value::Null);
            if body["summary"]["status"] == expected {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("execution `{execution_id}` did not reach `{expected}`");
}

#[tokio::test]
async fn durable_task_cancel_and_sse_cross_two_mcp_nodes() {
    let Ok(postgres_url) = std::env::var("HPA_POSTGRES_TEST_URL") else {
        eprintln!("skipped: HPA_POSTGRES_TEST_URL is not set");
        return;
    };
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&postgres_url)
        .await
        .expect("connect PostgreSQL");
    MIGRATOR.run(&pool).await.expect("apply migrations");
    pool.close().await;

    let root = std::env::temp_dir().join(format!("hpa-mcp-multinode-{}", uuid::Uuid::now_v7()));
    let prep_dir = root.join("prep");
    std::fs::create_dir_all(&prep_dir).expect("create prep directory");
    let client = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("HTTP client");

    let prep_port = free_port();
    let mut prep = start_server(&prep_dir, prep_port, None, None, 100);
    wait_for_health(&client, prep_port).await;
    let workflow: Value = serde_json::from_str(include_str!("../../../fixtures/08_approval.json"))
        .expect("approval fixture");
    assert_eq!(
        client
            .post(format!("http://127.0.0.1:{prep_port}/api/workflows"))
            .json(&json!({"ir": workflow}))
            .send()
            .await
            .expect("create workflow")
            .status(),
        200
    );
    assert_eq!(
        client
            .post(format!(
                "http://127.0.0.1:{prep_port}/api/workflows/wf_fixture_approval/publish"
            ))
            .send()
            .await
            .expect("publish workflow")
            .status(),
        200
    );
    prep.stop();
    let token_json: Value = serde_json::from_str(&run_cli(
        &prep_dir,
        &["mcp", "token", "create", "multi-node"],
    ))
    .expect("token JSON");
    let token = token_json["token"]
        .as_str()
        .expect("token secret")
        .to_string();

    let node_a_dir = root.join("node-a");
    let node_b_dir = root.join("node-b");
    copy_tree(&prep_dir, &node_a_dir);
    copy_tree(&prep_dir, &node_b_dir);

    let node_a_port = free_port();
    let node_b_port = free_port();
    let mut node_a = start_server(
        &node_a_dir,
        node_a_port,
        Some("node-a"),
        Some(&postgres_url),
        25,
    );
    let mut node_b = start_server(
        &node_b_dir,
        node_b_port,
        Some("node-b"),
        Some(&postgres_url),
        25,
    );
    wait_for_health(&client, node_a_port).await;
    wait_for_health(&client, node_b_port).await;

    let mut subscription = mcp_post(
        &client,
        node_b_port,
        &token,
        "subscriptions/listen",
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "subscriptions/listen",
            "params": {
                "_meta": modern_meta(),
                "subscriptions": {"tasksChanged": true}
            }
        }),
    )
    .await;
    assert_eq!(subscription.status(), 200);
    let initial = subscription
        .chunk()
        .await
        .expect("subscription stream")
        .expect("subscription initial event");
    assert!(String::from_utf8_lossy(&initial).contains("subscriptionId"));

    let execute = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "workflow.execute",
            "arguments": {
                "workflowId": "wf_fixture_approval",
                "version": "1.0.0",
                "input": {"value": 1}
            },
            "_meta": modern_meta()
        }
    });
    let first = mcp_post(&client, node_a_port, &token, "tools/call", execute.clone()).await;
    let first_status = first.status();
    let first = first.text().await.expect("first tool response");
    assert_eq!(first_status, 200, "{first}");
    let first: Value = serde_json::from_str(&first).expect("first tool response JSON");
    assert_eq!(first["result"]["resultType"], "input_required");
    let approval_id = first["result"]["_meta"]["hpa/approvalId"]
        .as_str()
        .expect("approval id");
    let request_state = first["result"]["requestState"]
        .as_str()
        .expect("request state")
        .to_string();
    let approval = client
        .post(format!(
            "http://127.0.0.1:{node_a_port}/api/mcp/approvals/{approval_id}/decide"
        ))
        .json(&json!({"decision": "approved"}))
        .send()
        .await
        .expect("approve MCP tool");
    assert_eq!(approval.status(), 200);
    let mut retry = execute;
    retry["params"]["requestState"] = Value::String(request_state);
    let second = mcp_post(&client, node_a_port, &token, "tools/call", retry).await;
    let second_status = second.status();
    let second = second.text().await.expect("second tool response");
    assert_eq!(second_status, 200, "{second}");
    let second: Value = serde_json::from_str(&second).expect("second tool response JSON");
    let task_id = second["result"]["structuredContent"]["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("task id missing: {second}"))
        .to_string();

    wait_execution_status(&client, node_a_port, &task_id, "waiting_approval").await;
    let waiting = wait_task_status(&client, node_b_port, &token, &task_id, "input_required").await;
    assert_eq!(
        waiting["result"]["_meta"]["io.modelcontextprotocol/task"]["ownerNodeId"],
        "node-a"
    );

    node_a.stop();
    let update = mcp_post(
        &client,
        node_b_port,
        &token,
        "tasks/update",
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tasks/update",
            "params": {
                "taskId": task_id,
                "update": {"status": "cancelled"},
                "_meta": modern_meta()
            }
        }),
    )
    .await;
    assert_eq!(update.status(), 200);
    let update: Value = update.json().await.expect("task update response");
    assert_eq!(update["result"]["waiting"]["cancelRequested"], true);
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(
        wait_task_status(&client, node_b_port, &token, &task_id, "input_required").await["result"]
            ["status"],
        "input_required"
    );

    let restarted_node_a_port = free_port();
    let mut restarted_node_a = start_server(
        &node_a_dir,
        restarted_node_a_port,
        Some("node-a"),
        Some(&postgres_url),
        25,
    );
    wait_for_health(&client, restarted_node_a_port).await;
    wait_task_status(&client, node_b_port, &token, &task_id, "cancelled").await;
    wait_execution_status(&client, restarted_node_a_port, &task_id, "cancelled").await;

    let mut observed = String::new();
    for _ in 0..20 {
        let chunk = tokio::time::timeout(Duration::from_secs(2), subscription.chunk())
            .await
            .expect("task SSE timeout")
            .expect("task SSE")
            .expect("task SSE chunk");
        observed.push_str(&String::from_utf8_lossy(&chunk));
        if observed.contains("notifications/tasks")
            && observed.contains(&task_id)
            && observed.contains("\"status\":\"cancelled\"")
        {
            break;
        }
    }
    assert!(
        observed.contains("notifications/tasks")
            && observed.contains(&task_id)
            && observed.contains("\"status\":\"cancelled\""),
        "missing cross-node task notification: {observed}"
    );

    restarted_node_a.stop();
    node_b.stop();
}
