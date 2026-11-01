use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const MANIFEST_FILE: &str = "hpa.plugin.json";

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hpa")
}

fn temp_dir(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("hpa-plugins-{label}-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&path).expect("temp dir");
    path
}

fn run_cli(data_dir: &Path, args: &[&str]) -> Output {
    Command::new(binary())
        .arg("--data-dir")
        .arg(data_dir)
        .args(args)
        .output()
        .expect("run hpa cli")
}

fn require_cli(data_dir: &Path, args: &[&str]) -> Value {
    let output = run_cli(data_dir, args);
    assert!(
        output.status.success(),
        "cli `{args:?}` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("cli stdout is json")
}

fn failure_cli(data_dir: &Path, args: &[&str]) -> String {
    let output = run_cli(data_dir, args);
    assert!(
        !output.status.success(),
        "cli `{args:?}` unexpectedly succeeded: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    String::from_utf8_lossy(&output.stderr).to_string()
}

fn write_files(root: &Path, files: &BTreeMap<String, Vec<u8>>) {
    for (path, bytes) in files {
        let target = root.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).expect("create parent dir");
        }
        std::fs::write(&target, bytes).expect("write fixture file");
    }
}

fn read_files(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, directory: &Path, files: &mut BTreeMap<String, Vec<u8>>) {
        for entry in std::fs::read_dir(directory).expect("read fixture dir") {
            let entry = entry.expect("fixture entry");
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, files);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .expect("strip root")
                    .to_string_lossy()
                    .replace('\\', "/");
                files.insert(relative, std::fs::read(&path).expect("read fixture file"));
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(root, root, &mut files);
    files
}

fn content_hash(files: &BTreeMap<String, Vec<u8>>) -> String {
    let mut hasher = Sha256::new();
    for (path, bytes) in files {
        if path == MANIFEST_FILE {
            continue;
        }
        let file_hash = format!("{:x}", Sha256::digest(bytes));
        hasher.update(path.as_bytes());
        hasher.update([0]);
        hasher.update(bytes.len().to_string().as_bytes());
        hasher.update([0]);
        hasher.update(file_hash.as_bytes());
        hasher.update([0]);
    }
    format!("sha256:{:x}", hasher.finalize())
}

fn pack_tar(files: &BTreeMap<String, Vec<u8>>, target: &Path) {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, bytes) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_cksum();
        builder
            .append_data(&mut header, path, bytes.as_slice())
            .expect("append tar entry");
    }
    builder.finish().expect("finish tar");
    std::fs::write(target, builder.into_inner().expect("tar bytes")).expect("write package");
}

fn append_raw_tar_entry(archive: &mut Vec<u8>, name: &str, data: &[u8]) {
    let mut header = [0u8; 512];
    header[..name.len()].copy_from_slice(name.as_bytes());
    header[100..108].copy_from_slice(b"0000644\0");
    header[108..116].copy_from_slice(b"0000000\0");
    header[116..124].copy_from_slice(b"0000000\0");
    header[124..135].copy_from_slice(format!("{:011o}", data.len()).as_bytes());
    header[135] = 0;
    header[136..148].copy_from_slice(b"00000000000\0");
    header[148..156].copy_from_slice(b"        ");
    header[156] = b'0';
    let checksum: u32 = header.iter().map(|byte| u32::from(*byte)).sum();
    header[148..156].copy_from_slice(format!("{checksum:06o}\0 ").as_bytes());
    archive.extend_from_slice(&header);
    archive.extend_from_slice(data);
    archive.extend(std::iter::repeat_n(0u8, (512 - data.len() % 512) % 512));
}

fn unsigned_package(files: BTreeMap<String, Vec<u8>>, target: &Path) {
    let mut files = files;
    let mut manifest: Value = serde_json::from_slice(
        files
            .get(MANIFEST_FILE)
            .expect("manifest present in fixture"),
    )
    .expect("manifest json");
    manifest["integrity"] = json!({"algorithm": "sha256", "digest": content_hash(&files)});
    files.insert(
        MANIFEST_FILE.to_string(),
        serde_json::to_vec_pretty(&manifest).expect("manifest bytes"),
    );
    pack_tar(&files, target);
}

