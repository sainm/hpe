use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use hpa_local::ai_worker_client::{AiWorkerConfig, AiWorkerProcess, AiWorkerStart};
use serde_json::{Value, json};

fn worker_environment(approval: bool, delay_ms: u64) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("HPA_AI_PLANNER".to_string(), "deterministic".to_string()),
        (
            "HPA_DETERMINISTIC_TOOL".to_string(),
            "workflow.execute".to_string(),
        ),
        (
            "HPA_DETERMINISTIC_ARGUMENTS".to_string(),
            r#"{"workflowId":"wf_1"}"#.to_string(),
        ),
        (
            "HPA_DETERMINISTIC_OUTPUT".to_string(),
            r#"{"ok":true}"#.to_string(),
        ),
        (
            "HPA_DETERMINISTIC_APPROVAL".to_string(),
            if approval { "1" } else { "0" }.to_string(),
        ),
        (
            "HPA_DETERMINISTIC_TOOL_DELAY_MS".to_string(),
            delay_ms.to_string(),
        ),
        (
            "HPA_DETERMINISTIC_TOOL_RESULTS".to_string(),
            json!({
                "workflow.execute": {
                    "callId": "tc_1",
                    "status": "succeeded",
                    "result": {"executionId": "exec_1"}
                }
            })
            .to_string(),
        ),
    ])
}

fn config(environment: BTreeMap<String, String>) -> AiWorkerConfig {
    AiWorkerConfig {
        executable: PathBuf::from(std::env::var("HPA_AI_WORKER_BIN").expect("HPA_AI_WORKER_BIN")),
        args: Vec::new(),
        request_token: "rust-worker-token".to_string(),
        environment,
    }
}

fn budget() -> Value {
    json!({
        "maxSteps": 4,
        "maxToolCalls": 2,
        "timeoutSeconds": 30,
        "maxTokens": 1000,
        "maxCostUsd": 0.1
    })
}

fn tool_specs(approval: bool) -> Vec<Value> {
    vec![json!({
        "type": "function",
        "function": {
            "name": "workflow.execute",
            "description": "Agent tool workflow.execute",
            "parameters": {
                "type": "object",
                "properties": {"workflowId": {"type": "string"}},
                "required": ["workflowId"]
            },
            "x-hpa": {
                "risk": if approval { "write" } else { "read" },
                "approvalRequired": approval,
                "idempotencyKeyTemplate": "{sessionId}/{step}/{callId}"
            }
        }
    })]
}

#[tokio::test]
async fn rust_client_runs_resumes_and_cancels_python_worker() {
    if std::env::var_os("HPA_AI_WORKER_BIN").is_none() {
        eprintln!("skipped: HPA_AI_WORKER_BIN is not set");
        return;
    }

    let mut completed = AiWorkerProcess::spawn(config(worker_environment(false, 0)))
        .await
        .expect("spawn Python worker");
    let completed_tools = tool_specs(false);
    completed
        .start(AiWorkerStart {
            session_id: "session-rust-complete",
            tenant_id: "tenant-1",
            agent_version_id: "agent-1",
            input: json!({"message": "run"}),
            instructions: Some("Complete the request."),
            tools: &completed_tools,
            budget: budget(),
            tool_token: None,
            model: None,
        })
        .await
        .expect("start worker request");
    let mut completed_types = Vec::new();
    for _ in 0..20 {
        let event = completed
            .next_event(Duration::from_secs(5))
            .await
            .expect("read worker event");
        completed_types.push(event["type"].as_str().unwrap_or_default().to_string());
        if event["type"] == "session.result" {
            assert_eq!(event["data"]["output"]["ok"], true);
            break;
        }
    }
    assert_eq!(
        completed_types.last().map(String::as_str),
        Some("session.result")
    );
    completed.shutdown().await.expect("shutdown worker");

    let mut resumed = AiWorkerProcess::spawn(config(worker_environment(true, 0)))
        .await
        .expect("spawn approval worker");
    let resumed_tools = tool_specs(true);
    resumed
        .start(AiWorkerStart {
            session_id: "session-rust-resume",
            tenant_id: "tenant-1",
            agent_version_id: "agent-1",
            input: json!({"message": "run"}),
            instructions: None,
            tools: &resumed_tools,
            budget: budget(),
            tool_token: Some("tool-token"),
            model: None,
        })
        .await
        .expect("start approval request");
    let paused = resumed
        .next_event(Duration::from_secs(5))
        .await
        .expect("read worker start");
    assert_eq!(paused["type"], "session.started");
    let mut approval_seen = false;
    for _ in 0..10 {
        let event = resumed
            .next_event(Duration::from_secs(5))
            .await
            .expect("read approval event");
        if event["type"] == "approval.required" {
            approval_seen = true;
            break;
        }
    }
    assert!(approval_seen);
    resumed
        .resume(
            "session-rust-resume",
            json!({"approved": true, "approver": "human-1"}),
            Some(budget()),
            None,
        )
        .await
        .expect("resume worker");
    let mut completed_after_resume = false;
    for _ in 0..20 {
        let event = resumed
            .next_event(Duration::from_secs(5))
            .await
            .expect("read resumed worker event");
        if event["type"] == "session.result" {
            completed_after_resume = true;
            break;
        }
    }
    assert!(completed_after_resume);
    resumed.shutdown().await.expect("shutdown resumed worker");

    let mut cancelled = AiWorkerProcess::spawn(config(worker_environment(false, 2_000)))
        .await
        .expect("spawn cancellable worker");
    let cancelled_tools = tool_specs(false);
    cancelled
        .start(AiWorkerStart {
            session_id: "session-rust-cancel",
            tenant_id: "tenant-1",
            agent_version_id: "agent-1",
            input: json!({"message": "run"}),
            instructions: None,
            tools: &cancelled_tools,
            budget: budget(),
            tool_token: None,
            model: None,
        })
        .await
        .expect("start cancellable request");
    tokio::time::sleep(Duration::from_millis(100)).await;
    cancelled
        .cancel("session-rust-cancel", "operator cancelled")
        .await
        .expect("cancel worker");
    let mut cancellation_events = Vec::new();
    for _ in 0..20 {
        let event = cancelled
            .next_event(Duration::from_secs(5))
            .await
            .expect("read cancellation event");
        cancellation_events.push(event["type"].as_str().unwrap_or_default().to_string());
        if event["type"] == "session.cancelled" {
            break;
        }
    }
    assert!(
        cancellation_events
            .iter()
            .any(|kind| kind == "session.cancelled")
    );
    cancelled
        .shutdown()
        .await
        .expect("shutdown cancelled worker");
}
