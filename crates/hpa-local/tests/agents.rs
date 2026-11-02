use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hpa")
}

fn skill_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/skills/invoice-audit")
}

fn agent_fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/agents")
        .join(name)
}

fn workflow_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/08_approval.json")
}

fn run_cli(data_dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(binary())
        .arg("--data-dir")
        .arg(data_dir)
        .args(args)
        .output()
        .expect("run cli")
}

fn require_cli(data_dir: &Path, args: &[&str]) -> Value {
    let output = run_cli(data_dir, args);
    assert!(
        output.status.success(),
        "cli `{args:?}` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("cli json")
}

fn temp_dir(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("hpa-agents-{label}-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&path).expect("temp dir");
    path
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

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("addr")
        .port()
}

struct Server {
    child: Child,
    port: u16,
    client: reqwest::Client,
}

impl Server {
    async fn start(data_dir: &Path) -> Self {
        Self::start_with_env(data_dir, &[]).await
    }

    async fn start_with_env(data_dir: &Path, environment: &[(&str, String)]) -> Self {
        let port = free_port();
        let mut command = Command::new(binary());
        command
            .arg("--data-dir")
            .arg(data_dir)
            .args(["serve", "--port", &port.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for (key, value) in environment {
            command.env(key, value);
        }
        let child = command.spawn().expect("spawn server");
        wait_for_health(port).await;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .expect("http client");
        Self {
            child,
            port,
            client,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        let response = self
            .client
            .post(self.url(path))
            .json(&body)
            .send()
            .await
            .expect("post");
        let status = response.status().as_u16();
        let value = response.json().await.unwrap_or(Value::Null);
        (status, value)
    }

    async fn get(&self, path: &str) -> (u16, Value) {
        let response = self.client.get(self.url(path)).send().await.expect("get");
        let status = response.status().as_u16();
        let value = response.json().await.unwrap_or(Value::Null);
        (status, value)
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn prepare_skill(data_dir: &Path) {
    let output = run_cli(
        data_dir,
        &["skill", "import", skill_fixture().to_str().expect("skill")],
    );
    assert!(
        output.status.success(),
        "skill import failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = run_cli(data_dir, &["skill", "publish", "skill_invoice_audit"]);
    assert!(
        output.status.success(),
        "skill publish failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn invoice_input() -> Value {
    json!({
        "invoiceId": "inv_42",
        "items": [
            {"name": "license", "amount": 120.5},
            {"name": "support", "amount": 79.5}
        ]
    })
}

#[test]
fn agent_version_is_immutable_and_replay_matches() {
    let data_dir = temp_dir("cli");
    prepare_skill(&data_dir);
    let path = agent_fixture("invoice-auditor.agent.json");
    let imported = require_cli(
        &data_dir,
        &["agent", "import", path.to_str().expect("agent")],
    );
    assert_eq!(imported["inserted"], true);
    require_cli(&data_dir, &["agent", "publish", "agent_invoice_auditor"]);
    let input = serde_json::to_string(&invoice_input()).expect("input");
    let run = require_cli(
        &data_dir,
        &["agent", "run", "agent_invoice_auditor", "--input", &input],
    );
    assert_eq!(run["status"], "succeeded");
    assert_eq!(run["output"]["audit"]["total"], 200.0);
    assert_eq!(run["toolCalls"], 1);
    let session_id = run["id"].as_str().expect("session").to_string();
    let replay = require_cli(&data_dir, &["agent", "replay", &session_id]);
    assert_eq!(replay["matches"], true);
    assert_eq!(replay["replayMode"], "trace");

    let original = std::fs::read_to_string(&path).expect("fixture");
    let mut changed: Value = serde_json::from_str(&original).expect("config");
    changed["description"] = json!("different content with the same version");
    let changed_path = data_dir.join("changed.agent.json");
    std::fs::write(
        &changed_path,
        serde_json::to_string_pretty(&changed).expect("changed"),
    )
    .expect("write");
    let frozen = run_cli(
        &data_dir,
        &[
            "agent",
            "import",
            changed_path.to_str().expect("changed path"),
        ],
    );
    assert!(!frozen.status.success());
    assert!(
        String::from_utf8_lossy(&frozen.stderr).contains("frozen"),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&frozen.stderr)
    );

    let mut invalid: Value = serde_json::from_str(&original).expect("config");
    invalid["version"] = json!("9.9.9");
    invalid["tools"] = json!(["hack.execute"]);
    let invalid_path = data_dir.join("invalid.agent.json");
    std::fs::write(
        &invalid_path,
        serde_json::to_string_pretty(&invalid).expect("invalid"),
    )
    .expect("write");
    let rejected = run_cli(
        &data_dir,
        &[
            "agent",
            "import",
            invalid_path.to_str().expect("invalid path"),
        ],
    );
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("unknown tool"),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&rejected.stderr)
    );
}

#[tokio::test]
async fn agent_tool_service_rejects_unbound_http_calls() {
    let data_dir = temp_dir("tool-auth");
    let mut server = Server::start(&data_dir).await;
    let body = json!({
        "tenantId": "local",
        "sessionId": "agentsess_missing",
        "callId": "call_missing",
        "tool": "workflow.list",
        "arguments": {},
        "idempotencyKey": "call_missing",
        "risk": "read"
    });
    let (status, _) = server
        .post("/api/internal/agent-tool-calls", body.clone())
        .await;
    assert_eq!(status, 403);
    let response = server
        .client
        .post(server.url("/api/internal/agent-tool-calls"))
        .header("x-hpa-tenant-id", "local")
        .json(&body)
        .send()
        .await
        .expect("tool request");
    assert_eq!(response.status().as_u16(), 403);
    let response = server
        .client
        .post(server.url("/api/internal/agent-tool-calls"))
        .header("x-hpa-tenant-id", "local")
        .header("authorization", "Bearer invalid-capability")
        .json(&body)
        .send()
        .await
        .expect("tool request");
    assert_eq!(response.status().as_u16(), 403);
    server.stop();
}

#[tokio::test]
async fn agent_approval_does_not_bypass_workflow_approval() {
    let data_dir = temp_dir("approval");
    let mut server = Server::start(&data_dir).await;
    let workflow: Value =
        serde_json::from_str(&std::fs::read_to_string(workflow_fixture()).expect("workflow"))
            .expect("workflow json");
    let (status, _) = server.post("/api/workflows", json!({"ir": workflow})).await;
    assert_eq!(status, 200);
    let (status, _) = server
        .post("/api/workflows/wf_fixture_approval/publish", json!({}))
        .await;
    assert_eq!(status, 200);
    let config: Value = serde_json::from_str(
        &std::fs::read_to_string(agent_fixture("workflow-supervisor.agent.json")).expect("agent"),
    )
    .expect("agent json");
    let (status, _) = server.post("/api/agents", json!({"config": config})).await;
    assert_eq!(status, 200);
    let (status, _) = server
        .post("/api/agents/agent_workflow_supervisor/publish", json!({}))
        .await;
    assert_eq!(status, 200);
    let (status, run) = server
        .post(
            "/api/agents/agent_workflow_supervisor/run",
            json!({"input": {
                "workflowId": "wf_fixture_approval",
                "workflowVersion": "1.0.0",
                "input": {}
            }}),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(run["status"], "waiting_approval");
    assert!(run["pendingApproval"].is_string());
    let session_id = run["id"].as_str().expect("session").to_string();
    let (_, approvals) = server.get("/api/agents/approvals?status=pending").await;
    assert_eq!(
        approvals["approvals"].as_array().expect("approvals").len(),
        1
    );
    let approval_id = approvals["approvals"][0]["id"]
        .as_str()
        .expect("approval")
        .to_string();

    server.stop();
    let mut server = Server::start(&data_dir).await;
    let (status, session) = server
        .get(&format!("/api/agents/sessions/{session_id}"))
        .await;
    assert_eq!(status, 200);
    assert_eq!(session["status"], "waiting_approval");
    let (_, approvals) = server.get("/api/agents/approvals?status=pending").await;
    assert_eq!(
        approvals["approvals"].as_array().expect("approvals").len(),
        1
    );
    let (status, decided) = server
        .post(
            &format!("/api/agents/approvals/{approval_id}/decide"),
            json!({"decision": "approved"}),
        )
        .await;
    assert_eq!(status, 200);
    let session = &decided["session"];
    assert_eq!(session["status"], "succeeded");
    assert_eq!(session["output"]["execution"]["status"], "waiting_approval");
    assert_eq!(session["toolCalls"], 2);
    let execution_id = session["output"]["execution"]["executionId"]
        .as_str()
        .expect("execution")
        .to_string();
    let (_, workflow_approvals) = server.get("/api/approvals").await;
    let pending = workflow_approvals
        .as_array()
        .expect("workflow approvals")
        .iter()
        .filter(|row| row["executionId"] == execution_id)
        .collect::<Vec<_>>();
    assert_eq!(pending.len(), 1);
    let node_id = pending[0]["request"]["nodeId"]
        .as_str()
        .expect("node id")
        .to_string();
    let (status, _) = server
        .post(
            &format!("/api/executions/{execution_id}/approvals"),
            json!({"nodeId": node_id, "decision": "approved", "form": {"decision": "approve"}}),
        )
        .await;
    assert_eq!(status, 200);
    let mut execution_status = String::new();
    for _ in 0..100 {
        let (_, execution) = server.get(&format!("/api/executions/{execution_id}")).await;
        execution_status = execution["summary"]["status"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if matches!(
            execution_status.as_str(),
            "succeeded" | "failed" | "cancelled"
        ) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(execution_status, "succeeded");
    server.stop();
}

#[tokio::test]
async fn agent_handoff_inherits_budget_and_depth() {
    let data_dir = temp_dir("handoff");
    prepare_skill(&data_dir);
    let mut server = Server::start(&data_dir).await;
    let child = json!({
        "schemaVersion": "hpa.agent/v1",
        "id": "agent_budget_child",
        "name": "Budget Child",
        "version": "1.0.0",
        "tools": ["skill.invoice-audit.audit"],
        "planner": {
            "kind": "deterministic",
            "plan": [
                {"kind": "tool", "tool": "skill.invoice-audit.audit", "arguments": {"invoiceId": {"$input": "invoiceId"}, "items": {"$input": "items"}}},
                {"kind": "tool", "tool": "skill.invoice-audit.audit", "arguments": {"invoiceId": {"$input": "invoiceId"}, "items": {"$input": "items"}}},
                {"kind": "respond", "output": {"$last": true}}
            ]
        },
        "limits": {"maxSteps": 5, "maxToolCalls": 5, "timeoutMs": 30000, "maxTokens": 8000, "maxCostUsd": 0.25, "maxHandoffs": 2, "maxDepth": 2},
        "subAgents": []
    });
    let parent = json!({
        "schemaVersion": "hpa.agent/v1",
        "id": "agent_budget_parent",
        "name": "Budget Parent",
        "version": "1.0.0",
        "tools": [],
        "planner": {
            "kind": "deterministic",
            "plan": [
                {"kind": "handoff", "agentId": "agent_budget_child", "input": {"$input": true}},
                {"kind": "respond", "output": {"$last": true}}
            ]
        },
        "limits": {"maxSteps": 5, "maxToolCalls": 1, "timeoutMs": 30000, "maxTokens": 8000, "maxCostUsd": 0.25, "maxHandoffs": 2, "maxDepth": 2},
        "subAgents": ["agent_budget_child"]
    });
    for config in [&child, &parent] {
        let (status, body) = server.post("/api/agents", json!({"config": config})).await;
        assert_eq!(status, 200, "create failed: {body}");
    }
    for id in ["agent_budget_child", "agent_budget_parent"] {
        let (status, body) = server
            .post(&format!("/api/agents/{id}/publish"), json!({}))
            .await;
        assert_eq!(status, 200, "publish failed: {body}");
    }
    let (status, session) = server
        .post(
            "/api/agents/agent_budget_parent/run",
            json!({"input": invoice_input()}),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(session["status"], "failed");
    assert!(
        session["error"]
            .as_str()
            .expect("error")
            .contains("AGENT_HANDOFF_CHILD_FAILED")
    );
    let child_session_id = session["childSessionId"]
        .as_str()
        .expect("child session")
        .to_string();
    let (_, child_session) = server
        .get(&format!("/api/agents/sessions/{child_session_id}"))
        .await;
    assert_eq!(child_session["status"], "budget_exhausted");
    assert!(
        child_session["error"]
            .as_str()
            .expect("error")
            .contains("AGENT_TOOL_BUDGET")
    );

    let recursive = json!({
        "schemaVersion": "hpa.agent/v1",
        "id": "agent_recursive",
        "name": "Recursive",
        "version": "1.0.0",
        "tools": [],
        "planner": {
            "kind": "deterministic",
            "plan": [
                {"kind": "handoff", "agentId": "agent_recursive", "input": {"$input": true}},
                {"kind": "respond", "output": {"$last": true}}
            ]
        },
        "limits": {"maxSteps": 5, "maxToolCalls": 2, "timeoutMs": 30000, "maxTokens": 8000, "maxCostUsd": 0.25, "maxHandoffs": 2, "maxDepth": 1},
        "subAgents": ["agent_recursive"]
    });
    let (status, body) = server
        .post("/api/agents", json!({"config": recursive}))
        .await;
    assert_eq!(status, 200, "recursive create failed: {body}");
    let (status, _) = server
        .post("/api/agents/agent_recursive/publish", json!({}))
        .await;
    assert_eq!(status, 200);
    let (_, session) = server
        .post("/api/agents/agent_recursive/run", json!({"input": {}}))
        .await;
    assert_eq!(session["status"], "failed");
    let child_id = session["childSessionId"].as_str().expect("recursive child");
    let (_, child_session) = server
        .get(&format!("/api/agents/sessions/{child_id}"))
        .await;
    assert_eq!(child_session["status"], "budget_exhausted");
    assert!(
        child_session["error"]
            .as_str()
            .expect("error")
            .contains("AGENT_HANDOFF_BUDGET")
    );
    server.stop();
}

enum MockMode {
    ToolThenFinal,
    Loop,
    PythonToolThenFinal,
    PythonWorkflowThenFinal,
    PythonHandoffThenFinal,
    PythonSlowFinal,
}

struct MockPlanner {
    port: u16,
    hits: Arc<AtomicUsize>,
}

fn spawn_mock(mode: MockMode) -> MockPlanner {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
    let port = listener.local_addr().expect("mock addr").port();
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                break;
            };
            let hit = counter.fetch_add(1, Ordering::SeqCst);
            let _ = handle_mock(&mut stream, &mode, hit);
        }
    });
    MockPlanner { port, hits }
}

fn handle_mock(stream: &mut TcpStream, mode: &MockMode, hit: usize) -> std::io::Result<()> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&buffer[..position]).to_string();
            let length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|value| value.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            if buffer.len() >= position + 4 + length {
                break;
            }
        }
    }
    let tool_call = json!({
        "id": "chatcmpl-mock",
        "object": "chat.completion",
        "model": "mock",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": format!("call_{hit}"),
                    "type": "function",
                    "function": {"name": "workflow.list", "arguments": "{}"}
                }]
            },
            "finish_reason": "tool_calls"
        }],
        "usage": {"prompt_tokens": 100, "completion_tokens": 20, "total_tokens": 120}
    });
    let final_message = json!({
        "id": "chatcmpl-mock",
        "object": "chat.completion",
        "model": "mock",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "{\"summary\":\"catalog ready\"}"},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 120, "completion_tokens": 15, "total_tokens": 135}
    });
    let python_tool = json!({
        "id": "chatcmpl-mock",
        "object": "chat.completion",
        "model": "mock",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": serde_json::to_string(&json!({
                    "action": "tool",
                    "toolCall": {
                        "callId": format!("call_{hit}"),
                        "tool": "workflow.list",
                        "arguments": {},
                        "risk": "read",
                        "approvalRequired": false,
                        "idempotencyKey": format!("session-1/{hit}/call_{hit}")
                    }
                })).expect("python tool decision")
            },
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 100, "completion_tokens": 20, "total_tokens": 120}
    });
    let python_final = json!({
        "id": "chatcmpl-mock",
        "object": "chat.completion",
        "model": "mock",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "{\"action\":\"finish\",\"output\":{\"summary\":\"python catalog ready\"}}"
            },
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 120, "completion_tokens": 15, "total_tokens": 135}
    });
    if matches!(mode, MockMode::PythonSlowFinal) {
        std::thread::sleep(Duration::from_secs(5));
    }
    let python_workflow = json!({
        "id": "chatcmpl-mock",
        "object": "chat.completion",
        "model": "mock",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": serde_json::to_string(&json!({
                    "action": "tool",
                    "toolCall": {
                        "callId": format!("workflow_{hit}"),
                        "tool": "workflow.execute",
                        "arguments": {
                            "workflowId": "wf_fixture_sequence",
                            "version": "1.0.0",
                            "input": {"value": 41}
                        },
                        "risk": "read",
                        "approvalRequired": false,
                        "idempotencyKey": format!("session/step-{hit}/workflow_{hit}")
                    }
                })).expect("python workflow decision")
            },
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 100, "completion_tokens": 20, "total_tokens": 120}
    });
    let python_handoff = json!({
        "id": "chatcmpl-mock",
        "object": "chat.completion",
        "model": "mock",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": serde_json::to_string(&json!({
                    "action": "tool",
                    "toolCall": {
                        "callId": format!("handoff_{hit}"),
                        "tool": "agent.handoff",
                        "arguments": {
                            "agentId": "agent_python_child",
                            "input": {"message": "child"}
                        },
                        "risk": "read",
                        "approvalRequired": false,
                        "idempotencyKey": format!("session/step-{hit}/handoff_{hit}")
                    }
                })).expect("python handoff decision")
            },
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 100, "completion_tokens": 20, "total_tokens": 120}
    });
    let body = match mode {
        MockMode::ToolThenFinal if hit > 0 => final_message,
        MockMode::ToolThenFinal | MockMode::Loop => tool_call,
        MockMode::PythonToolThenFinal if hit > 0 => python_final,
        MockMode::PythonToolThenFinal => python_tool,
        MockMode::PythonWorkflowThenFinal if hit > 0 => python_final,
        MockMode::PythonWorkflowThenFinal => python_workflow,
        MockMode::PythonHandoffThenFinal if hit > 0 => python_final,
        MockMode::PythonHandoffThenFinal => python_handoff,
        MockMode::PythonSlowFinal => python_final,
    };
    let text = body.to_string();
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        text.len(),
        text
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

