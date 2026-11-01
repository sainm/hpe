use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hpa")
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/skills/invoice-audit")
}

fn run_cli(data_dir: &Path, args: &[&str]) -> String {
    let output = Command::new(binary())
        .arg("--data-dir")
        .arg(data_dir)
        .args(args)
        .output()
        .expect("run hpa cli");
    assert!(
        output.status.success(),
        "cli failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

#[test]
fn stdio_mcp_server_lists_and_calls_skill_tools() {
    let data_dir = std::env::temp_dir().join(format!("hpa-mcp-e2e-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&data_dir).expect("data dir");
    run_cli(
        &data_dir,
        &["skill", "import", fixture().to_str().expect("fixture path")],
    );
    run_cli(
        &data_dir,
        &[
            "skill",
            "publish",
            "skill_invoice_audit",
            "--version",
            "1.0.0",
        ],
    );

    let mut child = Command::new(binary())
        .arg("--data-dir")
        .arg(&data_dir)
        .args(["mcp", "serve"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn mcp server");
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));

    let mut call = |id: u64, method: &str, params: Value| -> Value {
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(stdin, "{request}").expect("write request");
        stdin.flush().expect("flush");
        let mut line = String::new();
        loop {
            line.clear();
            let read = stdout.read_line(&mut line).expect("read response");
            assert!(read > 0, "mcp server closed");
            let response: Value = serde_json::from_str(line.trim()).expect("json response");
            if response.get("id").and_then(Value::as_u64) == Some(id) {
                return response;
            }
        }
    };

    let initialize = call(
        1,
        "initialize",
        json!({"protocolVersion": "2025-11-25", "capabilities": {}}),
    );
    assert_eq!(initialize["result"]["serverInfo"]["name"], "hpa-local");

    let list = call(2, "tools/list", json!({}));
    let tools = list["result"]["tools"].as_array().expect("tools");
    assert!(
        tools
            .iter()
            .any(|tool| tool["name"] == "skill.invoice-audit.audit"),
        "{tools:?}"
    );

    let call_result = call(
        3,
        "tools/call",
        json!({
            "name": "skill.invoice-audit.audit",
            "arguments": {
                "invoiceId": "inv_e2e",
                "items": [{"name": "a", "amount": 12.5}]
            }
        }),
    );
    assert_eq!(call_result["result"]["isError"], false, "{call_result}");
    assert_eq!(
        call_result["result"]["structuredContent"]["total"],
        json!(12.5)
    );

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&data_dir);
}
