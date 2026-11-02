use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::{Value, json};
use uuid::Uuid;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hpa")
}

fn temp_dir(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("hpa-connector-{label}-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&path).expect("temporary directory");
    path
}

fn run_cli(data_dir: &Path, args: &[&str], environment: &[(&str, &str)]) -> std::process::Output {
    let mut command = Command::new(binary());
    command.arg("--data-dir").arg(data_dir).args(args);
    for (key, value) in environment {
        command.env(key, value);
    }
    command.output().expect("run connector CLI")
}

fn require_cli(data_dir: &Path, args: &[&str], environment: &[(&str, &str)]) -> Value {
    let output = run_cli(data_dir, args, environment);
    assert!(
        output.status.success(),
        "connector CLI {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("connector CLI JSON")
}

fn failure_cli(data_dir: &Path, args: &[&str], environment: &[(&str, &str)]) -> String {
    let output = run_cli(data_dir, args, environment);
    assert!(
        !output.status.success(),
        "connector CLI {args:?} unexpectedly succeeded"
    );
    String::from_utf8_lossy(&output.stderr).to_string()
}

fn git(repository: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn git_connector_is_shell_free_bounded_and_read_only_by_default() {
    let data_dir = temp_dir("data-git");
    let repository = temp_dir("repo");
    git(&repository, &["init"]);
    git(&repository, &["config", "user.name", "HPA Test"]);
    git(
        &repository,
        &["config", "user.email", "hpa@example.invalid"],
    );
    std::fs::write(repository.join("tracked.txt"), "first\n").expect("write tracked file");
    git(&repository, &["add", "tracked.txt"]);
    git(&repository, &["commit", "-m", "initial"]);

    let config = json!({
        "repositoryRoot": repository,
        "allowWrite": false,
        "maxOutputBytes": 65536
    });
    let connection = require_cli(
        &data_dir,
        &[
            "connector",
            "add",
            "fixture-git",
            "git",
            "--config",
            &config.to_string(),
        ],
        &[],
    );
    let connection_id = connection["id"].as_str().expect("connection ID");

    let tested = require_cli(&data_dir, &["connector", "test", connection_id], &[]);
    assert_eq!(tested["test"]["ok"], true);
    assert_eq!(tested["test"]["kind"], "git");

    let status = require_cli(
        &data_dir,
        &[
            "connector",
            "invoke",
            connection_id,
            "status",
            "--args",
            "{}",
        ],
        &[],
    );
    assert_eq!(status["kind"], "git");
    assert!(status["stdout"].as_str().expect("status").contains("##"));

    let log = require_cli(
        &data_dir,
        &[
            "connector",
            "invoke",
            connection_id,
            "log",
            "--args",
            r#"{"limit":5}"#,
        ],
        &[],
    );
    assert!(log["stdout"].as_str().expect("log").contains("initial"));

    let traversal = failure_cli(
        &data_dir,
        &[
            "connector",
            "invoke",
            connection_id,
            "show",
            "--args",
            r#"{"revision":"HEAD","path":"../secret"}"#,
        ],
        &[],
    );
    assert!(traversal.contains("path traversal"), "{traversal}");

    let commit_denied = failure_cli(
        &data_dir,
        &[
            "connector",
            "invoke",
            connection_id,
            "commit",
            "--args",
            r#"{"message":"should fail"}"#,
        ],
        &[],
    );
    assert!(
        commit_denied.contains("write is disabled"),
        "{commit_denied}"
    );

    let audit = require_cli(&data_dir, &["connector", "audit", "--limit", "20"], &[]);
    assert!(audit["entries"].as_array().expect("audit entries").len() >= 4);
    let _ = std::fs::remove_dir_all(data_dir);
    let _ = std::fs::remove_dir_all(repository);
}

#[test]
fn postgres_connector_enforces_read_only_query_and_parameter_binding() {
    let Ok(database_url) = std::env::var("HPA_POSTGRES_TEST_URL") else {
        eprintln!("skipped: HPA_POSTGRES_TEST_URL is not set");
        return;
    };
    let suffix = Uuid::now_v7().simple().to_string();
    let table = format!("hpa_connector_probe_{suffix}");
    let credential_env = format!("HPA_CONNECTOR_DB_{suffix}");
    let data_dir = temp_dir("data-db");
    let config = json!({
        "allowedHosts": ["*"],
        "statementTimeoutMs": 5000,
        "maxRows": 10,
        "maxResponseBytes": 65536
    });
    let connection = require_cli(
        &data_dir,
        &[
            "connector",
            "add",
            "fixture-db",
            "database",
            "--config",
            &config.to_string(),
            "--credential-env",
            &credential_env,
        ],
        &[(credential_env.as_str(), database_url.as_str())],
    );
    let connection_id = connection["id"].as_str().expect("connection ID");
    let environment = [(credential_env.as_str(), database_url.as_str())];

    let tested = require_cli(
        &data_dir,
        &["connector", "test", connection_id],
        &environment,
    );
    assert_eq!(tested["test"]["ok"], true);

    require_cli(
        &data_dir,
        &[
            "connector",
            "invoke",
            connection_id,
            "execute",
            "--args",
            &json!({"sql": format!("CREATE TABLE {table} (id INTEGER PRIMARY KEY, name TEXT)")})
                .to_string(),
        ],
        &environment,
    );
    let inserted = require_cli(
        &data_dir,
        &[
            "connector",
            "invoke",
            connection_id,
            "execute",
            "--args",
            &json!({
                "sql": format!("INSERT INTO {table} (id, name) VALUES ($1, $2)"),
                "parameters": [7, "seven"]
            })
            .to_string(),
        ],
        &environment,
    );
    assert_eq!(inserted["rowsAffected"], 1);
    let queried = require_cli(
        &data_dir,
        &[
            "connector",
            "invoke",
            connection_id,
            "query",
            "--args",
            &json!({
                "sql": format!("SELECT id, name FROM {table} WHERE id = $1"),
                "parameters": [7]
            })
            .to_string(),
        ],
        &environment,
    );
    assert_eq!(queried["rowCount"], 1);
    assert_eq!(queried["rows"][0]["id"], 7);
    assert_eq!(queried["rows"][0]["name"], "seven");

    let write_in_query = failure_cli(
        &data_dir,
        &[
            "connector",
            "invoke",
            connection_id,
            "query",
            "--args",
            &json!({"sql": format!("DELETE FROM {table}")}).to_string(),
        ],
        &environment,
    );
    assert!(
        write_in_query.contains("must start with SELECT"),
        "{write_in_query}"
    );

    require_cli(
        &data_dir,
        &[
            "connector",
            "invoke",
            connection_id,
            "execute",
            "--args",
            &json!({"sql": format!("DROP TABLE {table}")}).to_string(),
        ],
        &environment,
    );
    let _ = std::fs::remove_dir_all(data_dir);
}

#[test]
fn redis_connector_publishes_once_with_idempotency_and_allowlisted_channels() {
    let available = Command::new("redis-server")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success());
    if !available {
        eprintln!("skipped: redis-server is not installed");
        return;
    }
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("reserve Redis port")
        .local_addr()
        .expect("Redis address")
        .port();
    let mut redis = Command::new("redis-server")
        .args([
            "--bind",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--save",
            "",
            "--appendonly",
            "no",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start Redis");
    for _ in 0..50 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let mut subscriber = Command::new("redis-cli")
        .args([
            "-h",
            "127.0.0.1",
            "-p",
            &port.to_string(),
            "--csv",
            "subscribe",
            "hpa.test.events",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("start Redis subscriber");
    std::thread::sleep(Duration::from_millis(200));

    let data_dir = temp_dir("data-mq");
    let config = json!({
        "url": format!("redis://127.0.0.1:{port}"),
        "allowedHosts": ["127.0.0.1"],
        "allowedChannels": ["hpa.*"],
        "database": 0,
        "maxMessageBytes": 65536
    });
    let connection = require_cli(
        &data_dir,
        &[
            "connector",
            "add",
            "fixture-mq",
            "mq",
            "--config",
            &config.to_string(),
        ],
        &[],
    );
    let connection_id = connection["id"].as_str().expect("MQ connection ID");
    let tested = require_cli(&data_dir, &["connector", "test", connection_id], &[]);
    assert_eq!(tested["test"]["ok"], true);

    let arguments =
        r#"{"channel":"hpa.test.events","message":"event-one","idempotencyKey":"event-1"}"#;
    let first = require_cli(
        &data_dir,
        &[
            "connector",
            "invoke",
            connection_id,
            "publish",
            "--args",
            arguments,
        ],
        &[],
    );
    assert_eq!(first["duplicate"], false);
    assert!(first["published"].as_i64().unwrap_or_default() >= 1);
    let duplicate = require_cli(
        &data_dir,
        &[
            "connector",
            "invoke",
            connection_id,
            "publish",
            "--args",
            arguments,
        ],
        &[],
    );
    assert_eq!(duplicate["duplicate"], true);
    assert_eq!(duplicate["published"], 0);
    let forbidden = failure_cli(
        &data_dir,
        &[
            "connector",
            "invoke",
            connection_id,
            "publish",
            "--args",
            r#"{"channel":"private.events","message":"blocked","idempotencyKey":"event-2"}"#,
        ],
        &[],
    );
    assert!(forbidden.contains("is not allowed"), "{forbidden}");

    std::thread::sleep(Duration::from_millis(200));
    let _ = subscriber.kill();
    let _ = subscriber.wait();
    let mut subscribed = String::new();
    if let Some(mut stdout) = subscriber.stdout.take() {
        stdout
            .read_to_string(&mut subscribed)
            .expect("read subscriber");
    }
    assert!(subscribed.contains("event-one"), "{subscribed}");
    let _ = redis.kill();
    let _ = redis.wait();
    let _ = std::fs::remove_dir_all(data_dir);
}

#[test]
fn s3_connector_signs_bounded_round_trip_and_rejects_key_escape() {
    let Ok(endpoint) = std::env::var("HPA_S3_TEST_ENDPOINT") else {
        eprintln!("skipped: HPA_S3_TEST_ENDPOINT is not set");
        return;
    };
    let access_key = std::env::var("HPA_S3_TEST_ACCESS_KEY").expect("S3 access key");
    let secret_key = std::env::var("HPA_S3_TEST_SECRET_KEY").expect("S3 secret key");
    let bucket = std::env::var("HPA_S3_TEST_BUCKET").expect("S3 bucket");
    let host = reqwest::Url::parse(&endpoint)
        .expect("S3 endpoint")
        .host_str()
        .expect("S3 host")
        .to_string();
    let suffix = Uuid::now_v7().simple().to_string();
    let credential_env = format!("HPA_CONNECTOR_S3_{suffix}");
    let credentials = json!({
        "accessKey": access_key,
        "secretKey": secret_key
    })
    .to_string();
    let data_dir = temp_dir("data-s3");
    let config = json!({
        "endpoint": endpoint,
        "region": "us-east-1",
        "bucket": bucket,
        "allowedHosts": [host],
        "maxObjectBytes": 65536
    });
    let connection = require_cli(
        &data_dir,
        &[
            "connector",
            "add",
            "fixture-s3",
            "s3",
            "--config",
            &config.to_string(),
            "--credential-env",
            &credential_env,
        ],
        &[(credential_env.as_str(), credentials.as_str())],
    );
    let connection_id = connection["id"].as_str().expect("S3 connection ID");
    let environment = [(credential_env.as_str(), credentials.as_str())];
    let tested = require_cli(
        &data_dir,
        &["connector", "test", connection_id],
        &environment,
    );
    assert_eq!(tested["test"]["ok"], true);

    let key = format!("connector-tests/{suffix}.txt");
    let content = b"signed object";
    let put = require_cli(
        &data_dir,
        &[
            "connector",
            "invoke",
            connection_id,
            "put",
            "--args",
            &json!({
                "key": key,
                "body": BASE64.encode(content),
                "contentType": "text/plain"
            })
            .to_string(),
        ],
        &environment,
    );
    assert_eq!(put["bytes"], content.len());
    let head = require_cli(
        &data_dir,
        &[
            "connector",
            "invoke",
            connection_id,
            "head",
            "--args",
            &json!({"key": key}).to_string(),
        ],
        &environment,
    );
    assert_eq!(head["contentLength"], content.len());
    let got = require_cli(
        &data_dir,
        &[
            "connector",
            "invoke",
            connection_id,
            "get",
            "--args",
            &json!({"key": key}).to_string(),
        ],
        &environment,
    );
    let body = BASE64
        .decode(got["body"].as_str().expect("S3 body"))
        .expect("base64 response");
    assert_eq!(body, content);
    let listed = require_cli(
        &data_dir,
        &[
            "connector",
            "invoke",
            connection_id,
            "list",
            "--args",
            &json!({"prefix": format!("connector-tests/{suffix}")}).to_string(),
        ],
        &environment,
    );
    assert_eq!(listed["count"], 1);
    assert_eq!(listed["keys"][0], key);
    let traversal = failure_cli(
        &data_dir,
        &[
            "connector",
            "invoke",
            connection_id,
            "get",
            "--args",
            &json!({"key": "../secret"}).to_string(),
        ],
        &environment,
    );
    assert!(traversal.contains("invalid S3 object key"), "{traversal}");
    require_cli(
        &data_dir,
        &[
            "connector",
            "invoke",
            connection_id,
            "delete",
            "--args",
            &json!({"key": key}).to_string(),
        ],
        &environment,
    );
    let _ = std::fs::remove_dir_all(data_dir);
}