fn model_agent(id: &str, port: u16, limits: Value) -> Value {
    json!({
        "schemaVersion": "hpa.agent/v1",
        "id": id,
        "name": id,
        "version": "1.0.0",
        "instructions": "Return a short JSON summary.",
        "tools": ["workflow.list"],
        "planner": {
            "kind": "openAi",
            "endpoint": format!("http://127.0.0.1:{port}/v1/chat/completions"),
            "model": "mock",
            "temperature": 0,
            "inputCostPer1k": 1.0,
            "outputCostPer1k": 2.0,
            "network": []
        },
        "limits": limits,
        "subAgents": []
    })
}

fn python_agent(id: &str, limits: Value) -> Value {
    json!({
        "schemaVersion": "hpa.agent/v1",
        "id": id,
        "name": id,
        "version": "1.0.0",
        "instructions": "Return a short JSON summary after using the declared tool once.",
        "tools": ["workflow.list"],
        "planner": {
            "kind": "python",
            "model": "openai/hpa-test-model"
        },
        "limits": limits,
        "subAgents": []
    })
}

fn python_workflow_agent(id: &str, limits: Value) -> Value {
    let mut config = python_agent(id, limits);
    config["tools"] = json!(["workflow.execute"]);
    config
}

#[tokio::test]
async fn openai_planner_calls_tools_accounts_tokens_and_detects_loops() {
    let data_dir = temp_dir("model");
    let mut server = Server::start(&data_dir).await;
    let mock = spawn_mock(MockMode::ToolThenFinal);
    let config = model_agent(
        "agent_model_probe",
        mock.port,
        json!({"maxSteps": 6, "maxToolCalls": 3, "timeoutMs": 30000, "maxTokens": 10000, "maxCostUsd": 1.0, "maxHandoffs": 1, "maxDepth": 1}),
    );
    let (status, body) = server.post("/api/agents", json!({"config": config})).await;
    assert_eq!(status, 200, "create failed: {body}");
    let (status, _) = server
        .post("/api/agents/agent_model_probe/publish", json!({}))
        .await;
    assert_eq!(status, 200);
    let (status, session) = server
        .post("/api/agents/agent_model_probe/run", json!({"input": {}}))
        .await;
    assert_eq!(status, 200);
    assert_eq!(session["status"], "succeeded", "session: {session}");
    assert_eq!(session["output"]["summary"], "catalog ready");
    assert_eq!(session["tokensIn"], 220);
    assert_eq!(session["tokensOut"], 35);
    assert_eq!(session["toolCalls"], 1);
    assert_eq!(mock.hits.load(Ordering::SeqCst), 2);
    let session_id = session["id"].as_str().expect("session").to_string();
    let (status, replay) = server
        .post(
            &format!("/api/agents/sessions/{session_id}/replay"),
            json!({}),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(replay["deterministic"], false);
    assert_eq!(replay["matches"], false);

    let loop_mock = spawn_mock(MockMode::Loop);
    let loop_config = model_agent(
        "agent_model_loop",
        loop_mock.port,
        json!({"maxSteps": 8, "maxToolCalls": 6, "timeoutMs": 30000, "maxTokens": 10000, "maxCostUsd": 1.0, "maxHandoffs": 1, "maxDepth": 1}),
    );
    let (status, _) = server
        .post("/api/agents", json!({"config": loop_config}))
        .await;
    assert_eq!(status, 200);
    let (status, _) = server
        .post("/api/agents/agent_model_loop/publish", json!({}))
        .await;
    assert_eq!(status, 200);
    let (_, session) = server
        .post("/api/agents/agent_model_loop/run", json!({"input": {}}))
        .await;
    assert_eq!(session["status"], "failed");
    assert!(
        session["error"]
            .as_str()
            .expect("error")
            .contains("AGENT_LOOP")
    );

    let denied = json!({
        "schemaVersion": "hpa.agent/v1",
        "id": "agent_model_denied",
        "name": "Denied",
        "version": "1.0.0",
        "tools": ["workflow.list"],
        "planner": {
            "kind": "openAi",
            "endpoint": "https://example.com/v1/chat/completions",
            "model": "mock",
            "network": []
        },
        "subAgents": []
    });
    let (status, _) = server.post("/api/agents", json!({"config": denied})).await;
    assert_eq!(status, 200);
    let (status, _) = server
        .post("/api/agents/agent_model_denied/publish", json!({}))
        .await;
    assert_eq!(status, 200);
    let (_, session) = server
        .post("/api/agents/agent_model_denied/run", json!({"input": {}}))
        .await;
    assert_eq!(session["status"], "failed");
    assert!(
        session["error"]
            .as_str()
            .expect("error")
            .contains("allowlist")
    );
    server.stop();
}

#[tokio::test]
async fn python_planner_routes_tool_calls_back_to_rust() {
    let Ok(worker) = std::env::var("HPA_AI_WORKER_BIN") else {
        eprintln!("skipped: HPA_AI_WORKER_BIN is not set");
        return;
    };
    let data_dir = temp_dir("python-agent");
    let mock = spawn_mock(MockMode::PythonToolThenFinal);
    let mut server = Server::start_with_env(
        &data_dir,
        &[
            ("HPA_AI_WORKER_BIN", worker),
            ("HPA_AI_WORKER_PLANNER", "litellm".to_string()),
            ("HPA_AI_MODELS", "openai/hpa-test-model".to_string()),
            (
                "HPA_LITELLM_BASE_URL",
                format!("http://127.0.0.1:{}/v1", mock.port),
            ),
            ("HPA_MODEL_API_KEY", "controlled-test-key".to_string()),
        ],
    )
    .await;
    let config = python_agent(
        "agent_python_probe",
        json!({
            "maxSteps": 6,
            "maxToolCalls": 3,
            "timeoutMs": 30000,
            "maxTokens": 10000,
            "maxCostUsd": 1.0,
            "maxHandoffs": 1,
            "maxDepth": 1
        }),
    );
    let (status, body) = server.post("/api/agents", json!({"config": config})).await;
    assert_eq!(status, 200, "create failed: {body}");
    let (status, body) = server
        .post("/api/agents/agent_python_probe/publish", json!({}))
        .await;
    assert_eq!(status, 200, "publish failed: {body}");
    let (status, session) = server
        .post("/api/agents/agent_python_probe/run", json!({"input": {}}))
        .await;
    assert_eq!(status, 200, "run failed: {session}");
    assert_eq!(session["status"], "succeeded", "session: {session}");
    assert_eq!(session["output"]["summary"], "python catalog ready");
    assert_eq!(session["toolCalls"], 1);
    assert_eq!(session["tokensIn"], 220);
    assert_eq!(session["tokensOut"], 35);
    assert_eq!(mock.hits.load(Ordering::SeqCst), 2);
    let event_kinds = session["events"]
        .as_array()
        .expect("events")
        .iter()
        .filter_map(|event| event["kind"].as_str())
        .collect::<Vec<_>>();
    assert!(event_kinds.contains(&"agent.step"));
    assert!(event_kinds.contains(&"agent.tool_requested"));
    assert!(event_kinds.contains(&"agent.tool_completed"));
    assert!(event_kinds.contains(&"agent.finished"));
    server.stop();
}

#[tokio::test]
async fn python_planner_hands_off_to_a_budgeted_child_agent() {
    let Ok(worker) = std::env::var("HPA_AI_WORKER_BIN") else {
        eprintln!("skipped: HPA_AI_WORKER_BIN is not set");
        return;
    };
    let data_dir = temp_dir("python-handoff");
    let mock = spawn_mock(MockMode::PythonHandoffThenFinal);
    let mut server = Server::start_with_env(
        &data_dir,
        &[
            ("HPA_AI_WORKER_BIN", worker),
            ("HPA_AI_WORKER_PLANNER", "litellm".to_string()),
            ("HPA_AI_MODELS", "openai/hpa-test-model".to_string()),
            (
                "HPA_LITELLM_BASE_URL",
                format!("http://127.0.0.1:{}/v1", mock.port),
            ),
            ("HPA_MODEL_API_KEY", "controlled-test-key".to_string()),
        ],
    )
    .await;
    let child = json!({
        "schemaVersion": "hpa.agent/v1",
        "id": "agent_python_child",
        "name": "Python Child",
        "version": "1.0.0",
        "tools": [],
        "planner": {
            "kind": "deterministic",
            "plan": [{"kind": "respond", "output": {"child": "complete"}}]
        },
        "limits": {
            "maxSteps": 4,
            "maxToolCalls": 2,
            "timeoutMs": 30000,
            "maxTokens": 1000,
            "maxCostUsd": 0.1,
            "maxHandoffs": 1,
            "maxDepth": 1
        },
        "subAgents": []
    });
    let mut parent = python_agent(
        "agent_python_parent",
        json!({
            "maxSteps": 6,
            "maxToolCalls": 3,
            "timeoutMs": 30000,
            "maxTokens": 10000,
            "maxCostUsd": 1.0,
            "maxHandoffs": 1,
            "maxDepth": 2
        }),
    );
    parent["tools"] = json!([]);
    parent["subAgents"] = json!(["agent_python_child"]);
    for config in [&child, &parent] {
        let (status, body) = server.post("/api/agents", json!({"config": config})).await;
        assert_eq!(status, 200, "create failed: {body}");
    }
    for id in ["agent_python_child", "agent_python_parent"] {
        let (status, body) = server
            .post(&format!("/api/agents/{id}/publish"), json!({}))
            .await;
        assert_eq!(status, 200, "publish failed: {body}");
    }
    let (status, session) = server
        .post("/api/agents/agent_python_parent/run", json!({"input": {}}))
        .await;
    assert_eq!(status, 200, "run failed: {session}");
    assert_eq!(session["status"], "succeeded", "session: {session}");
    assert_eq!(session["output"]["summary"], "python catalog ready");
    assert_eq!(session["handoffs"], 1);
    assert_eq!(session["toolCalls"], 1);
    let child_session_id = session["childSessionId"].as_str().expect("child session");
    let (_, child_session) = server
        .get(&format!("/api/agents/sessions/{child_session_id}"))
        .await;
    assert_eq!(child_session["status"], "succeeded");
    assert_eq!(child_session["parentSessionId"], session["id"]);
    assert_eq!(child_session["output"]["child"], "complete");
    assert_eq!(mock.hits.load(Ordering::SeqCst), 2);
    server.stop();
}

#[tokio::test]
async fn python_worker_cannot_downgrade_high_risk_or_bypass_approval() {
    let Ok(worker) = std::env::var("HPA_AI_WORKER_BIN") else {
        eprintln!("skipped: HPA_AI_WORKER_BIN is not set");
        return;
    };
    let data_dir = temp_dir("python-approval");
    let mock = spawn_mock(MockMode::PythonWorkflowThenFinal);
    let mut server = Server::start_with_env(
        &data_dir,
        &[
            ("HPA_AI_WORKER_BIN", worker),
            ("HPA_AI_WORKER_PLANNER", "litellm".to_string()),
            ("HPA_AI_MODELS", "openai/hpa-test-model".to_string()),
            (
                "HPA_LITELLM_BASE_URL",
                format!("http://127.0.0.1:{}/v1", mock.port),
            ),
            ("HPA_MODEL_API_KEY", "controlled-test-key".to_string()),
        ],
    )
    .await;
    let workflow: Value = serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/01_sequence.json"),
        )
        .expect("workflow"),
    )
    .expect("workflow json");
    let (status, body) = server.post("/api/workflows", json!({"ir": workflow})).await;
    assert_eq!(status, 200, "workflow create failed: {body}");
    let (status, body) = server
        .post(
            "/api/workflows/wf_fixture_sequence/publish",
            json!({"version": "1.0.0"}),
        )
        .await;
    assert_eq!(status, 200, "workflow publish failed: {body}");
    let config = python_workflow_agent(
        "agent_python_approval",
        json!({
            "maxSteps": 6,
            "maxToolCalls": 3,
            "timeoutMs": 30000,
            "maxTokens": 10000,
            "maxCostUsd": 1.0,
            "maxHandoffs": 1,
            "maxDepth": 1
        }),
    );
    let (status, body) = server.post("/api/agents", json!({"config": config})).await;
    assert_eq!(status, 200, "agent create failed: {body}");
    let (status, body) = server
        .post("/api/agents/agent_python_approval/publish", json!({}))
        .await;
    assert_eq!(status, 200, "agent publish failed: {body}");
    let (status, session) = server
        .post(
            "/api/agents/agent_python_approval/run",
            json!({"input": {}}),
        )
        .await;
    assert_eq!(status, 200, "run failed: {session}");
    assert_eq!(session["status"], "waiting_approval", "session: {session}");
    let approval_id = session["pendingApproval"]
        .as_str()
        .expect("pending approval")
        .to_string();
    let (_, approvals) = server.get("/api/agents/approvals?status=pending").await;
    let approval = approvals["approvals"]
        .as_array()
        .expect("approvals")
        .iter()
        .find(|approval| approval["id"] == approval_id)
        .expect("agent approval");
    assert_eq!(approval["toolName"], "workflow.execute");
    assert_eq!(approval["risk"], "high");

    let (status, decided) = server
        .post(
            &format!("/api/agents/approvals/{approval_id}/decide"),
            json!({"decision": "approved"}),
        )
        .await;
    assert_eq!(status, 200, "approval failed: {decided}");
    assert_eq!(
        decided["session"]["status"], "succeeded",
        "session: {decided}"
    );
    assert_eq!(decided["session"]["toolCalls"], 1);
    let execution_id = decided["session"]["lastOutput"]["executionId"]
        .as_str()
        .expect("execution id");
    let (_, execution) = server.get(&format!("/api/executions/{execution_id}")).await;
    assert_eq!(execution["summary"]["status"], "succeeded");
    server.stop();
}

