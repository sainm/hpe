use std::fs;
use std::path::{Path, PathBuf};

use hpa_skill::{
    RunStatus, RunnerConfig, SandboxKind, SkillError, SkillInspection, SkillRunRequest, inspect,
    run_script,
};
use serde_json::{Value, json};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/skills")
}

fn temp_root(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("hpa-runner-{label}-{}", uuid::Uuid::now_v7()));
    fs::create_dir_all(&path).expect("temp root");
    path
}

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent");
    }
    fs::write(path, content).expect("write");
}

fn invoice_runner(root: &Path) -> RunnerConfig {
    RunnerConfig {
        work_root: root.join("work"),
        artifact_store: root.join("artifacts"),
        audit_log: Some(root.join("logs/audit.jsonl")),
    }
}

fn invoice_request<'a>(inspection: &'a SkillInspection, input: Value) -> SkillRunRequest<'a> {
    SkillRunRequest {
        snapshot: &inspection.snapshot,
        entry: inspection.entry,
        permissions: &inspection.permissions,
        scripts: &inspection.scripts,
        script_id: "audit".to_string(),
        input,
    }
}

fn custom_skill(
    root: &Path,
    name: &str,
    runtime: &str,
    extension: &str,
    script: &str,
    timeout_ms: u64,
    output_bytes: u64,
) -> SkillInspection {
    let skill = root.join(name);
    write(
        &skill.join("SKILL.md"),
        &format!(
            "---\nname: {name}\ndescription: Sandbox test skill.\nlicense: MIT\n---\n\n# Test\n\nRun the entry.\n"
        ),
    );
    write(&skill.join(format!("scripts/run.{extension}")), script);
    let manifest = json!({
        "schemaVersion": "hpa.skill/v1",
        "id": format!("skill_{}", name.replace('-', "_")),
        "name": name,
        "version": "1.0.0",
        "license": "MIT",
        "entry": {"kind": "script"},
        "scripts": [{
            "id": "run",
            "path": format!("scripts/run.{extension}"),
            "runtime": runtime,
            "args": [],
            "timeoutMs": timeout_ms,
            "network": "none"
        }],
        "permissions": {
            "network": [],
            "commands": [runtime],
            "env": [],
            "resources": {"cpu": 10, "memoryMb": 256, "maxOutputBytes": output_bytes}
        },
        "tests": [{
            "name": "smoke",
            "script": "run",
            "input": {},
            "expect": {"exitCode": 0}
        }]
    });
    write(&skill.join("hpa.skill.json"), &manifest.to_string());
    let inspection = inspect(&skill).expect("inspect custom skill");
    assert!(
        inspection.report.is_valid(),
        "custom skill issues: {:?}",
        inspection.report.issues
    );
    inspection
}

#[tokio::test]
async fn deterministic_script_runs_in_sandbox_and_writes_artifact() {
    let root = temp_root("invoice");
    let inspection = inspect(&fixtures().join("invoice-audit")).expect("inspect fixture");
    let outcome = run_script(
        &invoice_runner(&root),
        invoice_request(
            &inspection,
            json!({
                "invoiceId": "inv_1001",
                "items": [{"name": "license", "amount": 120.5}, {"name": "support", "amount": 79.5}]
            }),
        ),
    )
    .await
    .expect("run script");

    assert_eq!(outcome.status, RunStatus::Succeeded, "{:?}", outcome.error);
    assert_eq!(outcome.output.as_ref().unwrap()["total"], json!(200.0));
    assert_eq!(outcome.artifacts.len(), 1);
    assert_eq!(outcome.artifacts[0].name, "summary.json");
    assert!(outcome.artifacts[0].stored_path.is_file());
    assert!(!outcome.started_at.is_empty());
    #[cfg(target_os = "macos")]
    assert_eq!(outcome.sandbox, SandboxKind::MacosSeatbelt);
    let audit = fs::read_to_string(root.join("logs/audit.jsonl")).expect("audit log");
    assert!(audit.contains("\"action\":\"skill.run\""));
    assert!(audit.contains("\"status\":\"succeeded\""));
    let work_entries = fs::read_dir(root.join("work")).expect("work root").count();
    assert_eq!(work_entries, 0, "work directory should be cleaned");
    fs::remove_dir_all(&root).expect("cleanup");
}

