use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use hpa_local::postgres_agent::{AgentSessionRecord, AgentToolCallRecord, PostgresAgentStore};
use serde_json::{Value, json};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hpa")
}

fn temp_dir(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("hpa-pg-agent-{label}-{}", uuid::Uuid::now_v7()));
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

fn later_rfc3339() -> String {
    (time::OffsetDateTime::now_utc() + time::Duration::minutes(5))
        .format(&time::format_description::well_known::Rfc3339)
        .expect("timestamp")
}

fn now_ms() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp_nanos() as i64 / 1_000_000
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
async fn agent_state_is_authoritative_in_postgres_across_nodes() {
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

    let config: Value = serde_json::from_str(include_str!(
        "../../../fixtures/agents/invoice-auditor.agent.json"
    ))
    .expect("agent fixture");
    let agent_id = config["id"].as_str().expect("agent id").to_string();
    let created = server_a
        .client
        .post(server_a.url("/api/agents"))
        .bearer_auth(&admin)
        .json(&json!({"config": config}))
        .send()
        .await
        .expect("create agent");
    assert_eq!(created.status(), 200);
    let published = server_a
        .client
        .post(server_a.url(&format!("/api/agents/{agent_id}/publish")))
        .bearer_auth(&admin)
        .json(&json!({}))
        .send()
        .await
        .expect("publish agent");
    assert_eq!(published.status(), 200);

    let listed = server_b
        .client
        .get(server_b.url("/api/agents"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("list agents");
    let listed: Value = listed.json().await.expect("agents json");
    assert!(
        listed["agents"]
            .as_array()
            .expect("agents")
            .iter()
            .any(|agent| agent["id"] == json!(agent_id)),
        "node B must see the agent version created on node A"
    );
    let version = server_b
        .client
        .get(server_b.url(&format!("/api/agents/{agent_id}/versions/1.0.0")))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("get version");
    assert_eq!(version.status(), 200);
    let version: Value = version.json().await.expect("version json");
    assert_eq!(version["version"], json!("1.0.0"));
    assert_eq!(version["status"], json!("published"));
    let agent_on_b = server_b
        .client
        .get(server_b.url(&format!("/api/agents/{agent_id}")))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("get agent on B");
    assert_eq!(agent_on_b.status(), 200);
    let agent_on_b: Value = agent_on_b.json().await.expect("agent json");
    assert_eq!(agent_on_b["publishedVersion"]["version"], json!("1.0.0"));
    assert_eq!(agent_on_b["versions"].as_array().map(Vec::len), Some(1));

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .expect("connect postgres");
    let store_a = PostgresAgentStore::new(pool.clone(), "local");
    let store_b = PostgresAgentStore::new(pool.clone(), "local");
    let session_id = format!("agentsess_{}", uuid::Uuid::now_v7());
    let now = now_rfc3339();
    let state = json!({
        "schemaVersion": "hpa.agent.session/v1",
        "agentId": agent_id,
        "agentVersion": "1.0.0",
        "status": "running",
        "input": {},
        "planIndex": 0,
        "planHistory": [],
        "steps": 0,
        "toolCalls": 0,
        "handoffs": 0,
        "depth": 0,
        "tokensIn": 0,
        "tokensOut": 0,
        "costUsd": 0.0,
        "messages": [],
        "lastOutput": null,
        "output": null,
        "pendingAction": null,
        "pendingApproval": null,
        "pendingToolCallId": null,
        "childSessionId": null,
        "stopReason": null,
        "error": null,
        "startedMs": now_ms(),
        "updatedAt": now,
        "eventSequence": 0
    });
    store_a
        .insert_agent_session(&AgentSessionRecord {
            id: session_id.clone(),
            agent_id: agent_id.clone(),
            agent_version: "1.0.0".to_string(),
            status: "running".to_string(),
            input_json: "{}".to_string(),
            output_json: None,
            state_json: state.to_string(),
            error_json: None,
            parent_session_id: None,
            depth: 0,
            created_at: now.clone(),
            updated_at: now.clone(),
            finished_at: None,
        })
        .await
        .expect("insert session");

    let session = server_b
        .client
        .get(server_b.url(&format!("/api/agents/sessions/{session_id}")))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("get session");
    assert_eq!(session.status(), 200);
    let session: Value = session.json().await.expect("session json");
    assert_eq!(session["status"], json!("running"));

    store_a
        .append_agent_events(&[hpa_local::postgres_agent::AgentEventRecord {
            session_id: session_id.clone(),
            sequence: 1,
            at: now.clone(),
            kind: "agent.started".to_string(),
            data_json: "{}".to_string(),
        }])
        .await
        .expect("append event");
    let events = server_b
        .client
        .get(server_b.url(&format!("/api/agents/sessions/{session_id}/events")))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("events");
    let events: Value = events.json().await.expect("events json");
    assert_eq!(events["events"].as_array().map(Vec::len), Some(1));

    let cancelled = server_b
        .client
        .post(server_b.url(&format!("/api/agents/sessions/{session_id}/cancel")))
        .bearer_auth(&admin)
        .json(&json!({}))
        .send()
        .await
        .expect("cancel");
    assert_eq!(cancelled.status(), 200);
    let cancelled: Value = cancelled.json().await.expect("cancelled json");
    assert_eq!(cancelled["status"], json!("cancelled"));
    let after = server_a
        .client
        .get(server_a.url(&format!("/api/agents/sessions/{session_id}")))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("session on A");
    let after: Value = after.json().await.expect("session json");
    assert_eq!(after["status"], json!("cancelled"));

    let resume_config = json!({
        "schemaVersion": "hpa.agent/v1",
        "id": "agent_pg_resume",
        "name": "PG Resume",
        "version": "1.0.0",
        "description": "Responds without tools.",
        "instructions": "Respond directly.",
        "tools": [],
        "planner": {
            "kind": "deterministic",
            "plan": [{"kind": "respond", "output": {"ok": true}}]
        },
        "limits": {
            "maxSteps": 2,
            "maxToolCalls": 1,
            "timeoutMs": 10000,
            "maxTokens": 1000,
            "maxCostUsd": 0.01,
            "maxHandoffs": 0,
            "maxDepth": 1
        },
        "approval": [],
        "subAgents": []
    });
    let resume_agent = "agent_pg_resume".to_string();
    let created = server_a
        .client
        .post(server_a.url("/api/agents"))
        .bearer_auth(&admin)
        .json(&json!({"config": resume_config}))
        .send()
        .await
        .expect("create resume agent");
    assert_eq!(created.status(), 200);
    let published = server_a
        .client
        .post(server_a.url(&format!("/api/agents/{resume_agent}/publish")))
        .bearer_auth(&admin)
        .json(&json!({}))
        .send()
        .await
        .expect("publish resume agent");
    assert_eq!(published.status(), 200);
    let run = server_a
        .client
        .post(server_a.url(&format!("/api/agents/{resume_agent}/run")))
        .bearer_auth(&admin)
        .json(&json!({"input": {"hello": "world"}}))
        .send()
        .await
        .expect("run deterministic agent");
    assert_eq!(run.status(), 200);
    let run: Value = run.json().await.expect("run json");
    assert_eq!(run["status"], json!("succeeded"));
    assert_eq!(run["output"]["ok"], json!(true));
    let run_session_id = run["id"].as_str().expect("run session id").to_string();
    let run_on_b = server_b
        .client
        .get(server_b.url(&format!("/api/agents/sessions/{run_session_id}")))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("run session on B");
    assert_eq!(run_on_b.status(), 200);
    let run_on_b: Value = run_on_b.json().await.expect("run session json");
    assert_eq!(run_on_b["status"], json!("succeeded"));
    assert_eq!(run_on_b["output"]["ok"], json!(true));
    let run_events_b = server_b
        .client
        .get(server_b.url(&format!("/api/agents/sessions/{run_session_id}/events")))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("run events on B");
    assert_eq!(run_events_b.status(), 200);
    let run_events_b: Value = run_events_b.json().await.expect("events json");
    let run_kinds = run_events_b["events"]
        .as_array()
        .expect("events")
        .iter()
        .filter_map(|event| event["kind"].as_str())
        .collect::<Vec<_>>();
    assert!(run_kinds.contains(&"agent.started"));
    assert!(run_kinds.contains(&"agent.finished"));
    let resume_session = format!("agentsess_{}", uuid::Uuid::now_v7());
    let resume_state = json!({
        "schemaVersion": "hpa.agent.session/v1",
        "agentId": resume_agent,
        "agentVersion": "1.0.0",
        "status": "interrupted",
        "input": {},
        "planIndex": 0,
        "planHistory": [],
        "steps": 0,
        "toolCalls": 0,
        "handoffs": 0,
        "depth": 0,
        "tokensIn": 0,
        "tokensOut": 0,
        "costUsd": 0.0,
        "messages": [],
        "lastOutput": null,
        "output": null,
        "pendingAction": null,
        "pendingApproval": null,
        "pendingToolCallId": null,
        "childSessionId": null,
        "stopReason": null,
        "error": null,
        "startedMs": now_ms(),
        "updatedAt": now,
        "eventSequence": 0
    });
    store_a
        .insert_agent_session(&AgentSessionRecord {
            id: resume_session.clone(),
            agent_id: resume_agent,
            agent_version: "1.0.0".to_string(),
            status: "interrupted".to_string(),
            input_json: "{}".to_string(),
            output_json: None,
            state_json: resume_state.to_string(),
            error_json: None,
            parent_session_id: None,
            depth: 0,
            created_at: now.clone(),
            updated_at: now.clone(),
            finished_at: None,
        })
        .await
        .expect("insert resume session");
    let resumed = server_b
        .client
        .post(server_b.url(&format!("/api/agents/sessions/{resume_session}/continue")))
        .bearer_auth(&admin)
        .json(&json!({}))
        .send()
        .await
        .expect("continue session");
    assert_eq!(resumed.status(), 200);
    let resumed: Value = resumed.json().await.expect("resumed json");
    assert_eq!(resumed["status"], json!("succeeded"));
    let resumed_on_a = server_a
        .client
        .get(server_a.url(&format!("/api/agents/sessions/{resume_session}")))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("resumed session on A");
    let resumed_on_a: Value = resumed_on_a.json().await.expect("session json");
    assert_eq!(resumed_on_a["status"], json!("succeeded"));

    let approval = store_a
        .create_agent_approval(
            &session_id,
            "workflow.execute",
            "{\"workflowId\":\"wf\"}",
            "high",
            &now,
            &later_rfc3339(),
        )
        .await
        .expect("create approval");
    let approvals = server_b
        .client
        .get(server_b.url("/api/agents/approvals?status=pending"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("approvals");
    let approvals: Value = approvals.json().await.expect("approvals json");
    assert!(
        approvals["approvals"]
            .as_array()
            .expect("approvals")
            .iter()
            .any(|item| item["id"] == json!(approval.id))
    );
    assert!(
        store_b
            .decide_agent_approval(&approval.id, "approved", &now_rfc3339())
            .await
            .expect("decide approval")
    );
    let decided = store_a
        .get_agent_approval(&approval.id)
        .await
        .expect("get approval")
        .expect("approval exists");
    assert_eq!(decided.status, "approved");

    let tool_call = AgentToolCallRecord {
        session_id: session_id.clone(),
        call_id: "call-1".to_string(),
        idempotency_key: "idem-1".to_string(),
        tool_name: "workflow.execute".to_string(),
        request_hash: "hash".to_string(),
        status: "in_progress".to_string(),
        response_json: None,
        error: None,
        created_at: now.clone(),
        updated_at: now.clone(),
    };
    assert!(
        store_a
            .begin_agent_tool_call(&tool_call)
            .await
            .expect("begin tool call")
    );
    assert!(
        !store_b
            .begin_agent_tool_call(&tool_call)
            .await
            .expect("duplicate tool call")
    );
    assert!(
        store_b
            .finish_agent_tool_call(
                &session_id,
                "call-1",
                "succeeded",
                Some("{\"ok\":true}"),
                None,
                &now_rfc3339(),
            )
            .await
            .expect("finish tool call")
    );
    let stored = store_a
        .get_agent_tool_call(&session_id, "call-1")
        .await
        .expect("get tool call")
        .expect("tool call exists");
    assert_eq!(stored.status, "succeeded");

    let approval_config = json!({
        "schemaVersion": "hpa.agent/v1",
        "id": "agent_pg_approval",
        "name": "PG Approval",
        "version": "1.0.0",
        "tools": ["workflow.list"],
        "planner": {
            "kind": "deterministic",
            "plan": [
                {"kind": "tool", "tool": "workflow.list", "arguments": {}},
                {"kind": "respond", "output": {"approved": true}}
            ]
        },
        "approval": [{"tool": "workflow.list", "mode": "always"}],
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
    let created = server_a
        .client
        .post(server_a.url("/api/agents"))
        .bearer_auth(&admin)
        .json(&json!({"config": approval_config}))
        .send()
        .await
        .expect("create approval agent");
    assert_eq!(created.status(), 200);
    let published = server_a
        .client
        .post(server_a.url("/api/agents/agent_pg_approval/publish"))
        .bearer_auth(&admin)
        .json(&json!({}))
        .send()
        .await
        .expect("publish approval agent");
    assert_eq!(published.status(), 200);
    let waiting = server_a
        .client
        .post(server_a.url("/api/agents/agent_pg_approval/run"))
        .bearer_auth(&admin)
        .json(&json!({"input": {}}))
        .send()
        .await
        .expect("run approval agent");
    assert_eq!(waiting.status(), 200);
    let waiting: Value = waiting.json().await.expect("waiting json");
    assert_eq!(waiting["status"], json!("waiting_approval"));
    let api_approval_id = waiting["pendingApproval"]
        .as_str()
        .expect("pending approval")
        .to_string();
    let api_approval_session = waiting["id"].as_str().expect("session").to_string();
    let pending = server_b
        .client
        .get(server_b.url("/api/agents/approvals?status=pending"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("pending approvals on B");
    let pending: Value = pending.json().await.expect("approvals json");
    assert!(
        pending["approvals"]
            .as_array()
            .expect("approvals")
            .iter()
            .any(|item| item["id"] == json!(api_approval_id))
    );
    let waiting_on_b = server_b
        .client
        .get(server_b.url(&format!("/api/agents/sessions/{api_approval_session}")))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("waiting session on B");
    let waiting_on_b: Value = waiting_on_b.json().await.expect("session json");
    assert_eq!(waiting_on_b["status"], json!("waiting_approval"));
    let decided = server_b
        .client
        .post(server_b.url(&format!("/api/agents/approvals/{api_approval_id}/decide")))
        .bearer_auth(&admin)
        .json(&json!({"decision": "approved"}))
        .send()
        .await
        .expect("decide approval on B");
    assert_eq!(decided.status(), 200);
    let decided: Value = decided.json().await.expect("decided json");
    assert_eq!(decided["approval"]["status"], json!("approved"));
    assert_eq!(decided["session"]["status"], json!("succeeded"));
    assert_eq!(decided["session"]["output"]["approved"], json!(true));
    let resumed_on_a = server_a
        .client
        .get(server_a.url(&format!("/api/agents/sessions/{api_approval_session}")))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("approved session on A");
    let resumed_on_a: Value = resumed_on_a.json().await.expect("session json");
    assert_eq!(resumed_on_a["status"], json!("succeeded"));
    let twice = server_a
        .client
        .post(server_a.url(&format!("/api/agents/approvals/{api_approval_id}/decide")))
        .bearer_auth(&admin)
        .json(&json!({"decision": "approved"}))
        .send()
        .await
        .expect("decide again");
    assert_eq!(twice.status(), 409);

    for dir in [&data_dir_a, &data_dir_b] {
        for table in [
            "agents",
            "agent_versions",
            "agent_sessions",
            "agent_events",
            "agent_approvals",
            "agent_tool_calls",
        ] {
            assert_eq!(
                sqlite_count(&dir.join("hpa.db"), table).await,
                0,
                "{table} must live in PostgreSQL"
            );
        }
    }

    let quota = json!({
        "maxWorkflows": 100,
        "maxExecutionsPerDay": 1000,
        "maxSkills": 100,
        "maxAgents": 1,
        "maxPlugins": 50,
        "maxConnections": 100,
        "maxConcurrentRuns": 8
    });
    let updated = server_a
        .client
        .put(server_a.url("/api/enterprise/quotas/local"))
        .bearer_auth(&admin)
        .json(&quota)
        .send()
        .await
        .expect("update agent quota");
    assert_eq!(updated.status(), 200);
    let blocked = server_a
        .client
        .post(server_a.url("/api/agents"))
        .bearer_auth(&admin)
        .json(&json!({"config": {}}))
        .send()
        .await
        .expect("agent quota");
    assert_eq!(blocked.status(), 429, "PG agent count must enforce quota");

    let mut run_quota = quota.clone();
    run_quota["maxAgents"] = json!(100);
    run_quota["maxExecutionsPerDay"] = json!(1);
    let updated = server_a
        .client
        .put(server_a.url("/api/enterprise/quotas/local"))
        .bearer_auth(&admin)
        .json(&run_quota)
        .send()
        .await
        .expect("update run quota");
    assert_eq!(updated.status(), 200);
    let blocked = server_a
        .client
        .post(server_a.url("/api/agents/agent_pg_resume/run"))
        .bearer_auth(&admin)
        .json(&json!({"input": {}}))
        .send()
        .await
        .expect("agent run quota");
    assert_eq!(
        blocked.status(),
        429,
        "PG agent session count must enforce the daily quota"
    );

    let _ = store_a;
    pool.close().await;
    let _ = std::fs::remove_dir_all(&data_dir_a);
    let _ = std::fs::remove_dir_all(&data_dir_b);
}
