use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use hpa_core::ir::Workflow;
use hpa_core::runtime::{
    ExecutionState, ExecutionStatus, Frame, StepStatus, new_state, resume_approval, step,
};
use hpa_core::validator;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const SEQUENCE: &str = include_str!("../../../fixtures/01_sequence.json");
const CONDITION: &str = include_str!("../../../fixtures/02_condition.json");
const SWITCH: &str = include_str!("../../../fixtures/03_switch.json");
const LOOP_WORKFLOW: &str = include_str!("../../../fixtures/04_loop.json");
const PARALLEL: &str = include_str!("../../../fixtures/05_parallel.json");
const APPROVAL: &str = include_str!("../../../fixtures/08_approval.json");
const SUBFLOW: &str = include_str!("../../../fixtures/09_subflow.json");

struct Prepared {
    workflow: Workflow,
    plan: hpa_core::compiler::ExecutionPlan,
}

fn prepare(text: &str) -> Prepared {
    let compilation = validator::check_json(text).expect("fixture should validate");
    let workflow = Workflow::from_json_str(text).expect("fixture workflow");
    Prepared {
        workflow,
        plan: compilation.plan,
    }
}

async fn drive(prepared: &Prepared, state: &mut ExecutionState) {
    for _ in 0..2_000 {
        if state.status.is_terminal() || state.status == ExecutionStatus::WaitingApproval {
            break;
        }
        let result = step(&prepared.workflow, &prepared.plan, state).await;
        match result.status {
            StepStatus::Continue => {}
            StepStatus::Retry { delay_ms } => {
                tokio::time::sleep(Duration::from_millis(delay_ms.min(50))).await;
            }
            StepStatus::WaitingApproval | StepStatus::Succeeded | StepStatus::Failed => break,
        }
    }
}

fn start(prepared: &Prepared, input: Value) -> ExecutionState {
    new_state(
        &prepared.workflow,
        &prepared.plan,
        format!("exec_{}", uuid::Uuid::now_v7()),
        input,
        BTreeMap::new(),
        "local",
    )
    .expect("execution state")
}

#[tokio::test]
async fn sequence_executes() {
    let prepared = prepare(SEQUENCE);
    let mut state = start(&prepared, json!({"value": 41}));
    drive(&prepared, &mut state).await;
    assert_eq!(state.status, ExecutionStatus::Succeeded);
    assert_eq!(state.node_outputs.get("map"), Some(&json!({"value": 42})));
}

#[tokio::test]
async fn condition_executes_one_branch() {
    let prepared = prepare(CONDITION);
    let mut state = start(&prepared, json!({"amount": 2000}));
    drive(&prepared, &mut state).await;
    assert_eq!(state.status, ExecutionStatus::Succeeded);
    assert_eq!(
        state.node_outputs.get("high"),
        Some(&json!({"band": "high"}))
    );
    assert!(!state.node_outputs.contains_key("low"));

    let mut low_state = start(&prepared, json!({"amount": 10}));
    drive(&prepared, &mut low_state).await;
    assert_eq!(low_state.status, ExecutionStatus::Succeeded);
    assert_eq!(
        low_state.node_outputs.get("low"),
        Some(&json!({"band": "low"}))
    );
    assert!(!low_state.node_outputs.contains_key("high"));
}

#[tokio::test]
async fn switch_executes_default_or_case() {
    let prepared = prepare(SWITCH);
    let mut state = start(&prepared, json!({"kind": "b"}));
    drive(&prepared, &mut state).await;
    assert_eq!(state.status, ExecutionStatus::Succeeded);
    assert_eq!(state.node_outputs.get("b"), Some(&json!({"result": "B"})));
    assert!(!state.node_outputs.contains_key("a"));
    assert!(!state.node_outputs.contains_key("other"));

    let mut default_state = start(&prepared, json!({"kind": "z"}));
    drive(&prepared, &mut default_state).await;
    assert_eq!(default_state.status, ExecutionStatus::Succeeded);
    assert_eq!(
        default_state.node_outputs.get("other"),
        Some(&json!({"result": "OTHER"}))
    );
}

#[tokio::test]
async fn loop_aggregates_items() {
    let prepared = prepare(LOOP_WORKFLOW);
    let mut state = start(&prepared, json!({}));
    drive(&prepared, &mut state).await;
    assert_eq!(state.status, ExecutionStatus::Succeeded);
    let output = state.node_outputs.get("loop").expect("loop output");
    assert_eq!(output["count"], json!(3));
    assert_eq!(
        output["items"],
        json!([{"doubled": 2}, {"doubled": 4}, {"doubled": 6}])
    );
    assert_eq!(output["failed"], json!([]));
}