#[tokio::test]
async fn input_schema_is_enforced_before_execution() {
    let root = temp_root("input-schema");
    let inspection = inspect(&fixtures().join("invoice-audit")).expect("inspect fixture");
    let error = run_script(
        &invoice_runner(&root),
        invoice_request(&inspection, json!({"invoiceId": "inv_1"})),
    )
    .await
    .expect_err("input should fail");
    assert!(
        matches!(&error, SkillError::Invalid { code, .. } if code == "SKILL_INPUT_INVALID"),
        "{error:?}"
    );
    fs::remove_dir_all(&root).expect("cleanup");
}

#[tokio::test]
async fn timeout_kills_the_script() {
    let root = temp_root("timeout");
    let inspection = custom_skill(
        &root,
        "sleepy-skill",
        "python3",
        "py",
        "import time\ntime.sleep(5)\nprint('{}')\n",
        300,
        65536,
    );
    let config = RunnerConfig {
        work_root: root.join("work"),
        artifact_store: root.join("artifacts"),
        audit_log: None,
    };
    let outcome = run_script(
        &config,
        SkillRunRequest {
            snapshot: &inspection.snapshot,
            entry: inspection.entry,
            permissions: &inspection.permissions,
            scripts: &inspection.scripts,
            script_id: "run".to_string(),
            input: json!({}),
        },
    )
    .await
    .expect("run");
    assert_eq!(outcome.status, RunStatus::TimedOut);
    assert_eq!(outcome.error.as_ref().unwrap().code, "SCRIPT_TIMEOUT");
    assert!(
        outcome.duration_ms < 3000,
        "duration {}",
        outcome.duration_ms
    );
    fs::remove_dir_all(&root).expect("cleanup");
}

#[tokio::test]
async fn output_limit_is_enforced() {
    let root = temp_root("output-limit");
    let inspection = custom_skill(
        &root,
        "noisy-skill",
        "python3",
        "py",
        "print('{\"ok\": true}' + 'x' * 8192)\n",
        5000,
        1024,
    );
    let config = RunnerConfig {
        work_root: root.join("work"),
        artifact_store: root.join("artifacts"),
        audit_log: None,
    };
    let outcome = run_script(
        &config,
        SkillRunRequest {
            snapshot: &inspection.snapshot,
            entry: inspection.entry,
            permissions: &inspection.permissions,
            scripts: &inspection.scripts,
            script_id: "run".to_string(),
            input: json!({}),
        },
    )
    .await
    .expect("run");
    assert_eq!(outcome.status, RunStatus::Failed);
    assert_eq!(outcome.error.as_ref().unwrap().code, "SCRIPT_OUTPUT_LIMIT");
    assert!(outcome.stdout.len() <= 1024);
    fs::remove_dir_all(&root).expect("cleanup");
}

#[tokio::test]
async fn shell_runtime_executes_without_a_shell_wrapper() {
    let root = temp_root("shell");
    let inspection = custom_skill(
        &root,
        "shell-skill",
        "bash",
        "sh",
        "printf '{\"ok\": true}\\n'\n",
        5000,
        65536,
    );
    let config = RunnerConfig {
        work_root: root.join("work"),
        artifact_store: root.join("artifacts"),
        audit_log: None,
    };
    let outcome = run_script(
        &config,
        SkillRunRequest {
            snapshot: &inspection.snapshot,
            entry: inspection.entry,
            permissions: &inspection.permissions,
            scripts: &inspection.scripts,
            script_id: "run".to_string(),
            input: json!({}),
        },
    )
    .await
    .expect("run");
    assert_eq!(outcome.status, RunStatus::Succeeded, "{:?}", outcome.error);
    assert_eq!(outcome.output.unwrap()["ok"], json!(true));
    fs::remove_dir_all(&root).expect("cleanup");
}

