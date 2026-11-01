use std::fs;
use std::path::{Path, PathBuf};

use hpa_skill::{Severity, SkillError, export_snapshot, inspect, publish_issues};
use serde_json::{Value, json};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/skills")
}

fn temp_dir(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("hpa-skill-{label}-{}", uuid::Uuid::now_v7()));
    fs::create_dir_all(&path).expect("create temp dir");
    path
}

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, content).expect("write file");
}

fn skill_md(name: &str, extra: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: Test skill for {name}.{extra}\n---\n\n# {name}\n\nRun the deterministic entry.\n"
    )
}

fn manifest(name: &str, script_path: &str) -> Value {
    json!({
        "schemaVersion": "hpa.skill/v1",
        "id": format!("skill_{}", name.replace('-', "_")),
        "name": name,
        "version": "1.0.0",
        "license": "MIT",
        "entry": {"kind": "script"},
        "scripts": [{
            "id": "run",
            "path": script_path,
            "runtime": "python3",
            "args": ["--input", "${inputFile}", "--output", "${artifactDir}"],
            "network": "none"
        }],
        "permissions": {"network": [], "commands": ["python3"]},
        "tests": [{"name": "smoke", "script": "run", "input": {}, "expect": {"exitCode": 0}}]
    })
}

#[test]
fn semver_requirements_match_dependencies() {
    use hpa_skill::version_satisfies;
    assert!(version_satisfies("1.2.3", "^1.0.0"));
    assert!(!version_satisfies("1.9.0", "~1.2.0"));
    assert!(version_satisfies("1.2.9", "~1.2.0"));
    assert!(version_satisfies("2.0.0", ">=2.0.0,<3.0.0"));
    assert!(!version_satisfies("3.0.0", ">=2.0.0,<3.0.0"));
    assert!(version_satisfies("1.0.0", "*"));
    assert!(!version_satisfies("1.0", "^1.0.0"));
    assert!(version_satisfies("0.3.2", "^0.3.0"));
    assert!(!version_satisfies("0.4.0", "^0.3.0"));
}

#[test]
fn valid_skill_preserves_unknown_fields() {
    let inspection = inspect(&fixtures().join("invoice-audit")).expect("inspect fixture");
    assert!(
        inspection.report.is_valid(),
        "unexpected issues: {:?}",
        inspection.report.issues
    );
    assert_eq!(inspection.snapshot.id, "skill_invoice_audit");
    assert_eq!(inspection.snapshot.version, "1.0.0");
    assert_eq!(inspection.snapshot.license.as_deref(), Some("Apache-2.0"));
    assert_eq!(inspection.scripts.len(), 1);
    assert_eq!(inspection.tests.len(), 1);
    assert!(inspection.entry.deterministic());
    assert_eq!(
        inspection.snapshot.manifest["unknownTopLevel"]["keep"],
        json!(true)
    );
    assert_eq!(
        inspection.snapshot.manifest["scripts"][0]["unknownScriptField"],
        json!("preserve-me")
    );
    assert_eq!(inspection.snapshot.content_hash.len(), 64);
    assert!(
        inspection
            .snapshot
            .files
            .iter()
            .any(|file| file.path == "NOTICE")
    );
    assert_eq!(
        inspection.frontmatter.allowed_tools,
        Some(vec!["python3".to_string()])
    );
    assert!(publish_issues(&inspection).is_empty());
}

#[test]
fn export_roundtrip_keeps_files_manifest_and_hash() {
    let inspection = inspect(&fixtures().join("invoice-audit")).expect("inspect fixture");
    let target = temp_dir("export");
    export_snapshot(&inspection.snapshot, &target).expect("export snapshot");

    let exported = inspect(&target).expect("inspect exported skill");
    assert!(exported.report.is_valid(), "{:?}", exported.report.issues);
    assert_eq!(
        exported.snapshot.content_hash,
        inspection.snapshot.content_hash
    );
    assert_eq!(exported.snapshot.id, inspection.snapshot.id);
    assert_eq!(exported.snapshot.version, inspection.snapshot.version);
    let raw: Value = serde_json::from_str(
        &fs::read_to_string(target.join("hpa.skill.json")).expect("read manifest"),
    )
    .expect("parse manifest");
    assert_eq!(raw["unknownTopLevel"]["keep"], json!(true));
    assert_eq!(
        raw["scripts"][0]["unknownScriptField"],
        json!("preserve-me")
    );
    assert!(target.join("NOTICE").is_file());
    assert!(target.join("references/policy.md").is_file());
    fs::remove_dir_all(&target).expect("cleanup export");
}

#[test]
fn proprietary_license_blocks_publish_and_export() {
    let inspection = inspect(&fixtures().join("proprietary-demo")).expect("inspect fixture");
    assert!(
        inspection.report.is_valid(),
        "{:?}",
        inspection.report.issues
    );
    let issues = publish_issues(&inspection);
    assert!(
        issues
            .iter()
            .any(|issue| issue.code == "SKILL_LICENSE_DENIED" && issue.severity == Severity::Error),
        "{issues:?}"
    );
    let target = temp_dir("proprietary-export");
    let error = export_snapshot(&inspection.snapshot, &target).expect_err("export must fail");
    assert!(matches!(error, SkillError::LicenseDenied(_)), "{error:?}");
    fs::remove_dir_all(&target).expect("cleanup");
}

