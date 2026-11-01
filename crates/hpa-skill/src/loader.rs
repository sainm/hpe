use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::error::{SkillError, SkillIssue, SkillReport, SkillResult};
use crate::manifest::{
    EntryKind, SKILL_SCHEMA_VERSION, SkillFile, SkillFrontmatter, SkillInspection, SkillManifest,
    SkillSnapshot, allowed_license, denied_license, derive_skill_id, valid_semver, valid_skill_id,
    valid_skill_name,
};

pub const MAX_FILES: usize = 500;
pub const MAX_TOTAL_BYTES: u64 = 20 * 1024 * 1024;
pub const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
pub const MAX_DEPTH: usize = 8;
pub const MAX_SCRIPT_ARGS: usize = 32;
pub const MAX_ARG_BYTES: usize = 4096;

const SAFE_ENV: &[&str] = &["LANG", "LC_ALL", "TZ", "NO_COLOR"];
const RUNTIMES: &[&str] = &["python3", "python", "node", "bash", "sh"];

pub fn inspect(root: &Path) -> SkillResult<SkillInspection> {
    let root = root
        .canonicalize()
        .map_err(|_| SkillError::NotFound(root.display().to_string()))?;
    let metadata = fs::metadata(&root)?;
    if !metadata.is_dir() {
        return Err(SkillError::invalid(
            "SKILL_NOT_DIRECTORY",
            format!("`{}` is not a directory", root.display()),
        ));
    }

    let mut report = SkillReport::default();
    let mut files = Vec::new();
    collect_files(&root, "", 0, &mut files, &mut report)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    check_limits(&files, &mut report);

    let Some(skill_md_file) = files.iter().find(|file| file.path == "SKILL.md") else {
        report.push(
            SkillIssue::error("SKILL_MD_MISSING", "SKILL.md", "SKILL.md is required")
                .with_hint("Add SKILL.md with YAML frontmatter and instructions."),
        );
        let snapshot = SkillSnapshot {
            id: derive_skill_id(&root_name(&root)),
            name: root_name(&root),
            version: "0.1.0".to_string(),
            license: None,
            manifest: json!({}),
            manifest_present: false,
            skill_md: String::new(),
            content_hash: content_hash(&files),
            files,
        };
        return Ok(SkillInspection {
            root,
            snapshot,
            frontmatter: SkillFrontmatter::default(),
            scripts: Vec::new(),
            tests: Vec::new(),
            permissions: Default::default(),
            entry: EntryKind::Instructions,
            report,
        });
    };

    let skill_md = String::from_utf8_lossy(&skill_md_file.content).to_string();
    let (frontmatter, instructions) = parse_frontmatter(&skill_md, &mut report);
    validate_frontmatter(&frontmatter, &instructions, &mut report);

    let manifest_file = files.iter().find(|file| file.path == "hpa.skill.json");
    let (manifest_raw, manifest_present) = match manifest_file {
        Some(file) => match serde_json::from_slice::<Value>(&file.content) {
            Ok(value @ Value::Object(_)) => (value, true),
            Ok(_) => {
                report.push(SkillIssue::error(
                    "SKILL_MANIFEST",
                    "hpa.skill.json",
                    "manifest must be a JSON object",
                ));
                (json!({}), true)
            }
            Err(error) => {
                report.push(SkillIssue::error(
                    "SKILL_MANIFEST",
                    "hpa.skill.json",
                    format!("invalid JSON: {error}"),
                ));
                (json!({}), true)
            }
        },
        None => (json!({}), false),
    };

    let manifest = match serde_json::from_value::<SkillManifest>(manifest_raw.clone()) {
        Ok(manifest) => manifest,
        Err(error) => {
            report.push(SkillIssue::error(
                "SKILL_MANIFEST",
                "hpa.skill.json",
                format!("manifest shape is invalid: {error}"),
            ));
            SkillManifest::default()
        }
    };

    if manifest_present {
        match manifest.schema_version.as_deref() {
            Some(SKILL_SCHEMA_VERSION) => {}
            Some(other) => report.push(SkillIssue::error(
                "SKILL_MANIFEST_VERSION",
                "hpa.skill.json.schemaVersion",
                format!("expected `{SKILL_SCHEMA_VERSION}`, found `{other}`"),
            )),
            None => report.push(SkillIssue::error(
                "SKILL_MANIFEST_VERSION",
                "hpa.skill.json.schemaVersion",
                format!("schemaVersion `{SKILL_SCHEMA_VERSION}` is required"),
            )),
        }
    }

    let name = manifest
        .name
        .clone()
        .or(frontmatter.name.clone())
        .unwrap_or_else(|| root_name(&root));
    let mut id = manifest
        .id
        .clone()
        .unwrap_or_else(|| derive_skill_id(&name));
    if manifest.id.is_none() {
        report.push(SkillIssue::warning(
            "SKILL_ID_DERIVED",
            "hpa.skill.json.id",
            format!("id was derived as `{id}`; add an explicit id before publishing"),
        ));
    }
    if !valid_skill_id(&id) {
        report.push(SkillIssue::error(
            "SKILL_ID",
            "hpa.skill.json.id",
            format!("invalid skill id `{id}`"),
        ));
        id = derive_skill_id(&name);
    }
    if manifest_present && manifest.name.is_none() {
        report.push(SkillIssue::error(
            "SKILL_NAME",
            "hpa.skill.json.name",
            "manifest name is required",
        ));
    }
    if let Some(manifest_name) = &manifest.name
        && let Some(frontmatter_name) = &frontmatter.name
        && manifest_name != frontmatter_name
    {
        report.push(SkillIssue::error(
            "SKILL_NAME_MISMATCH",
            "hpa.skill.json.name",
            format!(
                "manifest name `{manifest_name}` does not match SKILL.md name `{frontmatter_name}`"
            ),
        ));
    }

    let mut version = manifest
        .version
        .clone()
        .unwrap_or_else(|| "0.1.0".to_string());
    if manifest.version.is_none() {
        report.push(SkillIssue::warning(
            "SKILL_VERSION_DEFAULT",
            "hpa.skill.json.version",
            "version defaults to 0.1.0; set an explicit semver before publishing",
        ));
    }
    if !valid_semver(&version) {
        report.push(SkillIssue::error(
            "SKILL_VERSION",
            "hpa.skill.json.version",
            format!("invalid semver `{version}`"),
        ));
        version = "0.1.0".to_string();
    }

    let license = manifest
        .license
        .clone()
        .or(frontmatter.license.clone())
        .filter(|value| !value.trim().is_empty());
    match license.as_deref() {
        None => report.push(
            SkillIssue::warning(
                "SKILL_LICENSE_MISSING",
                "hpa.skill.json.license",
                "license is required for publish and export",
            )
            .with_hint("Add an SPDX license such as Apache-2.0 or MIT."),
        ),
        Some(value) if denied_license(value) => report.push(SkillIssue::warning(
            "SKILL_LICENSE_DENIED",
            "hpa.skill.json.license",
            format!("license `{value}` cannot be published or exported"),
        )),
        Some(value) if !allowed_license(value) => report.push(SkillIssue::warning(
            "SKILL_LICENSE_UNKNOWN",
            "hpa.skill.json.license",
            format!("license `{value}` is not in the local allowlist"),
        )),
        _ => {}
    }

    let entry = manifest.entry.kind();
    if !manifest.scripts.is_empty() && !entry.deterministic() {
        report.push(SkillIssue::warning(
            "SKILL_ENTRY_MISMATCH",
            "hpa.skill.json.entry.kind",
            "scripts are declared but the entry kind is `instructions`",
        ));
    }
    if entry.deterministic() && manifest.scripts.is_empty() {
        report.push(SkillIssue::error(
            "SKILL_SCRIPT_MISSING",
            "hpa.skill.json.entry.kind",
            "script entry requires at least one declared script",
        ));
    }

    validate_scripts(&manifest, &files, &mut report);
    validate_permissions(&manifest, &mut report);
    validate_tests(&manifest, &mut report);
    validate_dependencies(&manifest, &mut report);
    validate_schemas(&manifest, &mut report);

    if manifest_present {
        report.sort();
        let snapshot = SkillSnapshot {
            id,
            name,
            version,
            license,
            manifest: manifest_raw,
            manifest_present,
            skill_md,
            content_hash: content_hash(&files),
            files,
        };
        return Ok(SkillInspection {
            root,
            snapshot,
            frontmatter,
            scripts: manifest.scripts,
            tests: manifest.tests,
            permissions: manifest.permissions,
            entry,
            report,
        });
    }

    let synthesized = json!({
        "schemaVersion": SKILL_SCHEMA_VERSION,
        "id": id,
        "name": name,
        "version": version,
        "license": license,
        "entry": {"kind": entry},
        "scripts": [],
        "tests": [],
    });
    report.sort();
    let snapshot = SkillSnapshot {
        id,
        name,
        version,
        license,
        manifest: synthesized,
        manifest_present,
        skill_md,
        content_hash: content_hash(&files),
        files,
    };
    Ok(SkillInspection {
        root,
        snapshot,
        frontmatter,
        scripts: manifest.scripts,
        tests: manifest.tests,
        permissions: manifest.permissions,
        entry,
        report,
    })
}

