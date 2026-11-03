use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hpa")
}

fn temp_dir(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("hpa-pg-skill-{label}-{}", uuid::Uuid::now_v7()));
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
async fn skill_metadata_is_authoritative_in_postgres_across_nodes() {
    let Ok(database_url) = std::env::var("HPA_POSTGRES_TEST_URL") else {
        eprintln!("skipped: HPA_POSTGRES_TEST_URL is not set");
        return;
    };
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/skills/invoice-audit")
        .canonicalize()
        .expect("skill fixture");
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

    let imported = server_a
        .client
        .post(server_a.url("/api/skills/import"))
        .bearer_auth(&admin)
        .json(&json!({"path": fixture.display().to_string()}))
        .send()
        .await
        .expect("import skill");
    assert_eq!(imported.status(), 200);
    let imported: Value = imported.json().await.expect("import json");
    let skill_id = imported["skill"]["skill"]["id"]
        .as_str()
        .expect("skill id")
        .to_string();
    let version = imported["skill"]["skill"]["version"]
        .as_str()
        .expect("skill version")
        .to_string();

    let listed = server_b
        .client
        .get(server_b.url("/api/skills"))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("list skills");
    let listed: Value = listed.json().await.expect("skills json");
    assert!(
        listed["skills"]
            .as_array()
            .expect("skills")
            .iter()
            .any(|skill| skill["id"] == json!(skill_id)),
        "node B must see the skill imported on node A"
    );
    let version_response = server_b
        .client
        .get(server_b.url(&format!("/api/skills/{skill_id}/versions/{version}")))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("get version");
    assert_eq!(version_response.status(), 200);
    let source = server_b
        .client
        .get(server_b.url(&format!("/api/skills/{skill_id}/versions/{version}/source")))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("get source");
    assert_eq!(source.status(), 200);
    let source: Value = source.json().await.expect("source json");
    assert!(
        source["files"]["scripts/audit.py"].is_string(),
        "skill files must be readable on node B"
    );

    let signed = server_a
        .client
        .post(server_a.url(&format!("/api/skills/{skill_id}/versions/{version}/sign")))
        .bearer_auth(&admin)
        .json(&json!({}))
        .send()
        .await
        .expect("sign skill");
    assert_eq!(signed.status(), 200);
    let published = server_a
        .client
        .post(server_a.url(&format!(
            "/api/skills/{skill_id}/versions/{version}/publish"
        )))
        .bearer_auth(&admin)
        .json(&json!({}))
        .send()
        .await
        .expect("publish skill");
    assert_eq!(published.status(), 200);
    let version_response = server_b
        .client
        .get(server_b.url(&format!("/api/skills/{skill_id}/versions/{version}")))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("get published version");
    let version_response: Value = version_response.json().await.expect("version json");
    assert_eq!(version_response["skill"]["status"], json!("published"));

    let tested = server_b
        .client
        .post(server_b.url(&format!("/api/skills/{skill_id}/versions/{version}/tests")))
        .bearer_auth(&admin)
        .json(&json!({}))
        .send()
        .await
        .expect("test skill");
    assert_eq!(tested.status(), 200);
    let tested: Value = tested.json().await.expect("test json");
    assert_eq!(
        tested["passed"],
        json!(true),
        "skill test must pass on node B"
    );

    let ran = server_b
        .client
        .post(server_b.url(&format!("/api/skills/{skill_id}/versions/{version}/run")))
        .bearer_auth(&admin)
        .json(&json!({
            "script": "audit",
            "input": {
                "invoiceId": "inv_1001",
                "items": [
                    {"name": "license", "amount": 120.5},
                    {"name": "support", "amount": 79.5}
                ]
            }
        }))
        .send()
        .await
        .expect("run skill");
    assert_eq!(ran.status(), 200);
    let ran: Value = ran.json().await.expect("run json");
    let run_id = ran["run"]["id"].as_str().expect("run id").to_string();

    let runs = server_a
        .client
        .get(server_a.url(&format!("/api/skills/{skill_id}/runs")))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("list runs");
    let status = runs.status();
    let body = runs.text().await.expect("runs body");
    assert_eq!(status, 200, "list runs failed: {body}");
    let runs: Value = serde_json::from_str(&body).expect("runs json");
    assert!(
        runs["runs"]
            .as_array()
            .expect("runs")
            .iter()
            .any(|run| run["id"] == json!(run_id)),
        "node A must see the run created on node B"
    );
    let run_detail = server_b
        .client
        .get(server_b.url(&format!("/api/skill-runs/{run_id}")))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("get run");
    assert_eq!(run_detail.status(), 200);
    let artifacts = server_a
        .client
        .get(server_a.url(&format!("/api/skill-runs/{run_id}/artifacts")))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("list artifacts");
    let artifacts: Value = artifacts.json().await.expect("artifacts json");
    assert!(
        artifacts["artifacts"]
            .as_array()
            .expect("artifacts")
            .iter()
            .any(|artifact| artifact["name"] == json!("summary.json")),
        "run artifacts must be visible across nodes"
    );

    for dir in [&data_dir_a, &data_dir_b] {
        for table in [
            "skills",
            "skill_versions",
            "skill_files",
            "skill_runs",
            "skill_run_artifacts",
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
        "maxSkills": 1,
        "maxAgents": 100,
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
        .expect("update skill quota");
    assert_eq!(updated.status(), 200);
    let blocked = server_a
        .client
        .post(server_a.url("/api/skills/import"))
        .bearer_auth(&admin)
        .json(&json!({"path": fixture.display().to_string()}))
        .send()
        .await
        .expect("skill quota");
    assert_eq!(blocked.status(), 429, "PG skill count must enforce quota");

    let _ = std::fs::remove_dir_all(&data_dir_a);
    let _ = std::fs::remove_dir_all(&data_dir_b);
}
