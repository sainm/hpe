use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use hpa_local::postgres_agent::{AgentSessionRecord, AgentVersionRecord, PostgresAgentStore};
use serde_json::{Value, json};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hpa")
}

fn temp_dir(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("hpa-pg-lease-{label}-{}", uuid::Uuid::now_v7()));
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

fn running_state(agent_id: &str) -> String {
    json!({
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
        "startedMs": time::OffsetDateTime::now_utc().unix_timestamp() * 1000,
        "updatedAt": now_rfc3339(),
        "eventSequence": 0
    })
    .to_string()
}

struct Server {
    child: Child,
    port: u16,
    client: reqwest::Client,
}

impl Server {
    async fn start(data_dir: &Path, database_url: &str, node_id: &str, ttl_ms: u64) -> Self {
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
            .env("HPA_NODE_ID", node_id)
            .env("HPA_AGENT_LEASE_TTL_MS", ttl_ms.to_string())
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

fn session_row(session_id: &str, agent_id: &str) -> AgentSessionRecord {
    let now = now_rfc3339();
    AgentSessionRecord {
        id: session_id.to_string(),
        agent_id: agent_id.to_string(),
        agent_version: "1.0.0".to_string(),
        status: "running".to_string(),
        input_json: "{}".to_string(),
        output_json: None,
        state_json: running_state(agent_id),
        error_json: None,
        parent_session_id: None,
        depth: 0,
        created_at: now.clone(),
        updated_at: now,
        finished_at: None,
    }
}

#[tokio::test]
async fn agent_session_leases_prevent_cross_node_interruption() {
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

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .expect("connect postgres");
    let store = PostgresAgentStore::new(pool.clone(), "local");
    let agent_id = "lease_agent";
    let now = now_rfc3339();
    store
        .upsert_agent_version(&AgentVersionRecord {
            agent_id: agent_id.to_string(),
            version: "1.0.0".to_string(),
            name: "Lease Agent".to_string(),
            description: None,
            config_json: "{}".to_string(),
            content_hash: "lease-agent-hash".to_string(),
            status: "published".to_string(),
            created_at: now.clone(),
            published_at: Some(now.clone()),
        })
        .await
        .expect("insert agent version");

    let live_session = format!("agentsess_live_{}", uuid::Uuid::now_v7());
    store
        .insert_agent_session(&session_row(&live_session, agent_id))
        .await
        .expect("insert live session");
    let lease = store
        .acquire_agent_lease(&live_session, "node-a", 30_000)
        .await
        .expect("acquire lease")
        .expect("lease acquired");
    assert_eq!(lease.owner, "node-a");
    assert_eq!(lease.token, 1);

    let data_dir_b = temp_dir("b");
    let server_b = Server::start(&data_dir_b, &database_url, "node-b", 300).await;
    let row = store
        .get_agent_session(&live_session)
        .await
        .expect("get live session")
        .expect("live session exists");
    assert_eq!(
        row.status, "running",
        "node B must not interrupt a session whose lease is still held"
    );
    let lease = store
        .agent_lease(&live_session)
        .await
        .expect("read lease")
        .expect("lease exists");
    assert_eq!(lease.owner, "node-a");
    assert_eq!(lease.token, 1);
    assert!(
        store
            .renew_agent_lease(&live_session, "node-a", 1, 30_000)
            .await
            .expect("holder renew")
    );
    assert!(
        !store
            .renew_agent_lease(&live_session, "node-b", 1, 30_000)
            .await
            .expect("foreign renew")
    );
    let events = store
        .list_agent_events(&live_session, 10)
        .await
        .expect("list live events");
    assert!(events.is_empty());

    let mut updated = row.clone();
    updated.updated_at = now_rfc3339();
    assert!(
        store
            .fenced_update_agent_session(&updated, "node-a", 1, 30_000)
            .await
            .expect("holder update")
    );
    assert!(
        !store
            .fenced_update_agent_session(&updated, "node-b", 1, 30_000)
            .await
            .expect("foreign update")
    );
    assert!(
        store
            .acquire_agent_lease(&live_session, "node-b", 300)
            .await
            .expect("foreign acquire")
            .is_none(),
        "a live lease must block another node"
    );

    let expired_session = format!("agentsess_expired_{}", uuid::Uuid::now_v7());
    store
        .insert_agent_session(&session_row(&expired_session, agent_id))
        .await
        .expect("insert expired session");
    assert_eq!(
        store
            .acquire_agent_lease(&expired_session, "node-a", 1)
            .await
            .expect("short lease")
            .expect("short lease acquired")
            .token,
        1
    );
    tokio::time::sleep(Duration::from_millis(80)).await;
    let takeover = store
        .acquire_agent_lease(&expired_session, "node-b", 300)
        .await
        .expect("takeover")
        .expect("expired lease can be taken over");
    assert_eq!(takeover.owner, "node-b");
    assert_eq!(takeover.token, 2);
    assert!(
        !store
            .renew_agent_lease(&expired_session, "node-a", 1, 300)
            .await
            .expect("stale renew")
    );
    assert!(
        store
            .renew_agent_lease(&expired_session, "node-b", 2, 300)
            .await
            .expect("takeover renew")
    );
    let mut expired_row = store
        .get_agent_session(&expired_session)
        .await
        .expect("get expired session")
        .expect("expired session exists");
    expired_row.updated_at = now_rfc3339();
    assert!(
        !store
            .fenced_update_agent_session(&expired_row, "node-a", 1, 300)
            .await
            .expect("stale holder update")
    );
    assert!(
        store
            .fenced_update_agent_session(&expired_row, "node-b", 2, 300)
            .await
            .expect("new holder update")
    );
    store
        .release_agent_lease(&expired_session, "node-b", 2)
        .await
        .expect("release takeover");

    let data_dir_c = temp_dir("c");
    let server_c = Server::start(&data_dir_c, &database_url, "node-c", 300).await;
    let recovered = store
        .get_agent_session(&expired_session)
        .await
        .expect("get recovered session")
        .expect("recovered session exists");
    assert_eq!(recovered.status, "interrupted");
    let recovered_state: Value =
        serde_json::from_str(&recovered.state_json).expect("recovered state");
    assert_eq!(recovered_state["stopReason"], json!("runtime_restart"));
    let events = store
        .list_agent_events(&expired_session, 10)
        .await
        .expect("list recovered events");
    assert!(
        events.iter().any(|event| event.kind == "agent.interrupted"),
        "recover takeover must emit agent.interrupted"
    );
    let live = store
        .get_agent_session(&live_session)
        .await
        .expect("get live session after recovery")
        .expect("live session exists");
    assert_eq!(live.status, "running");
    assert_eq!(
        store
            .agent_lease(&live_session)
            .await
            .expect("read live lease")
            .expect("live lease exists")
            .owner,
        "node-a"
    );

    let agents = server_c
        .client
        .get(server_c.url("/api/agents/sessions"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("list sessions");
    assert_eq!(agents.status(), 200);

    let probe_config = json!({
        "schemaVersion": "hpa.agent/v1",
        "id": "agent_lease_probe",
        "name": "Lease Probe",
        "version": "1.0.0",
        "description": "Deterministic lease release probe",
        "instructions": "List workflows, then reply.",
        "tools": ["workflow.list"],
        "planner": {
            "kind": "deterministic",
            "plan": [
                {"kind": "tool", "tool": "workflow.list", "arguments": {}},
                {"kind": "respond", "output": {"ok": true}}
            ]
        },
        "limits": {
            "maxSteps": 2,
            "maxToolCalls": 1,
            "timeoutMs": 60000,
            "maxTokens": 100,
            "maxCostUsd": 0.01,
            "maxHandoffs": 1,
            "maxDepth": 1
        },
        "approval": [],
        "subAgents": []
    });
    let created = server_c
        .client
        .post(server_c.url("/api/agents"))
        .bearer_auth(&admin)
        .json(&json!({"config": probe_config}))
        .send()
        .await
        .expect("create probe agent");
    let status = created.status();
    let body = created.text().await.expect("create body");
    assert_eq!(status, 200, "create probe failed: {body}");
    let published = server_c
        .client
        .post(server_c.url("/api/agents/agent_lease_probe/publish"))
        .bearer_auth(&admin)
        .json(&json!({}))
        .send()
        .await
        .expect("publish probe agent");
    assert_eq!(published.status(), 200);
    let ran = server_c
        .client
        .post(server_c.url("/api/agents/agent_lease_probe/run"))
        .bearer_auth(&admin)
        .json(&json!({"input": {}}))
        .send()
        .await
        .expect("run probe agent");
    let status = ran.status();
    let body = ran.text().await.expect("run body");
    assert_eq!(status, 200, "probe run failed: {body}");
    let ran: Value = serde_json::from_str(&body).expect("run json");
    let probe_session = ran["id"].as_str().expect("probe session id").to_string();
    assert!(
        store
            .agent_lease(&probe_session)
            .await
            .expect("probe lease")
            .is_none(),
        "a completed agent run must release its lease"
    );
    let _ = server_b;

    let mut cancelled = live.clone();
    cancelled.status = "cancelled".to_string();
    cancelled.updated_at = now_rfc3339();
    store
        .update_agent_session(&cancelled)
        .await
        .expect("cancel session");
    assert!(
        !store
            .fenced_update_agent_session(&cancelled, "node-a", 1, 30_000)
            .await
            .expect("terminal update"),
        "a terminal session must reject late fenced writes"
    );

    for dir in [&data_dir_b, &data_dir_c] {
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

    pool.close().await;
    let _ = std::fs::remove_dir_all(&data_dir_a);
    let _ = std::fs::remove_dir_all(&data_dir_b);
    let _ = std::fs::remove_dir_all(&data_dir_c);
}