fn collect_files(
    directory: &Path,
    prefix: &str,
    depth: usize,
    files: &mut Vec<SkillFile>,
    report: &mut SkillReport,
) -> SkillResult<()> {
    if depth > MAX_DEPTH {
        report.push(SkillIssue::error(
            "SKILL_DEPTH",
            prefix,
            format!("directory nesting exceeds {MAX_DEPTH} levels"),
        ));
        return Ok(());
    }
    let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == ".git" || name == ".DS_Store" {
            continue;
        }
        let path = entry.path();
        let relative = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}/{name}")
        };
        if relative.contains('\\') || relative.chars().any(char::is_control) {
            report.push(SkillIssue::error(
                "SKILL_PATH",
                relative,
                "file path contains unsupported characters",
            ));
            continue;
        }
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            report.push(SkillIssue::error(
                "SKILL_PATH_SYMLINK",
                relative,
                "symbolic links are not allowed in skill packages",
            ));
            continue;
        }
        if metadata.is_dir() {
            collect_files(&path, &relative, depth + 1, files, report)?;
            continue;
        }
        if !metadata.is_file() {
            report.push(SkillIssue::error(
                "SKILL_PATH_TYPE",
                relative,
                "only regular files and directories are allowed",
            ));
            continue;
        }
        let size = metadata.len();
        if size > MAX_FILE_BYTES {
            report.push(SkillIssue::error(
                "SKILL_FILE_SIZE",
                relative,
                format!("file is {size} bytes; the limit is {MAX_FILE_BYTES}"),
            ));
            continue;
        }
        let content = fs::read(&path)?;
        let sha256 = hash_bytes(&content);
        files.push(SkillFile {
            path: relative,
            size,
            sha256,
            content,
        });
    }
    Ok(())
}