#[tokio::test]
async fn parallel_isolates_branches() {
    let prepared = prepare(PARALLEL);
    let mut state = start(&prepared, json!({}));
    drive(&prepared, &mut state).await;
    assert_eq!(state.status, ExecutionStatus::Succeeded);
    let output = state.node_outputs.get("parallel").expect("parallel output");
    assert_eq!(output["branches"]["left"], json!({"branch": "left"}));
    assert_eq!(output["branches"]["right"], json!({"branch": "right"}));
    assert_eq!(output["errors"], json!([]));
}

#[tokio::test]
async fn subflow_receives_input_and_returns_output() {
    let prepared = prepare(SUBFLOW);
    let mut state = start(&prepared, json!({"value": 21}));
    drive(&prepared, &mut state).await;
    assert_eq!(state.status, ExecutionStatus::Succeeded);
    assert_eq!(
        state.node_outputs.get("call"),
        Some(&json!({"doubled": 42}))
    );
}

#[tokio::test]
async fn approval_pauses_and_resumes_after_persistence() {
    let prepared = prepare(APPROVAL);
    let mut state = start(&prepared, json!({}));
    drive(&prepared, &mut state).await;
    assert_eq!(state.status, ExecutionStatus::WaitingApproval);
    let request = state.waiting.clone().expect("approval request");
    assert_eq!(request.node_id, "approve");

    let persisted = serde_json::to_string(&state).expect("serialize waiting state");
    let mut restored: ExecutionState =
        serde_json::from_str(&persisted).expect("restore waiting state");
    resume_approval(
        &mut restored,
        &prepared.plan,
        "approve",
        "approved",
        json!({"comment": "ok"}),
    )
    .expect("resume approval");
    drive(&prepared, &mut restored).await;
    assert_eq!(restored.status, ExecutionStatus::Succeeded);
    assert_eq!(
        restored.node_outputs.get("approved"),
        Some(&json!({"decision": "approved"}))
    );
    assert_eq!(
        restored
            .node_outputs
            .get("approve")
            .and_then(|value| value.get("comment")),
        Some(&json!("ok"))
    );
}

#[tokio::test]
async fn state_roundtrip_resumes_mid_execution() {
    let prepared = prepare(SEQUENCE);
    let mut state = start(&prepared, json!({"value": 1}));
    for _ in 0..2 {
        let result = step(&prepared.workflow, &prepared.plan, &mut state).await;
        assert!(matches!(
            result.status,
            StepStatus::Continue | StepStatus::Succeeded
        ));
    }
    let serialized = serde_json::to_string(&state).expect("serialize state");
    let mut restored: ExecutionState =
        serde_json::from_str(&serialized).expect("deserialize state");
    drive(&prepared, &mut restored).await;
    assert_eq!(restored.status, ExecutionStatus::Succeeded);
    assert_eq!(restored.node_outputs.get("map"), Some(&json!({"value": 2})));
}