fn plugin_files(
    id: &str,
    version: &str,
    license: &str,
    description: &str,
) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    let manifest = json!({
        "schemaVersion": "hpa.plugin/v1",
        "id": id,
        "name": format!("Fixture {id}"),
        "version": version,
        "description": description,
        "license": license,
        "publisher": {"id": "local", "name": "Local Workspace"},
        "compatibility": {"hpa": ">=0.1.0, <1.0.0", "os": []},
        "capabilities": [
            {"kind": "connector", "id": "plugin_http", "version": version, "path": "connectors/plugin-http.connector.json"},
            {"kind": "skill", "id": "skill_plugin_guide", "version": version, "path": "skills/plugin-guide"},
            {"kind": "agent", "id": "agent_plugin_probe", "version": version, "path": "agents/plugin.agent.json"}
        ],
        "dependencies": [],
        "permissions": [{"kind": "network", "hosts": ["127.0.0.1"]}]
    });
    files.insert(
        MANIFEST_FILE.to_string(),
        serde_json::to_vec_pretty(&manifest).expect("manifest"),
    );
    files.insert(
        "connectors/plugin-http.connector.json".to_string(),
        serde_json::to_vec_pretty(&json!({
            "schemaVersion": "hpa.connector/v1",
            "id": "plugin_http",
            "version": version,
            "name": "Plugin HTTP",
            "kind": "http",
            "description": "Declarative HTTP connector contributed by the fixture plugin.",
            "auth": [],
            "operations": [{
                "name": "ping",
                "description": "GET the connection root.",
                "risk": "read",
                "inputSchema": {"type": "object", "properties": {}},
                "http": {"method": "GET", "path": "/"}
            }],
            "permissions": [{"kind": "network", "hosts": ["127.0.0.1"]}]
        }))
        .expect("connector"),
    );
    files.insert(
        "skills/plugin-guide/SKILL.md".to_string(),
        b"---\nname: plugin-guide\ndescription: Instructions-only skill contributed by a fixture plugin.\nlicense: Apache-2.0\n---\n\n# Plugin Guide\n\nRead this guide to confirm plugin skills import as skill registry drafts.\n"
            .to_vec(),
    );
    files.insert(
        "skills/plugin-guide/hpa.skill.json".to_string(),
        serde_json::to_vec_pretty(&json!({
            "schemaVersion": "hpa.skill/v1",
            "id": "skill_plugin_guide",
            "name": "plugin-guide",
            "version": version,
            "license": "Apache-2.0",
            "entry": {"kind": "instructions"},
            "scripts": [],
            "tests": []
        }))
        .expect("skill"),
    );
    files.insert(
        "agents/plugin.agent.json".to_string(),
        serde_json::to_vec_pretty(&json!({
            "schemaVersion": "hpa.agent/v1",
            "id": "agent_plugin_probe",
            "name": "Plugin Probe Agent",
            "version": version,
            "description": "Imported by the fixture plugin as an AgentVersion draft.",
            "tools": ["workflow.list"],
            "planner": {
                "kind": "deterministic",
                "plan": [
                    {"kind": "tool", "tool": "workflow.list", "arguments": {}},
                    {"kind": "respond", "output": {"workflows": {"$last": "workflows"}}}
                ]
            },
            "subAgents": []
        }))
        .expect("agent"),
    );
    files
}

fn sign_and_pack(data_dir: &Path, directory: &Path, package: &Path) {
    let directory = directory.to_str().expect("directory path");
    let package = package.to_str().expect("package path");
    let signed = require_cli(data_dir, &["plugin", "sign", directory]);
    assert_eq!(signed["signed"], true, "sign output: {signed}");
    let packed = require_cli(data_dir, &["plugin", "pack", directory, package]);
    assert_eq!(packed["trust"], "verified", "pack output: {packed}");
}

struct Server {
    child: Child,
    port: u16,
    client: reqwest::Client,
}