fn check_limits(files: &[SkillFile], report: &mut SkillReport) {
    if files.len() > MAX_FILES {
        report.push(SkillIssue::error(
            "SKILL_FILE_COUNT",
            ".",
            format!("skill has {} files; the limit is {MAX_FILES}", files.len()),
        ));
    }
    let total: u64 = files.iter().map(|file| file.size).sum();
    if total > MAX_TOTAL_BYTES {
        report.push(SkillIssue::error(
            "SKILL_TOTAL_SIZE",
            ".",
            format!("skill is {total} bytes; the limit is {MAX_TOTAL_BYTES}"),
        ));
    }
}

fn parse_frontmatter(text: &str, report: &mut SkillReport) -> (SkillFrontmatter, String) {
    let Some((yaml, body)) = split_frontmatter(text) else {
        report.push(
            SkillIssue::error(
                "SKILL_FRONTMATTER",
                "SKILL.md",
                "SKILL.md must start with a `---` YAML frontmatter block",
            )
            .with_hint("Begin SKILL.md with `---`, the frontmatter fields, then another `---`."),
        );
        return (SkillFrontmatter::default(), text.to_string());
    };
    let mut value = match hpa_core::yaml::yaml_to_json(yaml) {
        Ok(value) => value,
        Err(error) => {
            report.push(SkillIssue::error(
                "SKILL_FRONTMATTER",
                "SKILL.md",
                format!("cannot parse frontmatter: {error}"),
            ));
            return (SkillFrontmatter::default(), body.to_string());
        }
    };
    if let Value::Object(object) = &mut value {
        if let Some(Value::String(tools)) = object.get("allowed-tools").cloned() {
            let tools: Vec<String> = tools
                .split([' ', ','])
                .map(str::trim)
                .filter(|tool| !tool.is_empty())
                .map(str::to_string)
                .collect();
            object.insert("allowed-tools".to_string(), json!(tools));
        }
    } else {
        report.push(SkillIssue::error(
            "SKILL_FRONTMATTER",
            "SKILL.md",
            "frontmatter must be a mapping",
        ));
        return (SkillFrontmatter::default(), body.to_string());
    }
    match serde_json::from_value::<SkillFrontmatter>(value) {
        Ok(frontmatter) => (frontmatter, body.to_string()),
        Err(error) => {
            report.push(SkillIssue::error(
                "SKILL_FRONTMATTER",
                "SKILL.md",
                format!("frontmatter shape is invalid: {error}"),
            ));
            (SkillFrontmatter::default(), body.to_string())
        }
    }
}