#[tokio::test]
async fn node_runtime_executes_deterministic_script() {
    let root = temp_root("node");
    let inspection = custom_skill(
        &root,
        "node-skill",
        "node",
        "js",
        "console.log(JSON.stringify({ok: true, runtime: 'node'}));\n",
        5000,
        65536,
    );
    let config = RunnerConfig {
        work_root: root.join("work"),
        artifact_store: root.join("artifacts"),
        audit_log: None,
    };
    let outcome = run_script(
        &config,
        SkillRunRequest {
            snapshot: &inspection.snapshot,
            entry: inspection.entry,
            permissions: &inspection.permissions,
            scripts: &inspection.scripts,
            script_id: "run".to_string(),
            input: json!({}),
        },
    )
    .await
    .expect("run");
    assert_eq!(outcome.status, RunStatus::Succeeded, "{:?}", outcome.error);
    assert_eq!(outcome.output.unwrap()["runtime"], json!("node"));
    fs::remove_dir_all(&root).expect("cleanup");
}

#[tokio::test]
async fn runtime_outside_command_whitelist_is_denied() {
    let root = temp_root("commands");
    let skill = root.join("command-skill");
    write(
        &skill.join("SKILL.md"),
        "---\nname: command-skill\ndescription: Command whitelist test.\nlicense: MIT\n---\n\n# Test\n",
    );
    write(
        &skill.join("scripts/run.sh"),
        "printf '{\"ok\": true}\\n'\n",
    );
    write(
        &skill.join("hpa.skill.json"),
        &json!({
            "schemaVersion": "hpa.skill/v1",
            "id": "skill_command_skill",
            "name": "command-skill",
            "version": "1.0.0",
            "license": "MIT",
            "entry": {"kind": "script"},
            "scripts": [{
                "id": "run",
                "path": "scripts/run.sh",
                "runtime": "bash",
                "args": [],
                "network": "none"
            }],
            "permissions": {"network": [], "commands": ["python3"]},
            "tests": [{"name": "smoke", "script": "run", "input": {}, "expect": {"exitCode": 0}}]
        })
        .to_string(),
    );
    let inspection = inspect(&skill).expect("inspect");
    assert!(
        inspection.report.is_valid(),
        "{:?}",
        inspection.report.issues
    );
    let config = RunnerConfig {
        work_root: root.join("work"),
        artifact_store: root.join("artifacts"),
        audit_log: None,
    };
    let error = run_script(
        &config,
        SkillRunRequest {
            snapshot: &inspection.snapshot,
            entry: inspection.entry,
            permissions: &inspection.permissions,
            scripts: &inspection.scripts,
            script_id: "run".to_string(),
            input: json!({}),
        },
    )
    .await
    .expect_err("command must be denied");
    assert!(matches!(error, SkillError::Permission(_)), "{error:?}");
    fs::remove_dir_all(&root).expect("cleanup");
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn seatbelt_denies_network_access() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let port = listener.local_addr().expect("address").port();
    drop(listener);

    let root = temp_root("network");
    let script = format!(
        "import json, socket\ntry:\n    sock = socket.create_connection((\"127.0.0.1\", {port}), timeout=1)\n    sock.close()\n    print(json.dumps({{\"connected\": True}}))\nexcept Exception as exc:\n    print(json.dumps({{\"connected\": False, \"error\": type(exc).__name__ + \": \" + str(exc)}}))\n    raise SystemExit(7)\n"
    );
    let inspection = custom_skill(
        &root,
        "network-skill",
        "python3",
        "py",
        &script,
        5000,
        65536,
    );
    let config = RunnerConfig {
        work_root: root.join("work"),
        artifact_store: root.join("artifacts"),
        audit_log: None,
    };
    let outcome = run_script(
        &config,
        SkillRunRequest {
            snapshot: &inspection.snapshot,
            entry: inspection.entry,
            permissions: &inspection.permissions,
            scripts: &inspection.scripts,
            script_id: "run".to_string(),
            input: json!({}),
        },
    )
    .await
    .expect("run");
    assert_eq!(outcome.status, RunStatus::Failed);
    let output = outcome.output.clone().unwrap_or_else(|| {
        panic!(
            "missing json output; exit={:?} stdout={} stderr={} error={:?}",
            outcome.exit_code, outcome.stdout, outcome.stderr, outcome.error
        )
    });
    assert_eq!(output["connected"], json!(false), "{output}");
    let message = output["error"].as_str().unwrap_or_default();
    assert!(
        message.contains("not permitted") || message.contains("Permission"),
        "{message}"
    );
    fs::remove_dir_all(&root).expect("cleanup");
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn seatbelt_denies_writes_outside_workspace() {
    let escape =
        PathBuf::from("/Users/sainm").join(format!("hpa-sandbox-escape-{}", uuid::Uuid::now_v7()));
    let root = temp_root("file-write");
    let script = format!(
        "import json\ntarget = {:?}\ntry:\n    open(target, \"w\", encoding=\"utf-8\").write(\"escaped\")\n    print(json.dumps({{\"wrote\": True}}))\nexcept Exception as exc:\n    print(json.dumps({{\"wrote\": False, \"error\": type(exc).__name__ + \": \" + str(exc)}}))\n",
        escape.display().to_string()
    );
    let inspection = custom_skill(&root, "write-skill", "python3", "py", &script, 5000, 65536);
    let config = RunnerConfig {
        work_root: root.join("work"),
        artifact_store: root.join("artifacts"),
        audit_log: None,
    };
    let outcome = run_script(
        &config,
        SkillRunRequest {
            snapshot: &inspection.snapshot,
            entry: inspection.entry,
            permissions: &inspection.permissions,
            scripts: &inspection.scripts,
            script_id: "run".to_string(),
            input: json!({}),
        },
    )
    .await
    .expect("run");
    assert_eq!(
        outcome.status,
        RunStatus::Succeeded,
        "exit={:?} stdout={} stderr={} error={:?}",
        outcome.exit_code,
        outcome.stdout,
        outcome.stderr,
        outcome.error
    );
    let output = outcome.output.expect("json output");
    assert_eq!(output["wrote"], json!(false), "{output}");
    assert!(
        !escape.exists(),
        "sandbox escape wrote {}",
        escape.display()
    );
    fs::remove_dir_all(&root).expect("cleanup");
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn seatbelt_denies_reading_user_home() {
    let root = temp_root("file-read");
    let marker = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(format!(".hpa-sandbox-read-{}", uuid::Uuid::now_v7()));
    fs::write(&marker, "host-only").expect("write marker");
    let marker_path = marker.display().to_string();
    let script = format!(
        "import json\npath = {marker_path:?}\ntry:\n    open(path, \"r\", encoding=\"utf-8\").read(16)\n    print(json.dumps({{\"read\": True}}))\nexcept Exception as exc:\n    print(json.dumps({{\"read\": False, \"error\": type(exc).__name__ + \": \" + str(exc)}}))\n"
    );
    let inspection = custom_skill(&root, "read-skill", "python3", "py", &script, 5000, 65536);
    let config = RunnerConfig {
        work_root: root.join("work"),
        artifact_store: root.join("artifacts"),
        audit_log: None,
    };
    let outcome = run_script(
        &config,
        SkillRunRequest {
            snapshot: &inspection.snapshot,
            entry: inspection.entry,
            permissions: &inspection.permissions,
            scripts: &inspection.scripts,
            script_id: "run".to_string(),
            input: json!({}),
        },
    )
    .await
    .expect("run");
    assert_eq!(outcome.status, RunStatus::Succeeded);
    let output = outcome.output.expect("json output");
    assert_eq!(output["read"], json!(false), "{output}");
    fs::remove_file(&marker).ok();
    fs::remove_dir_all(&root).expect("cleanup");
}
