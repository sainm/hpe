use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hpa")
}

fn temp_dir() -> PathBuf {
    let path = std::env::temp_dir().join(format!("hpa-recovery-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&path).expect("temp dir");
    path
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("address")
        .port()
}

struct Server {
    child: Child,
    port: u16,
    client: reqwest::Client,
}

impl Server {
    async fn start(data_dir: &Path) -> Self {
        let port = free_port();
        let child = Command::new(binary())
            .arg("--data-dir")
            .arg(data_dir)
            .args(["serve", "--port", &port.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn server");
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("client");
        let mut server = Self {
            child,
            port,
            client,
        };
        for _ in 0..400 {
            if let Ok(response) = server.client.get(server.url("/api/health")).send().await
                && response.status().is_success()
            {
                return server;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        server.stop();
        panic!("server did not become healthy");
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn workflow(port: u16) -> Value {
    json!({
        "$schema": "https://hpa.dev/schema/ir/v0.1/workflow.schema.json",
        "schemaVersion": "hpa.dev/v1",
        "id": "wf_crash_recovery",
        "name": "Crash recovery",
        "workflowVersion": "1.0.0",
        "nodes": [
            {"id": "start", "type": "start", "typeVersion": "1.0.0", "config": {}},
            {
                "id": "fetch",
                "type": "http.request",
                "typeVersion": "1.0.0",
                "config": {
                    "method": "GET",
                    "url": format!("http://127.0.0.1:{port}/slow")
                },
                "timeoutMs": 5000
            },
            {"id": "end", "type": "end", "typeVersion": "1.0.0", "config": {}}
        ],
        "edges": [
            {"id": "e1", "source": "start", "target": "fetch"},
            {"id": "e2", "source": "fetch", "target": "end"}
        ]
    })
}

async fn slow_http_server() -> (u16, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let port = listener.local_addr().expect("mock address").port();
    let requests = Arc::new(AtomicUsize::new(0));
    let count = requests.clone();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            count.fetch_add(1, Ordering::SeqCst);
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request).await;
            tokio::time::sleep(Duration::from_millis(350)).await;
            let body = r#"{"recovered":true}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        }
    });
    (port, requests, handle)
}

#[tokio::test]
async fn forcefully_killed_server_recovers_running_workflow_from_checkpoint() {
    let data_dir = temp_dir();
    let (mock_port, requests, mock) = slow_http_server().await;
    let mut server = Server::start(&data_dir).await;
    let openapi: Value = server
        .client
        .get(server.url("/api/openapi.json"))
        .send()
        .await
        .expect("openapi")
        .json()
        .await
        .expect("openapi JSON");
    assert_eq!(openapi["openapi"], "3.1.0");
    let yaml = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/01_sequence.yaml"),
    )
    .expect("YAML fixture");
    let validation: Value = server
        .client
        .post(server.url("/api/workflows/validate"))
        .json(&json!({"format": "yaml", "content": yaml}))
        .send()
        .await
        .expect("validate")
        .json()
        .await
        .expect("validation JSON");
    assert_eq!(validation["valid"], true);
    assert!(
        validation["sourceMap"]["entries"]
            .as_array()
            .is_some_and(|entries| entries.len() > 10)
    );

    let created = server
        .client
        .post(server.url("/api/workflows"))
        .json(&json!({"ir": workflow(mock_port)}))
        .send()
        .await
        .expect("create workflow");
    assert_eq!(created.status(), 200, "{}", created.text().await.unwrap());
    let published = server
        .client
        .post(server.url("/api/workflows/wf_crash_recovery/publish"))
        .send()
        .await
        .expect("publish");
    assert_eq!(
        published.status(),
        200,
        "{}",
        published.text().await.unwrap()
    );
    let started: Value = server
        .client
        .post(server.url("/api/workflows/wf_crash_recovery/executions"))
        .json(&json!({"input": {}}))
        .send()
        .await
        .expect("start execution")
        .json()
        .await
        .expect("execution JSON");
    let execution_id = started["summary"]["id"]
        .as_str()
        .expect("execution id")
        .to_string();

    for _ in 0..50 {
        if requests.load(Ordering::SeqCst) > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        requests.load(Ordering::SeqCst),
        1,
        "the HTTP node must be in flight before the forced kill"
    );
    server.stop();
    drop(server);

    let server = Server::start(&data_dir).await;
    let mut status = String::new();
    for _ in 0..200 {
        let response: Value = server
            .client
            .get(server.url(&format!("/api/executions/{execution_id}")))
            .send()
            .await
            .expect("read execution")
            .json()
            .await
            .expect("execution JSON");
        status = response["summary"]["status"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if status == "succeeded" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(status, "succeeded");
    assert!(
        requests.load(Ordering::SeqCst) >= 2,
        "the interrupted HTTP attempt must be retried after recovery"
    );
    mock.abort();
}