fn split_frontmatter(text: &str) -> Option<(&str, &str)> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let first_end = text.find('\n')?;
    if text[..first_end].trim() != "---" {
        return None;
    }
    let mut position = first_end + 1;
    for line in text[position..].split_inclusive('\n') {
        let content = line.trim_end_matches('\n').trim_end_matches('\r');
        if content.trim() == "---" {
            let body_start = position + line.len();
            return Some((&text[first_end + 1..position], &text[body_start..]));
        }
        position += line.len();
    }
    None
}

fn validate_frontmatter(
    frontmatter: &SkillFrontmatter,
    instructions: &str,
    report: &mut SkillReport,
) {
    match frontmatter.name.as_deref() {
        Some(name) if valid_skill_name(name) => {}
        Some(name) => report.push(SkillIssue::error(
            "SKILL_NAME",
            "SKILL.md.name",
            format!(
                "`{name}` must use lowercase letters, digits and hyphens, at most 64 characters"
            ),
        )),
        None => report.push(SkillIssue::error(
            "SKILL_NAME",
            "SKILL.md.name",
            "frontmatter name is required",
        )),
    }
    match frontmatter.description.as_deref() {
        Some(description)
            if !description.trim().is_empty() && description.chars().count() <= 1024 => {}
        Some(_) => report.push(SkillIssue::error(
            "SKILL_DESCRIPTION",
            "SKILL.md.description",
            "description must be 1..=1024 characters",
        )),
        None => report.push(SkillIssue::error(
            "SKILL_DESCRIPTION",
            "SKILL.md.description",
            "frontmatter description is required",
        )),
    }
    if frontmatter
        .compatibility
        .as_deref()
        .is_some_and(|value| value.chars().count() > 500)
    {
        report.push(SkillIssue::error(
            "SKILL_COMPATIBILITY",
            "SKILL.md.compatibility",
            "compatibility must be at most 500 characters",
        ));
    }
    if let Some(tools) = &frontmatter.allowed_tools
        && tools.iter().any(|tool| tool.trim().is_empty())
    {
        report.push(SkillIssue::error(
            "SKILL_ALLOWED_TOOLS",
            "SKILL.md.allowed-tools",
            "allowed-tools cannot contain empty entries",
        ));
    }
    if instructions.trim().is_empty() {
        report.push(SkillIssue::warning(
            "SKILL_INSTRUCTIONS_EMPTY",
            "SKILL.md",
            "instructions body is empty",
        ));
    }
    if instructions.lines().count() > 500 {
        report.push(SkillIssue::warning(
            "SKILL_INSTRUCTIONS_LONG",
            "SKILL.md",
            "instructions exceed the recommended 500 lines; move detail to references/",
        ));
    }
}