impl Server {
    async fn start(data_dir: &Path) -> Self {
        let port = TcpListener::bind("127.0.0.1:0")
            .expect("bind server port")
            .local_addr()
            .expect("server addr")
            .port();
        let child = Command::new(binary())
            .arg("--data-dir")
            .arg(data_dir)
            .args(["serve", "--port", &port.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn hpa server");
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .expect("http client");
        let server = Self {
            child,
            port,
            client,
        };
        for _ in 0..100 {
            if let Ok(response) = server
                .client
                .get(format!("http://127.0.0.1:{port}/api/health"))
                .send()
                .await
                && response.status().is_success()
            {
                return server;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("server did not become healthy");
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        let response = self
            .client
            .post(self.url(path))
            .json(&body)
            .send()
            .await
            .expect("post request");
        let status = response.status().as_u16();
        let value = response.json().await.unwrap_or(Value::Null);
        (status, value)
    }

    async fn get(&self, path: &str) -> (u16, Value) {
        let response = self
            .client
            .get(self.url(path))
            .send()
            .await
            .expect("get request");
        let status = response.status().as_u16();
        let value = response.json().await.unwrap_or(Value::Null);
        (status, value)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct MockHttp {
    port: u16,
    flaky_hits: Arc<AtomicUsize>,
}

fn spawn_mock_http() -> MockHttp {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock http");
    let port = listener.local_addr().expect("mock addr").port();
    let flaky_hits = Arc::new(AtomicUsize::new(0));
    let counter = flaky_hits.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                break;
            };
            let _ = handle_mock(&mut stream, &counter);
        }
    });
    MockHttp { port, flaky_hits }
}

fn handle_mock(stream: &mut TcpStream, flaky_hits: &AtomicUsize) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut request = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..read]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let head = String::from_utf8_lossy(&request);
    let path = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");
    let (status, content_type, body) = if path.starts_with("/flaky") {
        let hit = flaky_hits.fetch_add(1, Ordering::SeqCst);
        if hit == 0 {
            (500u16, "text/plain", "flaky failure".to_string())
        } else {
            (
                200u16,
                "application/json",
                json!({"flaky": true}).to_string(),
            )
        }
    } else {
        (
            200u16,
            "application/json",
            json!({"message": "pong"}).to_string(),
        )
    };
    let response = format!(
        "HTTP/1.1 {status} STATUS\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

fn connection_config(base_url: &str, max_attempts: u32, rate_limit: u32) -> Value {
    json!({
        "baseUrl": base_url,
        "allowedHosts": [],
        "auth": {"kind": "none"},
        "timeoutMs": 5000,
        "maxAttempts": max_attempts,
        "rateLimitPerMinute": rate_limit
    })
}

#[test]
fn signed_plugin_lifecycle_imports_capabilities_and_exposes_sbom() {
    let data_dir = temp_dir("lifecycle");
    let source = temp_dir("lifecycle-src");
    write_files(
        &source,
        &plugin_files(
            "plugin_lifecycle",
            "1.0.0",
            "Apache-2.0",
            "Signed lifecycle fixture.",
        ),
    );
    let package = data_dir.join("plugin_lifecycle-1.0.0.hplugin");
    sign_and_pack(&data_dir, &source, &package);
    let package = package.to_str().expect("package path");

    let installed = require_cli(&data_dir, &["plugin", "install", package]);
    assert_eq!(installed["plugin"]["id"], "plugin_lifecycle");
    assert_eq!(installed["plugin"]["status"], "enabled");
    assert_eq!(installed["plugin"]["installedVersion"], "1.0.0");
    assert_eq!(installed["plugin"]["trust"], "verified");
    let capabilities = installed["capabilities"]
        .as_array()
        .expect("installed capabilities");
    assert_eq!(capabilities.len(), 3, "capabilities: {capabilities:?}");
    for kind in ["connector", "skill", "agent"] {
        let capability = capabilities
            .iter()
            .find(|capability| capability["kind"] == kind)
            .unwrap_or_else(|| panic!("missing {kind} capability: {capabilities:?}"));
        assert_eq!(capability["status"], "enabled", "{kind} capability");
    }

    let definitions = require_cli(&data_dir, &["connector", "list"]);
    let definition = definitions["connectors"]
        .as_array()
        .expect("connector definitions")
        .iter()
        .find(|definition| {
            definition["connectorId"] == "plugin_http"
                && definition["pluginId"] == "plugin_lifecycle"
        })
        .expect("plugin connector definition");
    assert_eq!(definition["status"], "enabled");

    let skills = require_cli(&data_dir, &["skill", "list"]);
    let skill = skills["skills"]
        .as_array()
        .expect("skills")
        .iter()
        .find(|skill| skill["id"] == "skill_plugin_guide")
        .expect("imported skill");
    assert_eq!(skill["status"], "imported");
    assert_eq!(skill["latestVersion"], "1.0.0");

    let agents = require_cli(&data_dir, &["agent", "list"]);
    let agent = agents["agents"]
        .as_array()
        .expect("agents")
        .iter()
        .find(|agent| agent["id"] == "agent_plugin_probe")
        .expect("imported agent");
    assert_eq!(agent["status"], "imported");
    assert_eq!(agent["latestVersion"], "1.0.0");

    let sbom = &installed["sbom"];
    assert_eq!(sbom["schemaVersion"], "hpa.sbom/v1");
    assert_eq!(sbom["gates"]["license"], true);
    assert_eq!(sbom["gates"]["signature"], true);
    assert_eq!(sbom["gates"]["staticScan"], "pass");
    let files = sbom["files"].as_array().expect("sbom files");
    assert!(files.iter().any(|file| file["path"] == MANIFEST_FILE));
    for file in files {
        let sha = file["sha256"].as_str().expect("file sha256");
        assert_eq!(sha.len(), 64, "sha256: {sha}");
        assert!(sha.chars().all(|character| character.is_ascii_hexdigit()));
    }

    let config = json!({
        "baseUrl": "http://127.0.0.1:9",
        "allowedHosts": [],
        "auth": {"kind": "none"}
    })
    .to_string();
    let connection = require_cli(
        &data_dir,
        &[
            "connector",
            "add",
            "Lifecycle HTTP",
            "plugin_http",
            "--config",
            &config,
        ],
    );
    let connection_id = connection["id"]
        .as_str()
        .expect("connection id")
        .to_string();
    assert_eq!(connection["connectorId"], "plugin_http");

    let disabled = require_cli(&data_dir, &["plugin", "disable", "plugin_lifecycle"]);
    assert_eq!(disabled["plugin"]["status"], "disabled");
    for capability in disabled["capabilities"].as_array().expect("capabilities") {
        assert_eq!(capability["status"], "disabled");
    }
    let definitions = require_cli(&data_dir, &["connector", "list"]);
    let definition = definitions["connectors"]
        .as_array()
        .expect("connector definitions")
        .iter()
        .find(|definition| definition["pluginId"] == "plugin_lifecycle")
        .expect("plugin definition");
    assert_eq!(definition["status"], "disabled");
    let stderr = failure_cli(&data_dir, &["connector", "test", &connection_id]);
    assert!(
        stderr.contains("disabled"),
        "unexpected test stderr: {stderr}"
    );

    let enabled = require_cli(&data_dir, &["plugin", "enable", "plugin_lifecycle"]);
    assert_eq!(enabled["plugin"]["status"], "enabled");
    for capability in enabled["capabilities"].as_array().expect("capabilities") {
        assert_eq!(capability["status"], "enabled");
    }
    let definitions = require_cli(&data_dir, &["connector", "list"]);
    let definition = definitions["connectors"]
        .as_array()
        .expect("connector definitions")
        .iter()
        .find(|definition| definition["pluginId"] == "plugin_lifecycle")
        .expect("plugin definition");
    assert_eq!(definition["status"], "enabled");

    let removed = require_cli(
        &data_dir,
        &["plugin", "uninstall", "plugin_lifecycle", "--force"],
    );
    assert_eq!(removed["plugin"]["status"], "uninstalled");
    assert!(removed["plugin"]["installedVersion"].is_null());
    assert!(
        removed["capabilities"]
            .as_array()
            .expect("capabilities")
            .is_empty()
    );
    let definitions = require_cli(&data_dir, &["connector", "list"]);
    assert!(
        !definitions["connectors"]
            .as_array()
            .expect("connector definitions")
            .iter()
            .any(|definition| definition["pluginId"] == "plugin_lifecycle"),
        "plugin connector definitions should be removed: {definitions}"
    );
    let stderr = failure_cli(&data_dir, &["connector", "test", &connection_id]);
    assert!(
        stderr.contains("not found"),
        "unexpected test stderr: {stderr}"
    );
}

#[test]
fn unsigned_plugin_requires_allow_unsigned() {
    let data_dir = temp_dir("unsigned");
    let package = data_dir.join("unsigned.hplugin");
    unsigned_package(
        plugin_files(
            "plugin_unsigned",
            "1.0.0",
            "Apache-2.0",
            "Unsigned manifest with integrity.",
        ),
        &package,
    );
    let package = package.to_str().expect("package path");

    let stderr = failure_cli(&data_dir, &["plugin", "install", package]);
    assert!(
        stderr.contains("unsigned") && stderr.contains("allowUnsigned"),
        "unexpected unsigned stderr: {stderr}"
    );

    let installed = require_cli(
        &data_dir,
        &["plugin", "install", package, "--allow-unsigned"],
    );
    assert_eq!(installed["plugin"]["status"], "enabled");
    assert_eq!(installed["plugin"]["trust"], "untrusted");
    assert_eq!(installed["plugin"]["installedVersion"], "1.0.0");
}

#[test]
fn tampered_plugin_content_fails_integrity() {
    let data_dir = temp_dir("tamper");
    let source = temp_dir("tamper-src");
    write_files(
        &source,
        &plugin_files(
            "plugin_tamper",
            "1.0.0",
            "Apache-2.0",
            "Fixture used to detect tampering.",
        ),
    );
    require_cli(
        &data_dir,
        &["plugin", "sign", source.to_str().expect("source path")],
    );
    std::fs::write(
        source.join("skills/plugin-guide/SKILL.md"),
        b"---\nname: plugin-guide\ndescription: tampered after signing\nlicense: Apache-2.0\n---\n\nTampered.\n",
    )
    .expect("tamper skill");

    let package = data_dir.join("tamper-pack.hplugin");
    let stderr = failure_cli(
        &data_dir,
        &[
            "plugin",
            "pack",
            source.to_str().expect("source path"),
            package.to_str().expect("package path"),
        ],
    );
    assert!(
        stderr.contains("integrity digest does not match"),
        "unexpected pack stderr: {stderr}"
    );

    let fresh = temp_dir("tamper-fresh-src");
    write_files(
        &fresh,
        &plugin_files(
            "plugin_tamper",
            "1.0.0",
            "Apache-2.0",
            "Fixture used to detect tampering.",
        ),
    );
    let valid = data_dir.join("tamper-valid.hplugin");
    sign_and_pack(&data_dir, &fresh, &valid);

    let mut files = read_files(&fresh);
    files.insert(
        "skills/plugin-guide/SKILL.md".to_string(),
        b"tampered archive content".to_vec(),
    );
    let content_package = data_dir.join("tamper-content.hplugin");
    pack_tar(&files, &content_package);
    let stderr = failure_cli(
        &data_dir,
        &[
            "plugin",
            "install",
            content_package.to_str().expect("package"),
        ],
    );
    assert!(
        stderr.contains("integrity digest does not match"),
        "unexpected content install stderr: {stderr}"
    );
}

#[test]
fn tampered_signed_manifest_fails_signature_verification() {
    let data_dir = temp_dir("manifest-tamper");
    let source = temp_dir("manifest-tamper-src");
    write_files(
        &source,
        &plugin_files(
            "plugin_manifest_tamper",
            "1.0.0",
            "Apache-2.0",
            "Fixture used to detect manifest tampering.",
        ),
    );
    let valid = data_dir.join("manifest-valid.hplugin");
    sign_and_pack(&data_dir, &source, &valid);

    let mut files = read_files(&source);
    let mut manifest: Value =
        serde_json::from_slice(files.get(MANIFEST_FILE).expect("signed manifest"))
            .expect("manifest json");
    manifest["name"] = json!("Tampered after signing");
    files.insert(
        MANIFEST_FILE.to_string(),
        serde_json::to_vec_pretty(&manifest).expect("manifest bytes"),
    );
    let package = data_dir.join("manifest-tampered.hplugin");
    pack_tar(&files, &package);
    let stderr = failure_cli(
        &data_dir,
        &["plugin", "install", package.to_str().expect("package")],
    );
    assert!(
        stderr.contains("signature verification failed"),
        "unexpected manifest install stderr: {stderr}"
    );
}

#[test]
fn proprietary_license_is_denied_by_the_license_gate() {
    let data_dir = temp_dir("license");
    let source = temp_dir("license-src");
    write_files(
        &source,
        &plugin_files(
            "plugin_proprietary",
            "1.0.0",
            "Proprietary",
            "Fixture with a license outside the allowlist.",
        ),
    );
    require_cli(
        &data_dir,
        &["plugin", "sign", source.to_str().expect("source path")],
    );
    let package = data_dir.join("proprietary.hplugin");
    let stderr = failure_cli(
        &data_dir,
        &[
            "plugin",
            "pack",
            source.to_str().expect("source path"),
            package.to_str().expect("package path"),
        ],
    );
    assert!(
        stderr.contains("Proprietary") && stderr.contains("allowlist"),
        "unexpected license stderr: {stderr}"
    );
}

#[test]
fn plugin_archive_path_traversal_is_rejected() {
    let data_dir = temp_dir("traversal");
    let package = data_dir.join("escape.hplugin");
    let mut archive = Vec::new();
    append_raw_tar_entry(&mut archive, MANIFEST_FILE, b"{}");
    append_raw_tar_entry(&mut archive, "../escape", b"owned");
    archive.extend_from_slice(&[0u8; 1024]);
    std::fs::write(&package, archive).expect("write traversal package");

    let stderr = failure_cli(
        &data_dir,
        &["plugin", "install", package.to_str().expect("package")],
    );
    assert!(
        stderr.contains("invalid package path") && stderr.contains("../escape"),
        "unexpected traversal stderr: {stderr}"
    );
}

#[test]
fn plugin_upgrade_and_rollback_track_versions() {
    let data_dir = temp_dir("upgrade");
    let mut packages = Vec::new();
    for version in ["1.0.0", "1.1.0"] {
        let source = temp_dir(&format!("upgrade-{version}-src"));
        write_files(
            &source,
            &plugin_files(
                "plugin_upgrade",
                version,
                "Apache-2.0",
                "Upgrade fixture with a changed description.",
            ),
        );
        let package = data_dir.join(format!("plugin_upgrade-{version}.hplugin"));
        sign_and_pack(&data_dir, &source, &package);
        packages.push(package);
    }

    let first = require_cli(
        &data_dir,
        &["plugin", "install", packages[0].to_str().expect("v1")],
    );
    assert_eq!(first["plugin"]["installedVersion"], "1.0.0");
    assert!(first["plugin"]["previousVersion"].is_null());

    let second = require_cli(
        &data_dir,
        &["plugin", "install", packages[1].to_str().expect("v2")],
    );
    assert_eq!(second["plugin"]["installedVersion"], "1.1.0");
    assert_eq!(second["plugin"]["previousVersion"], "1.0.0");

    let definitions = require_cli(&data_dir, &["connector", "list"]);
    let definitions = definitions["connectors"].as_array().expect("definitions");
    let current = definitions
        .iter()
        .find(|definition| {
            definition["pluginId"] == "plugin_upgrade" && definition["version"] == "1.1.0"
        })
        .expect("1.1.0 connector");
    assert_eq!(current["status"], "enabled");
    let previous = definitions
        .iter()
        .find(|definition| {
            definition["pluginId"] == "plugin_upgrade" && definition["version"] == "1.0.0"
        })
        .expect("1.0.0 connector");
    assert_eq!(previous["status"], "disabled");

    let rolled_back = require_cli(&data_dir, &["plugin", "rollback", "plugin_upgrade"]);
    assert_eq!(rolled_back["plugin"]["status"], "enabled");
    assert_eq!(rolled_back["plugin"]["installedVersion"], "1.0.0");
    assert_eq!(rolled_back["plugin"]["previousVersion"], "1.1.0");
    let definitions = require_cli(&data_dir, &["connector", "list"]);
    let definitions = definitions["connectors"].as_array().expect("definitions");
    let restored = definitions
        .iter()
        .find(|definition| {
            definition["pluginId"] == "plugin_upgrade" && definition["version"] == "1.0.0"
        })
        .expect("restored connector");
    assert_eq!(restored["status"], "enabled");
    let superseded = definitions
        .iter()
        .find(|definition| {
            definition["pluginId"] == "plugin_upgrade" && definition["version"] == "1.1.0"
        })
        .expect("superseded connector");
    assert_eq!(superseded["status"], "disabled");
}

#[test]
fn marketplace_review_rate_install_and_withdraw() {
    let data_dir = temp_dir("marketplace");
    let source = temp_dir("marketplace-src");
    write_files(
        &source,
        &plugin_files(
            "plugin_market",
            "1.0.0",
            "Apache-2.0",
            "Marketplace fixture.",
        ),
    );
    let package = data_dir.join("plugin_market-1.0.0.hplugin");
    sign_and_pack(&data_dir, &source, &package);

    let published = require_cli(
        &data_dir,
        &["plugin", "publish", package.to_str().expect("package")],
    );
    assert_eq!(published["pluginId"], "plugin_market");
    assert_eq!(published["reviewStatus"], "pending");

    let stderr = failure_cli(
        &data_dir,
        &["plugin", "install-marketplace", "plugin_market", "1.0.0"],
    );
    assert!(
        stderr.contains("not approved"),
        "unexpected pre-review install stderr: {stderr}"
    );

    let approved = require_cli(
        &data_dir,
        &[
            "plugin",
            "review",
            "plugin_market",
            "1.0.0",
            "--status",
            "approved",
        ],
    );
    assert_eq!(approved["reviewStatus"], "approved");

    let rated = require_cli(
        &data_dir,
        &[
            "plugin",
            "rate",
            "plugin_market",
            "1.0.0",
            "5",
            "--comment",
            "works",
        ],
    );
    assert_eq!(rated["rating"]["count"], 1);
    assert_eq!(rated["rating"]["average"].as_f64(), Some(5.0));
    let rated = require_cli(
        &data_dir,
        &["plugin", "rate", "plugin_market", "1.0.0", "3"],
    );
    assert_eq!(rated["rating"]["count"], 2);
    assert_eq!(rated["rating"]["average"].as_f64(), Some(4.0));
    let stderr = failure_cli(
        &data_dir,
        &["plugin", "rate", "plugin_market", "1.0.0", "0"],
    );
    assert!(
        stderr.contains("rating must be 1..5"),
        "unexpected rating stderr: {stderr}"
    );

    let installed = require_cli(
        &data_dir,
        &["plugin", "install-marketplace", "plugin_market", "1.0.0"],
    );
    assert_eq!(installed["plugin"]["status"], "enabled");
    assert_eq!(installed["plugin"]["installedVersion"], "1.0.0");
    assert_eq!(installed["plugin"]["trust"], "verified");

    let listing = require_cli(&data_dir, &["plugin", "marketplace"]);
    let listing = listing["plugins"]
        .as_array()
        .expect("marketplace listings")
        .iter()
        .find(|listing| listing["pluginId"] == "plugin_market")
        .expect("marketplace listing");
    assert_eq!(listing["downloads"], 1);

    let withdrawn = require_cli(&data_dir, &["plugin", "withdraw", "plugin_market", "1.0.0"]);
    assert_eq!(withdrawn["reviewStatus"], "withdrawn");
    let stderr = failure_cli(
        &data_dir,
        &["plugin", "install-marketplace", "plugin_market", "1.0.0"],
    );
    assert!(
        stderr.contains("withdrawn") || stderr.contains("not approved"),
        "unexpected post-withdraw install stderr: {stderr}"
    );
}

#[tokio::test]
async fn connector_http_rate_limit_retry_and_ssrf() {
    let data_dir = temp_dir("connector-http");
    let mock = spawn_mock_http();
    let server = Server::start(&data_dir).await;
    let base_url = format!("http://127.0.0.1:{}", mock.port);

    let (status, connection) = server
        .post(
            "/api/connections",
            json!({
                "name": "Mock HTTP",
                "connectorId": "http",
                "connectorVersion": "1.0.0",
                "config": connection_config(&base_url, 2, 1)
            }),
        )
        .await;
    assert_eq!(status, 200, "create connection: {connection}");
    let connection_id = connection["id"]
        .as_str()
        .expect("connection id")
        .to_string();

    let (status, tested) = server
        .post(&format!("/api/connections/{connection_id}/test"), json!({}))
        .await;
    assert_eq!(status, 200, "test connection: {tested}");
    assert_eq!(tested["test"]["ok"], true);
    assert_eq!(tested["test"]["status"], 200);

    let (status, first) = server
        .post(
            &format!("/api/connections/{connection_id}/invoke"),
            json!({"operation": "request", "arguments": {"method": "GET", "path": "/"}}),
        )
        .await;
    assert_eq!(status, 200, "invoke: {first}");
    assert_eq!(first["result"]["status"], 200);
    assert_eq!(first["result"]["body"]["message"], "pong");

    let (status, limited) = server
        .post(
            &format!("/api/connections/{connection_id}/invoke"),
            json!({"operation": "request", "arguments": {"method": "GET", "path": "/"}}),
        )
        .await;
    assert_eq!(status, 429, "rate limited: {limited}");
    assert_eq!(limited["error"]["code"], "CONNECTOR_RATE_LIMIT");

    let (status, retry_connection) = server
        .post(
            "/api/connections",
            json!({
                "name": "Flaky HTTP",
                "connectorId": "http",
                "connectorVersion": "1.0.0",
                "config": connection_config(&base_url, 2, 100)
            }),
        )
        .await;
    assert_eq!(status, 200, "create retry connection: {retry_connection}");
    let retry_id = retry_connection["id"]
        .as_str()
        .expect("retry connection id")
        .to_string();
    let (status, retried) = server
        .post(
            &format!("/api/connections/{retry_id}/invoke"),
            json!({"operation": "request", "arguments": {"method": "GET", "path": "/flaky"}}),
        )
        .await;
    assert_eq!(status, 200, "retry invoke: {retried}");
    assert_eq!(retried["result"]["attempts"], 2);
    assert_eq!(retried["result"]["body"]["flaky"], true);
    assert_eq!(mock.flaky_hits.load(Ordering::SeqCst), 2);

    let (status, ssrf) = server
        .post(
            "/api/connections",
            json!({
                "name": "SSRF HTTP",
                "connectorId": "http",
                "connectorVersion": "1.0.0",
                "config": {
                    "baseUrl": "http://example.com",
                    "allowedHosts": [],
                    "auth": {"kind": "none"}
                }
            }),
        )
        .await;
    assert_eq!(status, 200, "create SSRF connection: {ssrf}");
    let ssrf_id = ssrf["id"].as_str().expect("ssrf connection id").to_string();
    let (status, denied) = server
        .post(&format!("/api/connections/{ssrf_id}/test"), json!({}))
        .await;
    assert_eq!(status, 403, "ssrf test: {denied}");
    assert_eq!(denied["error"]["code"], "CONNECTOR_FORBIDDEN");
    assert!(
        denied["error"]["message"]
            .as_str()
            .expect("message")
            .contains("allowedHosts"),
        "unexpected ssrf message: {denied}"
    );
    let (status, denied) = server
        .post(
            &format!("/api/connections/{ssrf_id}/invoke"),
            json!({"operation": "request", "arguments": {"method": "GET", "path": "/"}}),
        )
        .await;
    assert_eq!(status, 403, "ssrf invoke: {denied}");
    assert!(
        denied["error"]["message"]
            .as_str()
            .expect("message")
            .contains("allowedHosts"),
        "unexpected ssrf invoke message: {denied}"
    );
}

#[test]
fn file_connector_enforces_workspace_boundaries() {
    let data_dir = temp_dir("file-connector");
    let sample = require_cli(&data_dir, &["connector", "sample"]);
    let connection_id = sample["id"].as_str().expect("connection id").to_string();
    let root = PathBuf::from(sample["config"]["root"].as_str().expect("root"));
    assert_eq!(sample["config"]["allowWrite"], true);

    let read = require_cli(
        &data_dir,
        &[
            "connector",
            "invoke",
            &connection_id,
            "read",
            "--args",
            &json!({"path": "welcome.txt"}).to_string(),
        ],
    );
    assert!(
        read["content"]
            .as_str()
            .expect("content")
            .contains("hpa connector workspace")
    );

    let written = require_cli(
        &data_dir,
        &[
            "connector",
            "invoke",
            &connection_id,
            "write",
            "--args",
            &json!({"path": "notes/hello.txt", "content": "hello from the file connector"})
                .to_string(),
        ],
    );
    assert_eq!(written["bytes"], 29);
    let read_back = require_cli(
        &data_dir,
        &[
            "connector",
            "invoke",
            &connection_id,
            "read",
            "--args",
            &json!({"path": "notes/hello.txt"}).to_string(),
        ],
    );
    assert_eq!(read_back["content"], "hello from the file connector");

    let stderr = failure_cli(
        &data_dir,
        &[
            "connector",
            "invoke",
            &connection_id,
            "read",
            "--args",
            &json!({"path": "../escape"}).to_string(),
        ],
    );
    assert!(
        stderr.contains("path traversal"),
        "unexpected traversal stderr: {stderr}"
    );

    #[cfg(unix)]
    {
        let secret = data_dir.join("outside-secret.txt");
        std::fs::write(&secret, "outside the connector root").expect("secret");
        let link = root.join("escape-link.txt");
        std::os::unix::fs::symlink(&secret, &link).expect("symlink");
        let stderr = failure_cli(
            &data_dir,
            &[
                "connector",
                "invoke",
                &connection_id,
                "read",
                "--args",
                &json!({"path": "escape-link.txt"}).to_string(),
            ],
        );
        assert!(
            stderr.contains("path escapes the connection root"),
            "unexpected symlink read stderr: {stderr}"
        );
        let stderr = failure_cli(
            &data_dir,
            &[
                "connector",
                "invoke",
                &connection_id,
                "write",
                "--args",
                &json!({"path": "escape-link.txt", "content": "overwritten"}).to_string(),
            ],
        );
        assert!(
            stderr.contains("symlink"),
            "unexpected symlink write stderr: {stderr}"
        );
    }
}

#[tokio::test]
async fn agent_connector_tool_requires_independent_approval() {
    let data_dir = temp_dir("agent-connector");
    let workspace = temp_dir("agent-connector-workspace");
    std::fs::write(workspace.join("welcome.txt"), "agent connector content\n")
        .expect("workspace file");
    let server = Server::start(&data_dir).await;

    let (status, connection) = server
        .post(
            "/api/connections",
            json!({
                "name": "Agent Files",
                "connectorId": "file",
                "connectorVersion": "1.0.0",
                "config": {"root": workspace, "allowWrite": false}
            }),
        )
        .await;
    assert_eq!(status, 200, "create file connection: {connection}");
    let connection_id = connection["id"]
        .as_str()
        .expect("connection id")
        .to_string();
    let tool = format!("connector.{connection_id}.read");

    let config = json!({
        "schemaVersion": "hpa.agent/v1",
        "id": "agent_connector_probe",
        "name": "Connector Probe",
        "version": "1.0.0",
        "tools": [tool.clone()],
        "planner": {
            "kind": "deterministic",
            "plan": [
                {"kind": "tool", "tool": tool.clone(), "arguments": {"path": "welcome.txt"}},
                {"kind": "respond", "output": {"$last": true}}
            ]
        },
        "subAgents": []
    });
    let (status, created) = server.post("/api/agents", json!({"config": config})).await;
    assert_eq!(status, 200, "create agent: {created}");
    let (status, published) = server
        .post("/api/agents/agent_connector_probe/publish", json!({}))
        .await;
    assert_eq!(status, 200, "publish agent: {published}");

    let (status, run) = server
        .post(
            "/api/agents/agent_connector_probe/run",
            json!({"input": {}}),
        )
        .await;
    assert_eq!(status, 200, "run agent: {run}");
    assert_eq!(run["status"], "waiting_approval", "run: {run}");
    assert!(run["pendingApproval"].is_string(), "run: {run}");

    let (status, approvals) = server.get("/api/agents/approvals?status=pending").await;
    assert_eq!(status, 200);
    let approval = approvals["approvals"]
        .as_array()
        .expect("approvals")
        .iter()
        .find(|approval| approval["sessionId"] == run["id"] && approval["toolName"] == tool)
        .unwrap_or_else(|| panic!("missing connector approval: {approvals}"));
    let approval_id = approval["id"].as_str().expect("approval id").to_string();

    let (status, decided) = server
        .post(
            &format!("/api/agents/approvals/{approval_id}/decide"),
            json!({"decision": "approved"}),
        )
        .await;
    assert_eq!(status, 200, "decide approval: {decided}");
    let session = &decided["session"];
    assert_eq!(session["status"], "succeeded", "session: {session}");
    assert_eq!(
        session["output"]["content"], "agent connector content\n",
        "session: {session}"
    );
}