#[tokio::test]
async fn python_worker_receives_cross_request_cancellation() {
    let Ok(worker) = std::env::var("HPA_AI_WORKER_BIN") else {
        eprintln!("skipped: HPA_AI_WORKER_BIN is not set");
        return;
    };
    let data_dir = temp_dir("python-cancel");
    let mock = spawn_mock(MockMode::PythonSlowFinal);
    let mut server = Server::start_with_env(
        &data_dir,
        &[
            ("HPA_AI_WORKER_BIN", worker),
            ("HPA_AI_WORKER_PLANNER", "litellm".to_string()),
            ("HPA_AI_MODELS", "openai/hpa-test-model".to_string()),
            (
                "HPA_LITELLM_BASE_URL",
                format!("http://127.0.0.1:{}/v1", mock.port),
            ),
            ("HPA_MODEL_API_KEY", "controlled-test-key".to_string()),
        ],
    )
    .await;
    let mut config = python_agent(
        "agent_python_cancel",
        json!({
            "maxSteps": 4,
            "maxToolCalls": 2,
            "timeoutMs": 30000,
            "maxTokens": 10000,
            "maxCostUsd": 1.0,
            "maxHandoffs": 1,
            "maxDepth": 1
        }),
    );
    config["tools"] = json!([]);
    let (status, body) = server.post("/api/agents", json!({"config": config})).await;
    assert_eq!(status, 200, "create failed: {body}");
    let (status, body) = server
        .post("/api/agents/agent_python_cancel/publish", json!({}))
        .await;
    assert_eq!(status, 200, "publish failed: {body}");
    let client = server.client.clone();
    let run_url = server.url("/api/agents/agent_python_cancel/run");
    let run = tokio::spawn(async move {
        let response = client
            .post(run_url)
            .json(&json!({"input": {}}))
            .send()
            .await
            .expect("run request");
        let status = response.status().as_u16();
        let body = response.json::<Value>().await.expect("run response");
        (status, body)
    });
    let mut session_id = None;
    for _ in 0..100 {
        let (_, sessions) = server.get("/api/agents/sessions").await;
        if let Some(session) = sessions["sessions"]
            .as_array()
            .and_then(|sessions| sessions.first())
            && session["status"] == "running"
        {
            session_id = session["id"].as_str().map(str::to_string);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let session_id = session_id.expect("running session");
    let (status, body) = server
        .post(
            &format!("/api/agents/sessions/{session_id}/cancel"),
            json!({}),
        )
        .await;
    assert_eq!(status, 200, "cancel failed: {body}");
    assert_eq!(body["status"], "cancelled");
    let (status, completed) = run.await.expect("run task");
    assert_eq!(status, 200, "completed run: {completed}");
    assert_eq!(completed["status"], "cancelled", "session: {completed}");
    let (_, events) = server
        .get(&format!("/api/agents/sessions/{session_id}/events"))
        .await;
    assert!(
        events["events"]
            .as_array()
            .expect("events")
            .iter()
            .any(|event| event["kind"] == "agent.cancelled")
    );
    server.stop();
}

#[tokio::test]
async fn python_session_continues_after_control_plane_crash() {
    let Ok(worker) = std::env::var("HPA_AI_WORKER_BIN") else {
        eprintln!("skipped: HPA_AI_WORKER_BIN is not set");
        return;
    };
    let data_dir = temp_dir("python-recover");
    let mock = spawn_mock(MockMode::PythonSlowFinal);
    let mut environment = vec![
        ("HPA_AI_WORKER_BIN", worker),
        ("HPA_AI_WORKER_PLANNER", "litellm".to_string()),
        ("HPA_AI_MODELS", "openai/hpa-test-model".to_string()),
        (
            "HPA_LITELLM_BASE_URL",
            format!("http://127.0.0.1:{}/v1", mock.port),
        ),
        ("HPA_MODEL_API_KEY", "controlled-test-key".to_string()),
    ];
    if let Ok(postgres_url) = std::env::var("HPA_POSTGRES_TEST_URL") {
        environment.push(("HPA_AI_CHECKPOINT_POSTGRES_URL", postgres_url));
    }
    let mut server = Server::start_with_env(&data_dir, &environment).await;
    let mut config = python_agent(
        "agent_python_recover",
        json!({
            "maxSteps": 4,
            "maxToolCalls": 2,
            "timeoutMs": 30000,
            "maxTokens": 10000,
            "maxCostUsd": 1.0,
            "maxHandoffs": 1,
            "maxDepth": 1
        }),
    );
    config["tools"] = json!([]);
    let (status, body) = server.post("/api/agents", json!({"config": config})).await;
    assert_eq!(status, 200, "create failed: {body}");
    let (status, body) = server
        .post("/api/agents/agent_python_recover/publish", json!({}))
        .await;
    assert_eq!(status, 200, "publish failed: {body}");
    let client = server.client.clone();
    let run_url = server.url("/api/agents/agent_python_recover/run");
    let run = tokio::spawn(async move {
        client
            .post(run_url)
            .json(&json!({"input": {}}))
            .send()
            .await
    });
    let mut session_id = None;
    for _ in 0..100 {
        let (_, sessions) = server.get("/api/agents/sessions").await;
        if let Some(session) = sessions["sessions"]
            .as_array()
            .and_then(|sessions| sessions.first())
            && session["status"] == "running"
        {
            session_id = session["id"].as_str().map(str::to_string);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let session_id = session_id.expect("running session");
    server.stop();
    run.abort();
    let _ = run.await;

    let mut restarted = Server::start_with_env(&data_dir, &environment).await;
    let (status, interrupted) = restarted
        .get(&format!("/api/agents/sessions/{session_id}"))
        .await;
    assert_eq!(status, 200, "session read failed: {interrupted}");
    assert_eq!(interrupted["status"], "interrupted");
    let (status, resumed) = restarted
        .post(
            &format!("/api/agents/sessions/{session_id}/continue"),
            json!({}),
        )
        .await;
    assert_eq!(status, 200, "continue failed: {resumed}");
    assert_eq!(resumed["status"], "succeeded", "session: {resumed}");
    assert_eq!(resumed["output"]["summary"], "python catalog ready");
    restarted.stop();

    if let Ok(postgres_url) = std::env::var("HPA_POSTGRES_TEST_URL") {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&postgres_url)
            .await
            .expect("checkpoint postgres");
        let schema = hpa_local::ai_worker_client::checkpoint_schema_name("local");
        assert!(schema.starts_with("hpa_agent_cp_"));
        assert!(schema.len() <= 63);
        assert_ne!(
            schema,
            hpa_local::ai_worker_client::checkpoint_schema_name("tenant-2")
        );
        let table: Option<String> = sqlx::query_scalar("SELECT to_regclass($1)::text")
            .bind(format!("{schema}.checkpoints"))
            .fetch_one(&pool)
            .await
            .expect("checkpoint table lookup");
        assert_eq!(
            table.as_deref(),
            Some(format!("{schema}.checkpoints").as_str())
        );
        let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT count(*) FROM {schema}.checkpoints"
        )))
        .fetch_one(&pool)
        .await
        .expect("checkpoint count");
        assert!(count > 0, "PostgreSQL checkpoint schema is empty");
    }
}