fn validate_scripts(manifest: &SkillManifest, files: &[SkillFile], report: &mut SkillReport) {
    let mut seen = BTreeSet::new();
    for (index, script) in manifest.scripts.iter().enumerate() {
        let path = format!("hpa.skill.json.scripts[{index}]");
        if script.id.trim().is_empty() {
            report.push(SkillIssue::error(
                "SKILL_SCRIPT_ID",
                format!("{path}.id"),
                "script id is required",
            ));
        } else if !seen.insert(script.id.clone()) {
            report.push(SkillIssue::error(
                "SKILL_SCRIPT_DUPLICATE",
                format!("{path}.id"),
                format!("duplicate script id `{}`", script.id),
            ));
        }
        if !safe_relative_path(&script.path) || !script.path.starts_with("scripts/") {
            report.push(SkillIssue::error(
                "SKILL_SCRIPT_PATH",
                format!("{path}.path"),
                "script path must be a relative path under scripts/",
            ));
        } else if !files.iter().any(|file| file.path == script.path) {
            report.push(SkillIssue::error(
                "SKILL_SCRIPT_MISSING",
                format!("{path}.path"),
                format!("script `{}` does not exist in the package", script.path),
            ));
        }
        if !RUNTIMES.contains(&script.runtime()) {
            report.push(SkillIssue::error(
                "SKILL_RUNTIME",
                format!("{path}.runtime"),
                format!(
                    "runtime `{}` is not supported; use python3, node, bash or sh",
                    script.runtime()
                ),
            ));
        }
        if script.args.len() > MAX_SCRIPT_ARGS
            || script
                .args
                .iter()
                .any(|arg| arg.len() > MAX_ARG_BYTES || arg.contains('\0'))
        {
            report.push(SkillIssue::error(
                "SKILL_SCRIPT_ARGS",
                format!("{path}.args"),
                "args exceed the supported count or length",
            ));
        }
        if script
            .timeout_ms
            .is_some_and(|timeout| timeout == 0 || timeout > crate::manifest::MAX_TIMEOUT_MS)
        {
            report.push(SkillIssue::error(
                "SKILL_SCRIPT_TIMEOUT",
                format!("{path}.timeoutMs"),
                "timeoutMs must be between 1 and 600000",
            ));
        }
        if script.network() != "none" {
            report.push(
                SkillIssue::error(
                    "SKILL_PERMISSION",
                    format!("{path}.network"),
                    "local script execution is offline; network access is not available",
                )
                .with_hint("Use network: none and fetch resources through the host runtime."),
            );
        }
        for (key, value) in &script.env {
            if !SAFE_ENV.contains(&key.as_str()) {
                report.push(SkillIssue::error(
                    "SKILL_ENV",
                    format!("{path}.env.{key}"),
                    format!("environment variable `{key}` is not in the local allowlist"),
                ));
            }
            if value.len() > 1024 || value.contains('\0') || value.contains('\n') {
                report.push(SkillIssue::error(
                    "SKILL_ENV",
                    format!("{path}.env.{key}"),
                    "environment value is invalid",
                ));
            }
        }
    }
    if !manifest.scripts.is_empty() && manifest.tests.is_empty() {
        report.push(SkillIssue::warning(
            "SKILL_TEST_MISSING",
            "hpa.skill.json.tests",
            "declare at least one smoke test before publishing",
        ));
    }
}

fn validate_permissions(manifest: &SkillManifest, report: &mut SkillReport) {
    let permissions = &manifest.permissions;
    if !permissions.network_rules().is_empty() {
        report.push(SkillIssue::error(
            "SKILL_PERMISSION",
            "hpa.skill.json.permissions.network",
            "network permissions are not supported by the local sandbox",
        ));
    }
    for command in &permissions.commands {
        if !RUNTIMES.contains(&command.as_str()) && command != "python" {
            report.push(SkillIssue::warning(
                "SKILL_COMMAND_UNSUPPORTED",
                "hpa.skill.json.permissions.commands",
                format!("command `{command}` is not used by the local runtime"),
            ));
        }
    }
    for name in &permissions.env {
        if !SAFE_ENV.contains(&name.as_str()) {
            report.push(SkillIssue::warning(
                "SKILL_ENV_UNSUPPORTED",
                "hpa.skill.json.permissions.env",
                format!("environment variable `{name}` is not in the local allowlist"),
            ));
        }
    }
    if let Some(filesystem) = &permissions.filesystem
        && !filesystem.is_object()
        && !filesystem.is_array()
    {
        report.push(SkillIssue::error(
            "SKILL_PERMISSION",
            "hpa.skill.json.permissions.filesystem",
            "filesystem permissions must be an object or array",
        ));
    }
    if let Some(resources) = &permissions.resources {
        if resources
            .memory_mb
            .is_some_and(|value| value == 0 || value > 4096)
        {
            report.push(SkillIssue::error(
                "SKILL_RESOURCE",
                "hpa.skill.json.permissions.resources.memoryMb",
                "memoryMb must be between 1 and 4096",
            ));
        }
        if resources
            .cpu
            .is_some_and(|value| value == 0 || value > 3600)
        {
            report.push(SkillIssue::error(
                "SKILL_RESOURCE",
                "hpa.skill.json.permissions.resources.cpu",
                "cpu seconds must be between 1 and 3600",
            ));
        }
        if resources
            .max_output_bytes
            .is_some_and(|value| value == 0 || value > crate::manifest::MAX_OUTPUT_BYTES)
        {
            report.push(SkillIssue::error(
                "SKILL_RESOURCE",
                "hpa.skill.json.permissions.resources.maxOutputBytes",
                "maxOutputBytes is outside the supported range",
            ));
        }
    }
}

