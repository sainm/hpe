use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use reqwest::Client;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations/postgres");

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hpa")
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("address")
        .port()
}

struct ServerGuard {
    port: u16,
    data_dir: PathBuf,
    child: Option<Child>,
}

impl ServerGuard {
    fn start(data_dir: &Path, postgres_url: &str, node_id: &str, lease_key: &str) -> Self {
        let port = free_port();
        let child = Command::new(binary())
            .arg("--data-dir")
            .arg(data_dir)
            .args(["serve", "--port", &port.to_string()])
            .env("HPA_POSTGRES_SCHEDULER", "1")
            .env("HPA_POSTGRES_URL", postgres_url)
            .env("HPA_NODE_ID", node_id)
            .env("HPA_SCHEDULER_LEASE_KEY", lease_key)
            .env("HPA_SCHEDULER_LEASE_TTL_SECONDS", "2")
            .env("HPA_SCHEDULER_TICK_MS", "100")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn scheduler node");
        Self {
            port,
            data_dir: data_dir.to_path_buf(),
            child: Some(child),
        }
    }

    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
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
    panic!("scheduler node on port {port} did not become healthy");
}

async fn scheduler_is_leader(client: &Client, port: u16) -> bool {
    let response = client
        .get(format!("http://127.0.0.1:{port}/metrics"))
        .send()
        .await
        .expect("scheduler metrics");
    assert!(
        response.status().is_success(),
        "metrics returned {}",
        response.status()
    );
    let metrics = response.text().await.expect("metrics body");
    metrics
        .lines()
        .find_map(|line| {
            line.strip_prefix("hpa_scheduler_leader ")
                .and_then(|value| value.parse::<f64>().ok())
        })
        .is_some_and(|value| value == 1.0)
}

async fn wait_for_single_leader(client: &Client, left: u16, right: u16) -> (bool, bool) {
    for _ in 0..100 {
        let (left_leader, right_leader) = tokio::join!(
            scheduler_is_leader(client, left),
            scheduler_is_leader(client, right)
        );
        if left_leader ^ right_leader {
            return (left_leader, right_leader);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("scheduler nodes {left}/{right} did not converge to one leader");
}

async fn wait_until_leader(client: &Client, port: u16) {
    for _ in 0..100 {
        if scheduler_is_leader(client, port).await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("scheduler node on port {port} did not take over leadership");
}

async fn temporary_directory(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    tokio::fs::create_dir_all(&path)
        .await
        .expect("create scheduler data directory");
    path
}

#[tokio::test]
async fn postgres_lease_elects_one_scheduler_and_fails_over() {
    let Ok(postgres_url) = std::env::var("HPA_POSTGRES_TEST_URL") else {
        eprintln!("skipped: HPA_POSTGRES_TEST_URL is not set");
        return;
    };
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&postgres_url)
        .await
        .expect("connect PostgreSQL");
    MIGRATOR.run(&pool).await.expect("apply migrations");
    pool.close().await;

    let suffix = Uuid::now_v7().simple().to_string();
    let root = std::env::temp_dir().join(format!("hpa-scheduler-leader-{suffix}"));
    let dir_a = temporary_directory(&root, "node-a").await;
    let dir_b = temporary_directory(&root, "node-b").await;
    let lease_key = format!("hpa:scheduler:test:{suffix}");
    let client = Client::builder()
        .no_proxy()
        .build()
        .expect("scheduler test client");

    let node_a = ServerGuard::start(
        &dir_a,
        &postgres_url,
        &format!("scheduler-a-{suffix}"),
        &lease_key,
    );
    let node_b = ServerGuard::start(
        &dir_b,
        &postgres_url,
        &format!("scheduler-b-{suffix}"),
        &lease_key,
    );
    let mut nodes = [node_a, node_b];
    wait_for_health(&client, nodes[0].port).await;
    wait_for_health(&client, nodes[1].port).await;
    let (a_is_leader, _) = wait_for_single_leader(&client, nodes[0].port, nodes[1].port).await;
    let leader_index = if a_is_leader { 0 } else { 1 };
    let follower_index = 1 - leader_index;
    let follower_port = nodes[follower_index].port;
    let leader_dir = nodes[leader_index].data_dir.clone();
    let leader_node_id = format!(
        "scheduler-{}-{suffix}",
        if leader_index == 0 { "a" } else { "b" }
    );

    nodes[leader_index].stop();
    wait_until_leader(&client, follower_port).await;

    let restarted_port = free_port();
    let restarted = Command::new(binary())
        .arg("--data-dir")
        .arg(&leader_dir)
        .args(["serve", "--port", &restarted_port.to_string()])
        .env("HPA_POSTGRES_SCHEDULER", "1")
        .env("HPA_POSTGRES_URL", &postgres_url)
        .env("HPA_NODE_ID", &leader_node_id)
        .env("HPA_SCHEDULER_LEASE_KEY", &lease_key)
        .env("HPA_SCHEDULER_LEASE_TTL_SECONDS", "2")
        .env("HPA_SCHEDULER_TICK_MS", "100")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("restart former scheduler leader");
    let mut restarted = ServerGuard {
        port: restarted_port,
        data_dir: leader_dir,
        child: Some(restarted),
    };
    wait_for_health(&client, restarted.port).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        scheduler_is_leader(&client, follower_port).await,
        "current leader must keep the renewed lease"
    );
    assert!(
        !scheduler_is_leader(&client, restarted.port).await,
        "restarted stale leader must remain a standby"
    );

    restarted.stop();
    nodes[follower_index].stop();
    let _ = std::fs::remove_dir_all(&root);
}
