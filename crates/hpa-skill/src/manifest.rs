use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{SkillIssue, SkillReport};

pub const SKILL_SCHEMA_VERSION: &str = "hpa.skill/v1";
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;
pub const MAX_TIMEOUT_MS: u64 = 600_000;
pub const DEFAULT_OUTPUT_BYTES: u64 = 1024 * 1024;
pub const MAX_OUTPUT_BYTES: u64 = 16 * 1024 * 1024;
pub const DEFAULT_MEMORY_MB: u64 = 512;
pub const MAX_MEMORY_MB: u64 = 4096;
pub const DEFAULT_CPU_SECONDS: u64 = 30;
pub const MAX_ARTIFACT_BYTES: u64 = 50 * 1024 * 1024;
pub const MAX_ARTIFACT_FILES: usize = 100;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SkillManifest {
    #[serde(default)]
    pub schema_version: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub entry: EntrySpec,
    #[serde(default)]
    pub scripts: Vec<SkillScript>,
    #[serde(default)]
    pub permissions: SkillPermissions,
    #[serde(default)]
    pub dependencies: Vec<Value>,
    #[serde(default)]
    pub tests: Vec<SkillTest>,
    #[serde(default)]
    pub trust: Option<Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EntrySpec {
    #[serde(default)]
    pub kind: Option<EntryKind>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl EntrySpec {
    pub fn kind(&self) -> EntryKind {
        self.kind.unwrap_or_default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    #[default]
    Instructions,
    Script,
    Both,
}

impl EntryKind {
    pub fn deterministic(&self) -> bool {
        matches!(self, Self::Script | Self::Both)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SkillScript {
    pub id: String,
    pub path: String,
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub network: Option<String>,
    #[serde(default)]
    pub input_schema: Option<Value>,
    #[serde(default)]
    pub output_schema: Option<Value>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl SkillScript {
    pub fn timeout_ms(&self) -> u64 {
        self.timeout_ms
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .clamp(1, MAX_TIMEOUT_MS)
    }

    pub fn runtime(&self) -> &str {
        self.runtime.as_deref().unwrap_or("python3")
    }

    pub fn network(&self) -> &str {
        self.network.as_deref().unwrap_or("none")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SkillPermissions {
    #[serde(default)]
    pub filesystem: Option<Value>,
    #[serde(default)]
    pub network: Option<Value>,
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub resources: Option<ResourceLimits>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl SkillPermissions {
    pub fn resources(&self) -> ResourceLimits {
        self.resources.clone().unwrap_or_default()
    }

    pub fn network_rules(&self) -> Vec<String> {
        match &self.network {
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
            Some(Value::String(text)) if text != "none" && !text.is_empty() => vec![text.clone()],
            _ => Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ResourceLimits {
    #[serde(default)]
    pub cpu: Option<u64>,
    #[serde(default)]
    pub memory_mb: Option<u64>,
    #[serde(default)]
    pub max_output_bytes: Option<u64>,
    #[serde(default)]
    pub max_artifact_bytes: Option<u64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl ResourceLimits {
    pub fn cpu_seconds(&self) -> u64 {
        self.cpu.unwrap_or(DEFAULT_CPU_SECONDS).clamp(1, 3600)
    }

    pub fn memory_mb(&self) -> u64 {
        self.memory_mb
            .unwrap_or(DEFAULT_MEMORY_MB)
            .clamp(16, MAX_MEMORY_MB)
    }

    pub fn output_bytes(&self) -> u64 {
        self.max_output_bytes
            .unwrap_or(DEFAULT_OUTPUT_BYTES)
            .clamp(1024, MAX_OUTPUT_BYTES)
    }

    pub fn artifact_bytes(&self) -> u64 {
        self.max_artifact_bytes
            .unwrap_or(MAX_ARTIFACT_BYTES)
            .clamp(1024, MAX_ARTIFACT_BYTES)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SkillTest {
    pub name: String,
    #[serde(default)]
    pub script: Option<String>,
    #[serde(default)]
    pub input: Value,
    #[serde(default)]
    pub expect: SkillExpectation,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SkillExpectation {
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub output_schema: Option<Value>,
    #[serde(default)]
    pub artifacts: Vec<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SkillFrontmatter {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub compatibility: Option<String>,
    #[serde(default)]
    pub metadata: Option<Value>,
    #[serde(rename = "allowed-tools", default)]
    pub allowed_tools: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone)]
pub struct SkillFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
    pub content: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct SkillSnapshot {
    pub id: String,
    pub name: String,
    pub version: String,
    pub license: Option<String>,
    pub manifest: Value,
    pub manifest_present: bool,
    pub skill_md: String,
    pub files: Vec<SkillFile>,
    pub content_hash: String,
}

impl SkillSnapshot {
    pub fn manifest(&self) -> SkillManifest {
        serde_json::from_value(self.manifest.clone()).unwrap_or_default()
    }
}

#[derive(Debug, Clone)]
pub struct SkillInspection {
    pub root: std::path::PathBuf,
    pub snapshot: SkillSnapshot,
    pub frontmatter: SkillFrontmatter,
    pub scripts: Vec<SkillScript>,
    pub tests: Vec<SkillTest>,
    pub permissions: SkillPermissions,
    pub entry: EntryKind,
    pub report: SkillReport,
}

impl SkillInspection {
    pub fn manifest(&self) -> SkillManifest {
        serde_json::from_value(self.snapshot.manifest.clone()).unwrap_or_default()
    }

    pub fn has_errors(&self) -> bool {
        !self.report.is_valid()
    }

    pub fn error_issues(&self) -> Vec<SkillIssue> {
        self.report
            .issues
            .iter()
            .filter(|issue| issue.severity == crate::error::Severity::Error)
            .cloned()
            .collect()
    }
}

pub fn derive_skill_id(name: &str) -> String {
    let mut slug = String::new();
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.ends_with('_') {
            slug.push('_');
        }
    }
    let slug = slug.trim_matches('_');
    if slug.is_empty() {
        "skill_unnamed".to_string()
    } else {
        format!("skill_{slug}")
    }
}

pub fn valid_skill_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 96
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

pub fn valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-')
}

pub fn valid_semver(version: &str) -> bool {
    let core = version
        .split_once(['-', '+'])
        .map_or(version, |(core, _)| core);
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() != 3 {
        return false;
    }
    parts.iter().all(|part| {
        !part.is_empty()
            && part.bytes().all(|byte| byte.is_ascii_digit())
            && (part == &"0" || !part.starts_with('0'))
    })
}

fn numeric_version(version: &str) -> Option<(u64, u64, u64)> {
    let core = version
        .split_once(['-', '+'])
        .map_or(version, |(core, _)| core);
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

pub fn version_satisfies(version: &str, requirement: &str) -> bool {
    let Some(candidate) = numeric_version(version) else {
        return false;
    };
    let requirement = requirement.trim();
    if requirement.is_empty() || requirement == "*" {
        return true;
    }
    requirement
        .split([',', ' '])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .all(|part| {
            if part == "*" {
                return true;
            }
            let (operator, raw) = if let Some(rest) = part.strip_prefix(">=") {
                (">=", rest)
            } else if let Some(rest) = part.strip_prefix("<=") {
                ("<=", rest)
            } else if let Some(rest) = part.strip_prefix('>') {
                (">", rest)
            } else if let Some(rest) = part.strip_prefix('<') {
                ("<", rest)
            } else if let Some(rest) = part.strip_prefix('^') {
                ("^", rest)
            } else if let Some(rest) = part.strip_prefix('~') {
                ("~", rest)
            } else if let Some(rest) = part.strip_prefix('=') {
                ("=", rest)
            } else {
                ("=", part)
            };
            let Some(target) = numeric_version(raw.trim()) else {
                return false;
            };
            match operator {
                ">=" => candidate >= target,
                "<=" => candidate <= target,
                ">" => candidate > target,
                "<" => candidate < target,
                "=" => candidate == target,
                "^" => {
                    if target.0 > 0 {
                        candidate.0 == target.0 && candidate >= target
                    } else if target.1 > 0 {
                        candidate.0 == 0 && candidate.1 == target.1 && candidate >= target
                    } else {
                        candidate == target
                    }
                }
                "~" => candidate.0 == target.0 && candidate.1 == target.1 && candidate >= target,
                _ => false,
            }
        })
}

pub fn allowed_license(license: &str) -> bool {
    matches!(
        license.trim().to_ascii_lowercase().as_str(),
        "apache-2.0"
            | "mit"
            | "mit-0"
            | "bsd-2-clause"
            | "bsd-3-clause"
            | "isc"
            | "mpl-2.0"
            | "cc0-1.0"
            | "unlicense"
            | "0bsd"
            | "gpl-2.0-only"
            | "gpl-2.0-or-later"
            | "gpl-3.0-only"
            | "gpl-3.0-or-later"
            | "lgpl-2.1-only"
            | "lgpl-2.1-or-later"
            | "lgpl-3.0-only"
            | "lgpl-3.0-or-later"
            | "agpl-3.0-only"
            | "agpl-3.0-or-later"
    )
}

pub fn denied_license(license: &str) -> bool {
    matches!(
        license.trim().to_ascii_lowercase().as_str(),
        "" | "unlicensed" | "proprietary" | "none" | "commercial" | "all-rights-reserved"
    )
}

pub fn empty_input() -> Value {
    Value::Object(Map::new())
}