async fn serve_http(responses: Vec<(u16, &'static str)>) -> (u16, tokio::task::JoinHandle<usize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind server");
    let port = listener.local_addr().expect("server address").port();
    let handle = tokio::spawn(async move {
        let mut count = 0;
        for (status, body) in responses {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let mut buffer = [0u8; 4096];
            let _ = socket.read(&mut buffer).await;
            let response = format!(
                "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;
            count += 1;
        }
        count
    });
    (port, handle)
}

fn http_workflow(port: u16, timeout_ms: u64) -> String {
    json!({
        "$schema": "https://hpa.dev/schema/ir/v0.1/workflow.schema.json",
        "schemaVersion": "hpa.dev/v1",
        "id": "wf_http_retry",
        "name": "HTTP Retry",
        "workflowVersion": "1.0.0",
        "inputs": [],
        "outputs": [],
        "variables": [],
        "nodes": [
            {"id": "start", "type": "start", "typeVersion": "1.0.0", "config": {}},
            {
                "id": "fetch",
                "type": "http.request",
                "typeVersion": "1.0.0",
                "config": {"method": "GET", "url": format!("http://127.0.0.1:{port}/data")},
                "retry": {
                    "maxAttempts": 3,
                    "backoff": {"kind": "constant", "baseMs": 1, "maxMs": 1, "jitter": false},
                    "retryOn": ["HTTP_5XX", "TIMEOUT"]
                },
                "timeoutMs": timeout_ms,
                "errorPolicy": {"mode": "fallback", "fallbackPort": "error"}
            },
            {
                "id": "fallback",
                "type": "transform",
                "typeVersion": "1.0.0",
                "config": {"mapping": {"ok": false}}
            },
            {"id": "end", "type": "end", "typeVersion": "1.0.0", "config": {}}
        ],
        "edges": [
            {"id": "e1", "source": "start", "target": "fetch"},
            {"id": "e2", "source": "fetch", "sourcePort": "out", "target": "end"},
            {"id": "e3", "source": "fetch", "sourcePort": "error", "target": "fallback"},
            {"id": "e4", "source": "fallback", "target": "end"}
        ]
    })
    .to_string()
}

#[tokio::test]
async fn http_retries_server_errors_without_network() {
    let (port, server) = serve_http(vec![(500, "{}"), (500, "{}"), (200, r#"{"ok":true}"#)]).await;
    let text = http_workflow(port, 5_000);
    let prepared = prepare(&text);
    let mut state = start(&prepared, json!({}));
    drive(&prepared, &mut state).await;
    assert_eq!(state.status, ExecutionStatus::Succeeded);
    let fetch = state.node_outputs.get("fetch").expect("fetch output");
    assert_eq!(fetch["status"], json!(200));
    assert_eq!(fetch["body"]["ok"], json!(true));
    let attempts = server.await.expect("server task");
    assert_eq!(attempts, 3);
}

#[tokio::test]
async fn http_timeout_uses_fallback_branch() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind server");
    let port = listener.local_addr().expect("server address").port();
    let server = tokio::spawn(async move {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        let mut buffer = [0u8; 4096];
        let _ = socket.read(&mut buffer).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let body = r#"{"ok":true}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = socket.write_all(response.as_bytes()).await;
    });
    let text = http_workflow(port, 50);
    let prepared = prepare(&text);
    let mut state = start(&prepared, json!({}));
    drive(&prepared, &mut state).await;
    assert_eq!(state.status, ExecutionStatus::Succeeded);
    assert_eq!(
        state.node_outputs.get("fallback"),
        Some(&json!({"ok": false}))
    );
    server.abort();
}

#[tokio::test]
async fn error_graph_runs_and_preserves_failure() {
    let text = json!({
        "$schema": "https://hpa.dev/schema/ir/v0.1/workflow.schema.json",
        "schemaVersion": "hpa.dev/v1",
        "id": "wf_error_graph",
        "name": "Error Graph",
        "workflowVersion": "1.0.0",
        "inputs": [],
        "outputs": [],
        "variables": [],
        "nodes": [
            {"id": "start", "type": "start", "typeVersion": "1.0.0", "config": {}},
            {
                "id": "fetch",
                "type": "http.request",
                "typeVersion": "1.0.0",
                "config": {"method": "GET", "url": "http://127.0.0.1:1/"}
            },
            {"id": "end", "type": "end", "typeVersion": "1.0.0", "config": {}}
        ],
        "edges": [
            {"id": "e1", "source": "start", "target": "fetch"},
            {"id": "e2", "source": "fetch", "target": "end"}
        ],
        "errorHandling": {
            "mode": "graph",
            "graph": {
                "nodes": [
                    {
                        "id": "error_marker",
                        "type": "transform",
                        "typeVersion": "1.0.0",
                        "config": {"mapping": {"handled": true}}
                    }
                ],
                "edges": []
            }
        }
    })
    .to_string();
    let prepared = prepare(&text);
    let mut state = start(&prepared, json!({}));
    drive(&prepared, &mut state).await;
    assert_eq!(state.status, ExecutionStatus::Failed);
    assert_eq!(
        state.node_outputs.get("error_marker"),
        Some(&json!({"handled": true}))
    );
    assert_eq!(
        state.error.as_ref().map(|error| error.code.as_str()),
        Some("NETWORK")
    );
}

#[tokio::test]
async fn dynamic_url_is_denied_before_network_access() {
    let text = json!({
        "$schema": "https://hpa.dev/schema/ir/v0.1/workflow.schema.json",
        "schemaVersion": "hpa.dev/v1",
        "id": "wf_dynamic_url",
        "name": "Dynamic URL",
        "workflowVersion": "1.0.0",
        "inputs": [{"name": "url", "type": "string", "required": true}],
        "outputs": [],
        "variables": [],
        "nodes": [
            {"id": "start", "type": "start", "typeVersion": "1.0.0", "config": {}},
            {
                "id": "fetch",
                "type": "http.request",
                "typeVersion": "1.0.0",
                "config": {"method": "GET", "url": {"$expr": ".input.url"}}
            },
            {"id": "end", "type": "end", "typeVersion": "1.0.0", "config": {}}
        ],
        "edges": [
            {"id": "e1", "source": "start", "target": "fetch"},
            {"id": "e2", "source": "fetch", "target": "end"}
        ]
    })
    .to_string();
    let prepared = prepare(&text);
    let mut state = start(&prepared, json!({"url": "https://example.com/"}));
    drive(&prepared, &mut state).await;
    assert_eq!(state.status, ExecutionStatus::Failed);
    assert_eq!(
        state.error.as_ref().map(|error| error.code.as_str()),
        Some("PERMISSION_NETWORK")
    );
}

#[tokio::test]
#[ignore = "requires outbound network"]
async fn https_request_uses_tls_client() {
    let text = json!({
        "$schema": "https://hpa.dev/schema/ir/v0.1/workflow.schema.json",
        "schemaVersion": "hpa.dev/v1",
        "id": "wf_https",
        "name": "HTTPS",
        "workflowVersion": "1.0.0",
        "inputs": [],
        "outputs": [],
        "variables": [],
        "nodes": [
            {"id": "start", "type": "start", "typeVersion": "1.0.0", "config": {}},
            {
                "id": "fetch",
                "type": "http.request",
                "typeVersion": "1.0.0",
                "config": {"method": "GET", "url": "https://example.com/"},
                "permissions": {"network": ["example.com"]}
            },
            {"id": "end", "type": "end", "typeVersion": "1.0.0", "config": {}}
        ],
        "edges": [
            {"id": "e1", "source": "start", "target": "fetch"},
            {"id": "e2", "source": "fetch", "target": "end"}
        ]
    })
    .to_string();
    let prepared = prepare(&text);
    let mut state = start(&prepared, json!({}));
    drive(&prepared, &mut state).await;
    assert_eq!(state.status, ExecutionStatus::Succeeded);
    assert_eq!(
        state
            .node_outputs
            .get("fetch")
            .and_then(|value| value.get("status")),
        Some(&json!(200))
    );
}

#[tokio::test]
async fn parallel_branches_run_concurrently() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind server");
    let port = listener.local_addr().expect("server address").port();
    let server = tokio::spawn(async move {
        let mut handles = Vec::new();
        for _ in 0..2 {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            handles.push(tokio::spawn(async move {
                let mut buffer = [0u8; 4096];
                let _ = socket.read(&mut buffer).await;
                tokio::time::sleep(Duration::from_millis(300)).await;
                let body = r#"{"ok":true}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            }));
        }
        for handle in handles {
            let _ = handle.await;
        }
    });
    let text = json!({
        "$schema": "https://hpa.dev/schema/ir/v0.1/workflow.schema.json",
        "schemaVersion": "hpa.dev/v1",
        "id": "wf_parallel_concurrent",
        "name": "Parallel Concurrent",
        "workflowVersion": "1.0.0",
        "inputs": [],
        "outputs": [],
        "variables": [],
        "nodes": [
            {"id": "start", "type": "start", "typeVersion": "1.0.0", "config": {}},
            {
                "id": "parallel",
                "type": "parallel",
                "typeVersion": "1.0.0",
                "config": {"join": "all", "failFast": false, "maxConcurrency": 2},
                "branches": [
                    {
                        "id": "left",
                        "graph": {
                            "nodes": [{
                                "id": "left_fetch",
                                "type": "http.request",
                                "typeVersion": "1.0.0",
                                "config": {"method": "GET", "url": format!("http://127.0.0.1:{port}/left")}
                            }],
                            "edges": []
                        }
                    },
                    {
                        "id": "right",
                        "graph": {
                            "nodes": [{
                                "id": "right_fetch",
                                "type": "http.request",
                                "typeVersion": "1.0.0",
                                "config": {"method": "GET", "url": format!("http://127.0.0.1:{port}/right")}
                            }],
                            "edges": []
                        }
                    }
                ]
            },
            {"id": "end", "type": "end", "typeVersion": "1.0.0", "config": {}}
        ],
        "edges": [
            {"id": "e1", "source": "start", "target": "parallel"},
            {"id": "e2", "source": "parallel", "target": "end"}
        ]
    })
    .to_string();
    let prepared = prepare(&text);
    let mut state = start(&prepared, json!({}));
    let started = std::time::Instant::now();
    drive(&prepared, &mut state).await;
    let elapsed = started.elapsed();
    assert_eq!(state.status, ExecutionStatus::Succeeded);
    assert!(elapsed < Duration::from_millis(520), "elapsed: {elapsed:?}");
    let output = state.node_outputs.get("parallel").expect("parallel output");
    assert_eq!(output["branches"]["left"]["status"], json!(200));
    assert_eq!(output["branches"]["right"]["status"], json!(200));
    server.await.expect("server task");
}

#[tokio::test]
async fn parallel_branch_progress_survives_restart() {
    let counter = Arc::new(AtomicUsize::new(0));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind server");
    let port = listener.local_addr().expect("server address").port();
    let server_counter = Arc::clone(&counter);
    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            server_counter.fetch_add(1, Ordering::SeqCst);
            let mut buffer = [0u8; 4096];
            let _ = socket.read(&mut buffer).await;
            let body = r#"{"ok":true}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
        }
    });
    let text = json!({
        "$schema": "https://hpa.dev/schema/ir/v0.1/workflow.schema.json",
        "schemaVersion": "hpa.dev/v1",
        "id": "wf_parallel_recovery",
        "name": "Parallel Recovery",
        "workflowVersion": "1.0.0",
        "inputs": [],
        "outputs": [],
        "variables": [],
        "nodes": [
            {"id": "start", "type": "start", "typeVersion": "1.0.0", "config": {}},
            {
                "id": "parallel",
                "type": "parallel",
                "typeVersion": "1.0.0",
                "config": {"join": "all", "failFast": false, "maxConcurrency": 2},
                "branches": [
                    {
                        "id": "left",
                        "graph": {
                            "nodes": [{
                                "id": "left_fetch",
                                "type": "http.request",
                                "typeVersion": "1.0.0",
                                "config": {"method": "GET", "url": format!("http://127.0.0.1:{port}/left")}
                            }],
                            "edges": []
                        }
                    },
                    {
                        "id": "right",
                        "graph": {
                            "nodes": [{
                                "id": "right_fetch",
                                "type": "http.request",
                                "typeVersion": "1.0.0",
                                "config": {"method": "GET", "url": format!("http://127.0.0.1:{port}/right")}
                            }],
                            "edges": []
                        }
                    }
                ]
            },
            {"id": "end", "type": "end", "typeVersion": "1.0.0", "config": {}}
        ],
        "edges": [
            {"id": "e1", "source": "start", "target": "parallel"},
            {"id": "e2", "source": "parallel", "target": "end"}
        ]
    })
    .to_string();
    let prepared = prepare(&text);
    let mut state = start(&prepared, json!({}));

    let mut observed = false;
    for _ in 0..10 {
        let _ = step(&prepared.workflow, &prepared.plan, &mut state).await;
        let mid_parallel = state
            .frames
            .iter()
            .rev()
            .find_map(|frame| match frame {
                Frame::Parallel(parallel) if !parallel.concurrent_states.is_empty() => {
                    Some(parallel)
                }
                _ => None,
            })
            .map(|parallel| {
                parallel.concurrent_states.iter().all(|branch| {
                    branch.state.status == ExecutionStatus::Running
                        && !branch.state.node_outputs.is_empty()
                })
            });
        if mid_parallel == Some(true) {
            observed = true;
            break;
        }
    }
    assert!(
        observed,
        "expected a mid-parallel snapshot after branch requests"
    );
    assert_eq!(counter.load(Ordering::SeqCst), 2);

    let encoded = serde_json::to_string(&state).expect("serialize execution state");
    let mut resumed: ExecutionState =
        serde_json::from_str(&encoded).expect("restore execution state");
    drive(&prepared, &mut resumed).await;

    assert_eq!(resumed.status, ExecutionStatus::Succeeded);
    assert_eq!(
        counter.load(Ordering::SeqCst),
        2,
        "completed branch requests must not be replayed after restart"
    );
    server.await.expect("server task");
}