fn validate_tests(manifest: &SkillManifest, report: &mut SkillReport) {
    let scripts: BTreeSet<&str> = manifest
        .scripts
        .iter()
        .map(|script| script.id.as_str())
        .collect();
    let mut seen = BTreeSet::new();
    for (index, test) in manifest.tests.iter().enumerate() {
        let path = format!("hpa.skill.json.tests[{index}]");
        if test.name.trim().is_empty() {
            report.push(SkillIssue::error(
                "SKILL_TEST_NAME",
                format!("{path}.name"),
                "test name is required",
            ));
        } else if !seen.insert(test.name.clone()) {
            report.push(SkillIssue::error(
                "SKILL_TEST_DUPLICATE",
                format!("{path}.name"),
                format!("duplicate test name `{}`", test.name),
            ));
        }
        let script = test.script.as_deref().unwrap_or_default();
        if script.is_empty() {
            report.push(SkillIssue::error(
                "SKILL_TEST_SCRIPT",
                format!("{path}.script"),
                "test must reference a script id",
            ));
        } else if !scripts.contains(script) {
            report.push(SkillIssue::error(
                "SKILL_TEST_SCRIPT",
                format!("{path}.script"),
                format!("test references unknown script `{script}`"),
            ));
        }
        if test
            .expect
            .exit_code
            .is_some_and(|code| !(0..=255).contains(&code))
        {
            report.push(SkillIssue::error(
                "SKILL_TEST_EXPECT",
                format!("{path}.expect.exitCode"),
                "exitCode must be between 0 and 255",
            ));
        }
        if let Some(schema) = &test.expect.output_schema
            && jsonschema::validator_for(schema).is_err()
        {
            report.push(SkillIssue::error(
                "SKILL_TEST_SCHEMA",
                format!("{path}.expect.outputSchema"),
                "outputSchema is not a valid JSON Schema",
            ));
        }
        if test
            .expect
            .artifacts
            .iter()
            .any(|artifact| !safe_relative_path(artifact))
        {
            report.push(SkillIssue::error(
                "SKILL_TEST_ARTIFACT",
                format!("{path}.expect.artifacts"),
                "artifact expectations must be safe relative paths",
            ));
        }
    }
}

fn validate_dependencies(manifest: &SkillManifest, report: &mut SkillReport) {
    for (index, dependency) in manifest.dependencies.iter().enumerate() {
        let path = format!("hpa.skill.json.dependencies[{index}]");
        let Some(object) = dependency.as_object() else {
            report.push(SkillIssue::error(
                "SKILL_DEPENDENCY",
                path,
                "dependency must be an object",
            ));
            continue;
        };
        let Some(kind) = object.get("type").and_then(Value::as_str) else {
            report.push(SkillIssue::error(
                "SKILL_DEPENDENCY",
                format!("{path}.type"),
                "dependency type is required",
            ));
            continue;
        };
        if !matches!(kind, "skill" | "python" | "node") {
            report.push(SkillIssue::warning(
                "SKILL_DEPENDENCY_UNSUPPORTED",
                format!("{path}.type"),
                format!("dependency type `{kind}` is preserved but not resolved locally"),
            ));
        }
        let name = object
            .get("id")
            .or_else(|| object.get("name"))
            .and_then(Value::as_str);
        if name.is_none_or(|name| name.trim().is_empty()) {
            report.push(SkillIssue::error(
                "SKILL_DEPENDENCY",
                path,
                "dependency id/name is required",
            ));
        }
    }
}

fn validate_schemas(manifest: &SkillManifest, report: &mut SkillReport) {
    for (index, script) in manifest.scripts.iter().enumerate() {
        for (field, schema) in [
            ("inputSchema", &script.input_schema),
            ("outputSchema", &script.output_schema),
        ] {
            if let Some(schema) = schema
                && jsonschema::validator_for(schema).is_err()
            {
                report.push(SkillIssue::error(
                    "SKILL_SCHEMA",
                    format!("hpa.skill.json.scripts[{index}].{field}"),
                    "schema is not a valid JSON Schema",
                ));
            }
        }
    }
}

pub fn safe_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.starts_with('\\')
        && !path.contains('\\')
        && !path.contains('\0')
        && path
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
}

pub fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn content_hash(files: &[SkillFile]) -> String {
    let mut ordered: Vec<&SkillFile> = files.iter().collect();
    ordered.sort_by(|left, right| left.path.cmp(&right.path));
    let mut hasher = Sha256::new();
    for file in ordered {
        hasher.update(file.path.as_bytes());
        hasher.update([0]);
        hasher.update(file.sha256.as_bytes());
        hasher.update(b"\n");
    }
    format!("{:x}", hasher.finalize())
}

fn root_name(root: &Path) -> String {
    root.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| "skill".to_string())
}
