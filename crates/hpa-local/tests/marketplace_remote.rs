use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{Signer, SigningKey};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SignedIndex {
    schema_version: String,
    source_id: String,
    sequence: i64,
    generated_at: String,
    packages: Vec<IndexPackage>,
    withdrawals: Vec<IndexWithdrawal>,
    signature: IndexSignature,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct IndexPackage {
    plugin_id: String,
    version: String,
    path: String,
    sha256: String,
    publisher_id: String,
    signer_key_id: String,
    review_status: String,
    published_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provenance: Option<IndexProvenance>,
    #[serde(skip_serializing_if = "Option::is_none")]
    oci: Option<IndexOci>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct IndexProvenance {
    kind: String,
    statement_path: String,
    statement_sha256: String,
    bundle_path: String,
    bundle_sha256: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct IndexOci {
    repository: String,
    digest: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct IndexWithdrawal {
    plugin_id: String,
    version: String,
    reason: String,
    at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IndexPayload {
    schema_version: String,
    source_id: String,
    sequence: i64,
    generated_at: String,
    packages: Vec<IndexPackage>,
    withdrawals: Vec<IndexWithdrawal>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IndexSignature {
    algorithm: String,
    key_id: String,
    public_key: String,
    value: String,
}

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hpa")
}

fn temp_dir(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("hpa-remote-market-{label}-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&path).expect("temp directory");
    path
}

fn run_cli(data_dir: &Path, args: &[String]) -> std::process::Output {
    Command::new(binary())
        .arg("--data-dir")
        .arg(data_dir)
        .args(args)
        .output()
        .expect("run plugin CLI")
}

fn run_cli_env(
    data_dir: &Path,
    args: &[String],
    environment: &[(&str, String)],
) -> std::process::Output {
    let mut command = Command::new(binary());
    command.arg("--data-dir").arg(data_dir).args(args);
    for (key, value) in environment {
        command.env(key, value);
    }
    command.output().expect("run plugin CLI with environment")
}

fn require_cli_env(data_dir: &Path, args: &[String], environment: &[(&str, String)]) -> Value {
    let output = run_cli_env(data_dir, args, environment);
    assert!(
        output.status.success(),
        "plugin CLI {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("plugin CLI JSON")
}

fn require_cli(data_dir: &Path, args: &[String]) -> Value {
    let output = run_cli(data_dir, args);
    assert!(
        output.status.success(),
        "plugin CLI {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("plugin CLI JSON")
}

fn failure_cli(data_dir: &Path, args: &[String]) -> String {
    let output = run_cli(data_dir, args);
    assert!(
        !output.status.success(),
        "plugin CLI {args:?} unexpectedly succeeded"
    );
    String::from_utf8_lossy(&output.stderr).to_string()
}

fn key_material() -> (SigningKey, String, String) {
    let mut secret = [0u8; 32];
    getrandom::getrandom(&mut secret).expect("random key");
    let key = SigningKey::from_bytes(&secret);
    let public = BASE64.encode(key.verifying_key().to_bytes());
    let key_id = format!(
        "source-{}",
        &format!("{:x}", Sha256::digest(key.verifying_key().to_bytes()))[..16]
    );
    (key, key_id, public)
}

fn signed_index(
    source_id: &str,
    sequence: i64,
    packages: Vec<IndexPackage>,
    withdrawals: Vec<IndexWithdrawal>,
    key: &SigningKey,
    key_id: &str,
) -> Value {
    let schema_version = "hpa.marketplace.index/v1".to_string();
    let generated_at = "2026-11-01T00:00:00Z".to_string();
    let payload = IndexPayload {
        schema_version: schema_version.clone(),
        source_id: source_id.to_string(),
        sequence,
        generated_at: generated_at.clone(),
        packages: packages.clone(),
        withdrawals: withdrawals.clone(),
    };
    let signature = key.sign(&serde_json::to_vec(&payload).expect("payload"));
    serde_json::to_value(SignedIndex {
        schema_version,
        source_id: source_id.to_string(),
        sequence,
        generated_at,
        packages,
        withdrawals,
        signature: IndexSignature {
            algorithm: "ed25519".to_string(),
            key_id: key_id.to_string(),
            public_key: BASE64.encode(key.verifying_key().to_bytes()),
            value: BASE64.encode(signature.to_bytes()),
        },
    })
    .expect("index JSON")
}

fn plugin_fixture(root: &Path) {
    std::fs::create_dir_all(root.join("connectors")).expect("connector directory");
    std::fs::write(
        root.join("hpa.plugin.json"),
        serde_json::to_vec_pretty(&json!({
            "schemaVersion": "hpa.plugin/v1",
            "id": "plugin_remote_fixture",
            "name": "Remote Fixture",
            "version": "1.0.0",
            "license": "Apache-2.0",
            "publisher": {"id": "publisher_remote", "name": "Remote Publisher"},
            "compatibility": {"hpa": ">=0.1.0, <1.0.0", "os": []},
            "capabilities": [
                {"kind": "connector", "id": "remote_probe", "version": "1.0.0", "path": "connectors/probe.json"}
            ],
            "dependencies": [],
            "permissions": [{"kind": "network", "hosts": ["127.0.0.1"]}]
        }))
        .expect("manifest"),
    )
    .expect("write manifest");
    std::fs::write(
        root.join("connectors/probe.json"),
        serde_json::to_vec_pretty(&json!({
            "schemaVersion": "hpa.connector/v1",
            "id": "remote_probe",
            "version": "1.0.0",
            "name": "Remote probe",
            "kind": "http",
            "auth": [],
            "operations": [{
                "name": "ping",
                "risk": "read",
                "inputSchema": {"type": "object"},
                "http": {"method": "GET", "path": "/"}
            }],
            "permissions": [{"kind": "network", "hosts": ["127.0.0.1"]}]
        }))
        .expect("connector"),
    )
    .expect("write connector");
}

#[test]
fn remote_marketplace_verifies_publishers_rotation_hash_and_withdrawal() {
    let data_dir = temp_dir("data");
    let source_root = temp_dir("mirror");
    let plugin_dir = temp_dir("plugin");
    plugin_fixture(&plugin_dir);
    require_cli(
        &data_dir,
        &[
            "plugin".to_string(),
            "sign".to_string(),
            plugin_dir.to_string_lossy().to_string(),
        ],
    );
    let package_path = source_root.join("packages/plugin_remote_fixture-1.0.0.hplugin");
    std::fs::create_dir_all(package_path.parent().expect("package parent"))
        .expect("package directory");
    require_cli(
        &data_dir,
        &[
            "plugin".to_string(),
            "pack".to_string(),
            plugin_dir.to_string_lossy().to_string(),
            package_path.to_string_lossy().to_string(),
        ],
    );
    let plugin_key = require_cli(&data_dir, &["plugin".to_string(), "key".to_string()]);
    require_cli(
        &data_dir,
        &[
            "plugin".to_string(),
            "publisher-add".to_string(),
            "publisher_remote".to_string(),
            "Remote Publisher".to_string(),
        ],
    );
    require_cli(
        &data_dir,
        &[
            "plugin".to_string(),
            "publisher-key-add".to_string(),
            "publisher_remote".to_string(),
            "--key-id".to_string(),
            plugin_key["keyId"]
                .as_str()
                .expect("plugin key")
                .to_string(),
            "--public-key".to_string(),
            plugin_key["publicKey"]
                .as_str()
                .expect("plugin public key")
                .to_string(),
        ],
    );

    let (source_key, source_key_id, source_public) = key_material();
    let package_bytes = std::fs::read(&package_path).expect("package bytes");
    let package = IndexPackage {
        plugin_id: "plugin_remote_fixture".to_string(),
        version: "1.0.0".to_string(),
        path: "packages/plugin_remote_fixture-1.0.0.hplugin".to_string(),
        sha256: format!("{:x}", Sha256::digest(&package_bytes)),
        publisher_id: "publisher_remote".to_string(),
        signer_key_id: plugin_key["keyId"].as_str().expect("key ID").to_string(),
        review_status: "approved".to_string(),
        published_at: Some("2026-11-01T00:00:00Z".to_string()),
        provenance: None,
        oci: None,
    };
    let index_path = source_root.join("index.json");
    std::fs::write(
        &index_path,
        serde_json::to_vec_pretty(&signed_index(
            "community",
            1,
            vec![package.clone()],
            Vec::new(),
            &source_key,
            &source_key_id,
        ))
        .expect("index"),
    )
    .expect("write index");
    require_cli(
        &data_dir,
        &[
            "plugin".to_string(),
            "marketplace-source-add".to_string(),
            "community".to_string(),
            "--label".to_string(),
            "Community mirror".to_string(),
            "--index".to_string(),
            index_path.to_string_lossy().to_string(),
            "--key-id".to_string(),
            source_key_id.clone(),
            "--public-key".to_string(),
            source_public,
        ],
    );
    let refreshed = require_cli(
        &data_dir,
        &[
            "plugin".to_string(),
            "marketplace-source-refresh".to_string(),
            "community".to_string(),
        ],
    );
    assert_eq!(refreshed["packages"], 1);
    std::fs::write(
        &index_path,
        serde_json::to_vec_pretty(&signed_index(
            "community",
            0,
            vec![package.clone()],
            Vec::new(),
            &source_key,
            &source_key_id,
        ))
        .expect("rollback index"),
    )
    .expect("write rollback index");
    let rollback = failure_cli(
        &data_dir,
        &[
            "plugin".to_string(),
            "marketplace-source-refresh".to_string(),
            "community".to_string(),
        ],
    );
    assert!(rollback.contains("rollback"), "{rollback}");
    let installed = require_cli(
        &data_dir,
        &[
            "plugin".to_string(),
            "marketplace-remote-install".to_string(),
            "community".to_string(),
            "plugin_remote_fixture".to_string(),
            "1.0.0".to_string(),
        ],
    );
    assert_eq!(installed["plugin"]["id"], "plugin_remote_fixture");

    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("reserve marketplace port")
        .local_addr()
        .expect("marketplace address")
        .port();
    let mut marketplace_server = Command::new("python3")
        .args([
            "-m",
            "http.server",
            &port.to_string(),
            "--bind",
            "127.0.0.1",
        ])
        .current_dir(&source_root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start marketplace HTTP source");
    for _ in 0..50 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    std::fs::write(
        source_root.join("http-index.json"),
        serde_json::to_vec_pretty(&signed_index(
            "http-source",
            1,
            vec![package.clone()],
            Vec::new(),
            &source_key,
            &source_key_id,
        ))
        .expect("HTTP index"),
    )
    .expect("write HTTP index");
    require_cli(
        &data_dir,
        &[
            "plugin".to_string(),
            "marketplace-source-add".to_string(),
            "http-source".to_string(),
            "--label".to_string(),
            "HTTP mirror".to_string(),
            "--index".to_string(),
            format!("http://127.0.0.1:{port}/http-index.json"),
            "--key-id".to_string(),
            source_key_id.clone(),
            "--public-key".to_string(),
            BASE64.encode(source_key.verifying_key().to_bytes()),
        ],
    );
    require_cli(
        &data_dir,
        &[
            "plugin".to_string(),
            "marketplace-source-refresh".to_string(),
            "http-source".to_string(),
        ],
    );
    let http_packages = require_cli(
        &data_dir,
        &[
            "plugin".to_string(),
            "marketplace-remote".to_string(),
            "--source".to_string(),
            "http-source".to_string(),
        ],
    );
    assert_eq!(
        http_packages["packages"]
            .as_array()
            .expect("packages")
            .len(),
        1
    );
    let _ = marketplace_server.kill();
    let _ = marketplace_server.wait();

    std::fs::write(&package_path, b"tampered package").expect("tamper package");
    std::fs::write(
        &index_path,
        serde_json::to_vec_pretty(&signed_index(
            "community",
            2,
            vec![package.clone()],
            Vec::new(),
            &source_key,
            &source_key_id,
        ))
        .expect("tampered index"),
    )
    .expect("write tampered index");
    let tampered = failure_cli(
        &data_dir,
        &[
            "plugin".to_string(),
            "marketplace-source-refresh".to_string(),
            "community".to_string(),
        ],
    );
    assert!(tampered.contains("hash does not match"), "{tampered}");
    std::fs::write(&package_path, &package_bytes).expect("restore package");

    require_cli(
        &data_dir,
        &[
            "plugin".to_string(),
            "publisher-key-revoke".to_string(),
            "publisher_remote".to_string(),
            "--key-id".to_string(),
            plugin_key["keyId"]
                .as_str()
                .expect("plugin key")
                .to_string(),
        ],
    );
    std::fs::write(
        &index_path,
        serde_json::to_vec_pretty(&signed_index(
            "community",
            2,
            vec![package.clone()],
            Vec::new(),
            &source_key,
            &source_key_id,
        ))
        .expect("revoked publisher index"),
    )
    .expect("write revoked publisher index");
    let revoked_publisher = failure_cli(
        &data_dir,
        &[
            "plugin".to_string(),
            "marketplace-source-refresh".to_string(),
            "community".to_string(),
        ],
    );
    assert!(revoked_publisher.contains("revoked"), "{revoked_publisher}");

    let withdrawal = IndexWithdrawal {
        plugin_id: "plugin_remote_fixture".to_string(),
        version: "1.0.0".to_string(),
        reason: "security advisory".to_string(),
        at: "2026-11-01T01:00:00Z".to_string(),
    };
    std::fs::write(
        &index_path,
        serde_json::to_vec_pretty(&signed_index(
            "community",
            2,
            Vec::new(),
            vec![withdrawal],
            &source_key,
            &source_key_id,
        ))
        .expect("withdrawal index"),
    )
    .expect("write withdrawal index");
    require_cli(
        &data_dir,
        &[
            "plugin".to_string(),
            "marketplace-source-refresh".to_string(),
            "community".to_string(),
        ],
    );
    let detail = require_cli(
        &data_dir,
        &[
            "plugin".to_string(),
            "show".to_string(),
            "plugin_remote_fixture".to_string(),
        ],
    );
    assert_eq!(detail["plugin"]["status"], "disabled");
    let denied = failure_cli(
        &data_dir,
        &[
            "plugin".to_string(),
            "marketplace-remote-install".to_string(),
            "community".to_string(),
            "plugin_remote_fixture".to_string(),
            "1.0.0".to_string(),
        ],
    );
    assert!(denied.contains("withdrawn"), "{denied}");

    let (rotated_key, rotated_key_id, rotated_public) = key_material();
    require_cli(
        &data_dir,
        &[
            "plugin".to_string(),
            "marketplace-source-rotate".to_string(),
            "community".to_string(),
            "--key-id".to_string(),
            rotated_key_id.clone(),
            "--public-key".to_string(),
            rotated_public,
        ],
    );
    std::fs::write(
        &index_path,
        serde_json::to_vec_pretty(&signed_index(
            "community",
            3,
            Vec::new(),
            Vec::new(),
            &rotated_key,
            &rotated_key_id,
        ))
        .expect("rotated index"),
    )
    .expect("write rotated index");
    require_cli(
        &data_dir,
        &[
            "plugin".to_string(),
            "marketplace-source-refresh".to_string(),
            "community".to_string(),
        ],
    );
    std::fs::write(
        &index_path,
        serde_json::to_vec_pretty(&signed_index(
            "community",
            4,
            Vec::new(),
            Vec::new(),
            &source_key,
            &source_key_id,
        ))
        .expect("old-key index"),
    )
    .expect("write old-key index");
    let old_key = failure_cli(
        &data_dir,
        &[
            "plugin".to_string(),
            "marketplace-source-refresh".to_string(),
            "community".to_string(),
        ],
    );
    assert!(old_key.contains("revoked"), "{old_key}");
    let _ = std::fs::remove_dir_all(data_dir);
    let _ = std::fs::remove_dir_all(source_root);
    let _ = std::fs::remove_dir_all(plugin_dir);
}

#[test]
fn remote_marketplace_verifies_cosign_provenance_and_oci_binding() {
    let Ok(cosign) = std::env::var("HPA_COSIGN_BIN") else {
        eprintln!("skipped: HPA_COSIGN_BIN is not set");
        return;
    };
    let data_dir = temp_dir("provenance-data");
    let source_root = temp_dir("provenance-mirror");
    let plugin_dir = temp_dir("provenance-plugin");
    plugin_fixture(&plugin_dir);
    require_cli(
        &data_dir,
        &[
            "plugin".to_string(),
            "sign".to_string(),
            plugin_dir.to_string_lossy().to_string(),
        ],
    );
    let package_path = source_root.join("packages/plugin_remote_fixture-1.0.0.hplugin");
    std::fs::create_dir_all(package_path.parent().expect("package parent"))
        .expect("package directory");
    require_cli(
        &data_dir,
        &[
            "plugin".to_string(),
            "pack".to_string(),
            plugin_dir.to_string_lossy().to_string(),
            package_path.to_string_lossy().to_string(),
        ],
    );
    let plugin_key = require_cli(&data_dir, &["plugin".to_string(), "key".to_string()]);
    require_cli(
        &data_dir,
        &[
            "plugin".to_string(),
            "publisher-add".to_string(),
            "publisher_remote".to_string(),
            "Remote Publisher".to_string(),
        ],
    );
    require_cli(
        &data_dir,
        &[
            "plugin".to_string(),
            "publisher-key-add".to_string(),
            "publisher_remote".to_string(),
            "--key-id".to_string(),
            plugin_key["keyId"]
                .as_str()
                .expect("plugin key")
                .to_string(),
            "--public-key".to_string(),
            plugin_key["publicKey"]
                .as_str()
                .expect("plugin public key")
                .to_string(),
        ],
    );
    let (source_key, source_key_id, source_public) = key_material();
    let package_bytes = std::fs::read(&package_path).expect("package bytes");
    let package_sha256 = format!("{:x}", Sha256::digest(&package_bytes));
    let oci_digest = format!("sha256:{:x}", Sha256::digest(b"oci-manifest"));
    let provenance_dir = source_root.join("provenance");
    std::fs::create_dir_all(&provenance_dir).expect("provenance directory");
    let statement_path = provenance_dir.join("plugin_remote_fixture-1.0.0.json");
    let bundle_path = provenance_dir.join("plugin_remote_fixture-1.0.0.bundle.json");
    let statement = serde_json::to_vec(&json!({
        "_type": "https://in-toto.io/Statement/v1",
        "subject": [
            {
                "name": "plugin_remote_fixture-1.0.0.hplugin",
                "digest": {"sha256": package_sha256}
            },
            {
                "name": "registry.example/hpa/plugin_remote_fixture",
                "digest": {"sha256": oci_digest.trim_start_matches("sha256:").to_string()}
            }
        ],
        "predicateType": "https://hpa.dev/provenance/plugin/v1",
        "predicate": {
            "builder": {"id": "https://builder.example/hpa"},
            "buildType": "https://hpa.dev/build/plugin/v1"
        }
    }))
    .expect("provenance statement");
    std::fs::write(&statement_path, &statement).expect("write statement");
    let key_prefix = provenance_dir.join("cosign");
    let generated = Command::new(&cosign)
        .args(["generate-key-pair", "--output-key-prefix"])
        .arg(&key_prefix)
        .env("COSIGN_PASSWORD", "test-password")
        .output()
        .expect("generate cosign key");
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let private_key = key_prefix.with_extension("key");
    let public_key = key_prefix.with_extension("pub");
    let signed = Command::new(&cosign)
        .args([
            "sign-blob",
            "--yes",
            "--tlog-upload=false",
            "--key",
            private_key.to_str().expect("private key"),
            "--bundle",
            bundle_path.to_str().expect("bundle"),
            statement_path.to_str().expect("statement"),
        ])
        .env("COSIGN_PASSWORD", "test-password")
        .output()
        .expect("sign provenance");
    assert!(
        signed.status.success(),
        "{}",
        String::from_utf8_lossy(&signed.stderr)
    );
    let bundle = std::fs::read(&bundle_path).expect("bundle");
    let package = IndexPackage {
        plugin_id: "plugin_remote_fixture".to_string(),
        version: "1.0.0".to_string(),
        path: "packages/plugin_remote_fixture-1.0.0.hplugin".to_string(),
        sha256: package_sha256,
        publisher_id: "publisher_remote".to_string(),
        signer_key_id: plugin_key["keyId"].as_str().expect("key ID").to_string(),
        review_status: "approved".to_string(),
        published_at: Some("2026-11-01T00:00:00Z".to_string()),
        provenance: Some(IndexProvenance {
            kind: "cosign-v1".to_string(),
            statement_path: "provenance/plugin_remote_fixture-1.0.0.json".to_string(),
            statement_sha256: format!("{:x}", Sha256::digest(&statement)),
            bundle_path: "provenance/plugin_remote_fixture-1.0.0.bundle.json".to_string(),
            bundle_sha256: format!("{:x}", Sha256::digest(&bundle)),
        }),
        oci: Some(IndexOci {
            repository: "registry.example/hpa/plugin_remote_fixture".to_string(),
            digest: oci_digest,
        }),
    };
    let index_path = source_root.join("index.json");
    std::fs::write(
        &index_path,
        serde_json::to_vec_pretty(&signed_index(
            "provenance",
            1,
            vec![package],
            Vec::new(),
            &source_key,
            &source_key_id,
        ))
        .expect("provenance index"),
    )
    .expect("write provenance index");
    require_cli(
        &data_dir,
        &[
            "plugin".to_string(),
            "marketplace-source-add".to_string(),
            "provenance".to_string(),
            "--label".to_string(),
            "Provenance mirror".to_string(),
            "--index".to_string(),
            index_path.to_string_lossy().to_string(),
            "--key-id".to_string(),
            source_key_id,
            "--public-key".to_string(),
            source_public,
        ],
    );
    let environment = [
        ("HPA_REQUIRE_PROVENANCE", "1".to_string()),
        ("HPA_COSIGN_BIN", cosign),
        (
            "HPA_COSIGN_PUBLIC_KEY",
            public_key.to_string_lossy().to_string(),
        ),
    ];
    require_cli_env(
        &data_dir,
        &[
            "plugin".to_string(),
            "marketplace-source-refresh".to_string(),
            "provenance".to_string(),
        ],
        &environment,
    );
    let remote = require_cli(
        &data_dir,
        &[
            "plugin".to_string(),
            "marketplace-remote".to_string(),
            "--source".to_string(),
            "provenance".to_string(),
        ],
    );
    assert_eq!(remote["packages"][0]["provenance"]["kind"], "cosign-v1");
    let installed = require_cli_env(
        &data_dir,
        &[
            "plugin".to_string(),
            "marketplace-remote-install".to_string(),
            "provenance".to_string(),
            "plugin_remote_fixture".to_string(),
            "1.0.0".to_string(),
        ],
        &environment,
    );
    assert_eq!(installed["plugin"]["id"], "plugin_remote_fixture");

    let cached_statement = data_dir.join(
        "marketplace/sources/provenance/packages/plugin_remote_fixture/1.0.0.provenance.json",
    );
    std::fs::write(&cached_statement, b"tampered provenance").expect("tamper provenance");
    let denied = run_cli_env(
        &data_dir,
        &[
            "plugin".to_string(),
            "marketplace-remote-install".to_string(),
            "provenance".to_string(),
            "plugin_remote_fixture".to_string(),
            "1.0.0".to_string(),
        ],
        &environment,
    );
    assert!(!denied.status.success());
    assert!(
        String::from_utf8_lossy(&denied.stderr).contains("provenance statement hash"),
        "{}",
        String::from_utf8_lossy(&denied.stderr)
    );
    let _ = std::fs::remove_dir_all(data_dir);
    let _ = std::fs::remove_dir_all(source_root);
    let _ = std::fs::remove_dir_all(plugin_dir);
}