#[test]
fn missing_license_blocks_export() {
    let root = temp_dir("missing-license");
    write(&root.join("SKILL.md"), &skill_md("missing-license", ""));
    let inspection = inspect(&root).expect("inspect");
    assert!(
        inspection.report.is_valid(),
        "{:?}",
        inspection.report.issues
    );
    let target = temp_dir("missing-license-export");
    let error = export_snapshot(&inspection.snapshot, &target).expect_err("export must fail");
    assert!(matches!(error, SkillError::LicenseDenied(_)), "{error:?}");
    fs::remove_dir_all(&root).expect("cleanup");
    fs::remove_dir_all(&target).expect("cleanup");
}

#[test]
fn script_path_escape_and_missing_files_are_rejected() {
    let root = temp_dir("escape");
    write(&root.join("SKILL.md"), &skill_md("escape-skill", ""));
    write(
        &root.join("hpa.skill.json"),
        &manifest("escape-skill", "scripts/../../evil.py").to_string(),
    );
    let inspection = inspect(&root).expect("inspect");
    assert!(
        inspection.report.has_code("SKILL_SCRIPT_PATH"),
        "{:?}",
        inspection.report.issues
    );

    write(
        &root.join("hpa.skill.json"),
        &manifest("escape-skill", "scripts/missing.py").to_string(),
    );
    let inspection = inspect(&root).expect("inspect");
    assert!(
        inspection.report.has_code("SKILL_SCRIPT_MISSING"),
        "{:?}",
        inspection.report.issues
    );
    fs::remove_dir_all(&root).expect("cleanup");
}

#[cfg(unix)]
#[test]
fn symbolic_links_are_rejected() {
    use std::os::unix::fs::symlink;

    let root = temp_dir("symlink");
    write(&root.join("SKILL.md"), &skill_md("symlink-skill", ""));
    write(
        &root.join("hpa.skill.json"),
        &manifest("symlink-skill", "scripts/run.py").to_string(),
    );
    fs::create_dir_all(root.join("scripts")).expect("scripts dir");
    symlink("/etc/passwd", root.join("scripts/run.py")).expect("symlink");
    let inspection = inspect(&root).expect("inspect");
    assert!(
        inspection.report.has_code("SKILL_PATH_SYMLINK"),
        "{:?}",
        inspection.report.issues
    );
    fs::remove_dir_all(&root).expect("cleanup");
}

#[test]
fn missing_frontmatter_and_network_permissions_are_rejected() {
    let root = temp_dir("invalid");
    write(
        &root.join("SKILL.md"),
        "# No frontmatter\n\nThis file has no YAML block.\n",
    );
    let inspection = inspect(&root).expect("inspect");
    assert!(inspection.report.has_code("SKILL_FRONTMATTER"));
    assert!(inspection.report.has_code("SKILL_NAME"));
    fs::remove_dir_all(&root).expect("cleanup");

    let root = temp_dir("network");
    write(&root.join("SKILL.md"), &skill_md("network-skill", ""));
    let mut value = manifest("network-skill", "scripts/run.py");
    value["permissions"]["network"] = json!(["api.example.com:443"]);
    write(&root.join("hpa.skill.json"), &value.to_string());
    write(&root.join("scripts/run.py"), "print('{}')\n");
    let inspection = inspect(&root).expect("inspect");
    assert!(
        inspection.report.has_code("SKILL_PERMISSION"),
        "{:?}",
        inspection.report.issues
    );
    fs::remove_dir_all(&root).expect("cleanup");
}

#[test]
fn allowed_tools_string_is_normalized() {
    let root = temp_dir("allowed-tools");
    write(
        &root.join("SKILL.md"),
        &skill_md(
            "allowed-tools-demo",
            "\nlicense: MIT\nallowed-tools: python3, node",
        ),
    );
    let inspection = inspect(&root).expect("inspect");
    assert!(
        inspection.report.is_valid(),
        "{:?}",
        inspection.report.issues
    );
    assert_eq!(
        inspection.frontmatter.allowed_tools,
        Some(vec!["python3".to_string(), "node".to_string()])
    );
    fs::remove_dir_all(&root).expect("cleanup");
}

#[test]
fn oversized_script_args_and_invalid_schema_are_rejected() {
    let root = temp_dir("args");
    write(&root.join("SKILL.md"), &skill_md("args-skill", ""));
    let mut value = manifest("args-skill", "scripts/run.py");
    value["scripts"][0]["args"] = json!(vec!["x".repeat(5000)]);
    value["scripts"][0]["outputSchema"] = json!({"type": 42});
    write(&root.join("hpa.skill.json"), &value.to_string());
    write(&root.join("scripts/run.py"), "print('{}')\n");
    let inspection = inspect(&root).expect("inspect");
    assert!(inspection.report.has_code("SKILL_SCRIPT_ARGS"));
    assert!(inspection.report.has_code("SKILL_SCHEMA"));
    fs::remove_dir_all(&root).expect("cleanup");
}
