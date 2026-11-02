use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::Value;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hpa")
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("address")
        .port()
}

fn temp_dir() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("hpa-marketplace-events-{}", uuid::Uuid::now_v7()))
}

fn run_cli(data_dir: &Path, args: &[&str]) {
    let output = Command::new(binary())
        .arg("--data-dir")
        .arg(data_dir)
        .args(args)
        .output()
        .expect("run CLI");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

struct Server {
    child: Child,
    base_url: String,
    data_dir: std::path::PathBuf,
}

impl Server {
    async fn start() -> Self {
        let data_dir = temp_dir();
        std::fs::create_dir_all(&data_dir).expect("data dir");
        let port = free_port();
        let mut child = Command::new(binary())
            .arg("--data-dir")
            .arg(&data_dir)
            .args(["serve", "--port", &port.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn server");
        let base_url = format!("http://127.0.0.1:{port}");
        let client = reqwest::Client::new();
        for _ in 0..100 {
            if client
                .get(format!("{base_url}/api/health"))
                .send()
                .await
                .is_ok_and(|response| response.status().is_success())
            {
                return Self {
                    child,
                    base_url,
                    data_dir,
                };
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let _ = child.kill();
        let _ = child.wait();
        panic!("server did not start");
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn next_marketplace_event<S, B>(stream: &mut S) -> Value
where
    S: futures_core::Stream<Item = Result<B, reqwest::Error>> + Unpin,
    B: AsRef<[u8]>,
{
    let mut buffer = String::new();
    loop {
        let chunk = stream
            .next()
            .await
            .expect("marketplace stream ended")
            .expect("marketplace stream error");
        buffer.push_str(&String::from_utf8_lossy(chunk.as_ref()));
        while let Some(boundary) = buffer.find("\n\n") {
            let frame = buffer[..boundary].to_string();
            buffer.drain(..boundary + 2);
            let data = frame
                .lines()
                .filter(|line| line.starts_with("data:"))
                .map(|line| line[5..].trim_start())
                .collect::<Vec<_>>()
                .join("\n");
            if !data.is_empty() {
                return serde_json::from_str(&data).expect("marketplace event JSON");
            }
        }
    }
}

#[tokio::test]
async fn marketplace_sse_resumes_after_withdrawal_capable_event() {
    let server = Server::start().await;
    let client = reqwest::Client::new();
    let response = client
        .get(format!(
            "{}/api/marketplace/events/stream?since=0",
            server.base_url
        ))
        .send()
        .await
        .expect("open marketplace stream");
    assert_eq!(response.status(), 200);
    let mut first_stream = response.bytes_stream();

    let data_dir = server.data_dir.clone();
    let install = tokio::task::spawn_blocking(move || {
        run_cli(&data_dir, &["plugin", "sample"]);
    });
    let first = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = next_marketplace_event(&mut first_stream).await;
            if event["kind"] == "plugin.installed" {
                return event;
            }
        }
    })
    .await
    .expect("first event timeout");
    install.await.expect("install task");
    assert_eq!(first["kind"], "plugin.installed");
    let first_sequence = first["sequence"].as_i64().expect("first sequence");
    drop(first_stream);

    run_cli(
        &server.data_dir,
        &["plugin", "disable", "plugin_sample_kit"],
    );
    let resumed = client
        .get(format!(
            "{}/api/marketplace/events/stream?since={first_sequence}",
            server.base_url
        ))
        .header("last-event-id", first_sequence.to_string())
        .send()
        .await
        .expect("resume marketplace stream");
    let mut resumed_stream = resumed.bytes_stream();
    let next = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = next_marketplace_event(&mut resumed_stream).await;
            assert!(event["sequence"].as_i64().expect("sequence") > first_sequence);
            if event["kind"] == "plugin.disabled" {
                return event;
            }
        }
    })
    .await
    .expect("resume event timeout");

    let latest = client
        .get(format!("{}/api/marketplace/events/latest", server.base_url))
        .send()
        .await
        .expect("latest sequence")
        .json::<Value>()
        .await
        .expect("latest JSON");
    assert!(
        latest["sequence"].as_i64().expect("latest") >= next["sequence"].as_i64().expect("next")
    );
}
