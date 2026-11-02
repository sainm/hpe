use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use hpa_skill::{
    EntryKind, RunOutcome, RunnerConfig, SandboxKind, SkillError, SkillManifest, SkillReport,
    SkillRunRequest, SkillSnapshot, SkillTest, publish_snapshot_issues, run_script,
    version_satisfies,
};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::db::{Db, DbError, SkillRunRow, SkillSummaryRow, SkillVersionRow, build_skill_snapshot};

#[derive(Debug, Error)]
pub enum SkillServiceError {
    #[error("{0}")]
    Skill(#[from] SkillError),
    #[error("{0}")]
    Db(#[from] DbError),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    NotFound(String),
    #[error("{message}")]
    Invalid {
        code: String,
        message: String,
        details: Option<Value>,
    },
}

impl SkillServiceError {
    fn invalid(
        code: impl Into<String>,
        message: impl Into<String>,
        details: Option<Value>,
    ) -> Self {
        Self::Invalid {
            code: code.into(),
            message: message.into(),
            details,
        }
    }
}

#[derive(Clone)]
pub struct SkillService {
    pub db: Db,
    pub data_dir: PathBuf,
}

pub struct LoadedSkill {
    pub version: SkillVersionRow,
    pub snapshot: SkillSnapshot,
    pub manifest: SkillManifest,
    pub report: SkillReport,
    pub signature: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct ImportOutcome {
    pub inserted: bool,
    pub summary: SkillSummaryRow,
    pub version: SkillVersionRow,
    pub report: SkillReport,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    pub name: String,
    pub passed: bool,
    pub failures: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<RunOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSource {
    pub version: String,
    pub files: BTreeMap<String, String>,
    pub binary_files: Vec<String>,
}

impl SkillService {
    pub fn new(db: Db, data_dir: PathBuf) -> Self {
        Self { db, data_dir }
    }

    pub fn runner_config(&self) -> RunnerConfig {
        RunnerConfig {
            work_root: self.data_dir.join("tmp"),
            artifact_store: self.data_dir.join("artifacts"),
            audit_log: Some(self.data_dir.join("logs/audit.jsonl")),
        }
    }

    pub async fn import(&self, path: &Path) -> Result<ImportOutcome, SkillServiceError> {
        let inspection = hpa_skill::inspect(path)?;
        if !inspection.report.is_valid() {
            return Err(SkillServiceError::invalid(
                "SKILL_INVALID",
                "skill package validation failed",
                Some(serde_json::to_value(&inspection.report).unwrap_or(Value::Null)),
            ));
        }
        self.resolve_dependencies(&inspection.manifest()).await?;
        let imported_at = now_rfc3339();
        let source = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let inserted = self
            .db
            .import_skill(
                &inspection.snapshot,
                &inspection.report,
                Some(&source.display().to_string()),
                &imported_at,
            )
            .await?;
        let summary = self
            .db
            .get_skill(&inspection.snapshot.id)
            .await?
            .ok_or_else(|| SkillServiceError::NotFound(inspection.snapshot.id.clone()))?;
        let version = self
            .db
            .get_skill_version(&inspection.snapshot.id, &inspection.snapshot.version)
            .await?
            .ok_or_else(|| SkillServiceError::NotFound(inspection.snapshot.id.clone()))?;
        Ok(ImportOutcome {
            inserted,
            summary,
            version,
            report: inspection.report,
        })
    }

    pub async fn load(
        &self,
        skill_id: &str,
        version: Option<&str>,
    ) -> Result<LoadedSkill, SkillServiceError> {
        let summary = self
            .db
            .get_skill(skill_id)
            .await?
            .ok_or_else(|| SkillServiceError::NotFound(skill_id.to_string()))?;
        let version = version.unwrap_or(&summary.latest_version);
        let row = self
            .db
            .get_skill_version(skill_id, version)
            .await?
            .ok_or_else(|| {
                SkillServiceError::NotFound(format!("{skill_id}@{version} is not imported"))
            })?;
        let files = self.db.get_skill_files(skill_id, version).await?;
        let snapshot = build_skill_snapshot(&row, &files)?;
        let manifest = snapshot.manifest();
        let report: SkillReport = serde_json::from_str(&row.report_json)?;
        let signature = self.signature_info(skill_id, version).await?;
        Ok(LoadedSkill {
            version: row,
            snapshot,
            manifest,
            report,
            signature,
        })
    }

    pub async fn publish(
        &self,
        skill_id: &str,
        version: Option<&str>,
    ) -> Result<bool, SkillServiceError> {
        let loaded = self.load(skill_id, version).await?;
        if !loaded.report.is_valid() {
            return Err(SkillServiceError::invalid(
                "SKILL_INVALID",
                "stored skill report contains errors",
                Some(serde_json::to_value(&loaded.report).unwrap_or(Value::Null)),
            ));
        }
        let issues = publish_snapshot_issues(&loaded.snapshot, &loaded.manifest);
        let has_errors = issues
            .iter()
            .any(|issue| issue.severity == hpa_skill::Severity::Error);
        if has_errors {
            return Err(SkillServiceError::invalid(
                "SKILL_NOT_PUBLISHABLE",
                "skill cannot be published",
                Some(serde_json::to_value(&issues).unwrap_or(Value::Null)),
            ));
        }
        if let Some(signature) = &loaded.signature
            && signature.get("verified").and_then(Value::as_bool) == Some(false)
        {
            return Err(SkillServiceError::invalid(
                "SKILL_SIGNATURE_INVALID",
                "skill signature does not match the stored snapshot",
                Some(signature.clone()),
            ));
        }
        if loaded.signature.is_none()
            && std::env::var("HPA_REQUIRE_SKILL_SIGNATURE")
                .map(|value| matches!(value.as_str(), "1" | "true" | "required"))
                .unwrap_or(false)
        {
            return Err(SkillServiceError::invalid(
                "SKILL_SIGNATURE_REQUIRED",
                "skill publishing requires a local signature",
                None,
            ));
        }
        Ok(self
            .db
            .publish_skill_version(skill_id, &loaded.snapshot.version, &now_rfc3339())
            .await?)
    }

    pub async fn export(
        &self,
        skill_id: &str,
        version: Option<&str>,
        target: &Path,
    ) -> Result<(), SkillServiceError> {
        let loaded = self.load(skill_id, version).await?;
        hpa_skill::export_snapshot(&loaded.snapshot, target)?;
        Ok(())
    }

    pub async fn source(
        &self,
        skill_id: &str,
        version: Option<&str>,
    ) -> Result<SkillSource, SkillServiceError> {
        let loaded = self.load(skill_id, version).await?;
        let mut files = BTreeMap::new();
        let mut binary_files = Vec::new();
        for file in loaded.snapshot.files {
            match String::from_utf8(file.content) {
                Ok(text) => {
                    files.insert(file.path, text);
                }
                Err(_) => binary_files.push(file.path),
            }
        }
        Ok(SkillSource {
            version: loaded.snapshot.version,
            files,
            binary_files,
        })
    }

    pub async fn sign(
        &self,
        skill_id: &str,
        version: Option<&str>,
    ) -> Result<Value, SkillServiceError> {
        let loaded = self.load(skill_id, version).await?;
        let key = self.local_signing_key().await?;
        let payload = skill_signature_payload(
            &loaded.snapshot.id,
            &loaded.snapshot.version,
            &loaded.snapshot.content_hash,
        );
        let signature = key.signing_key.sign(payload.as_bytes());
        let record = serde_json::json!({
            "algorithm": "ed25519",
            "keyId": key.key_id,
            "publicKey": key.public_key,
            "value": BASE64.encode(signature.to_bytes()),
            "contentHash": loaded.snapshot.content_hash,
            "signedAt": now_rfc3339(),
            "trust": "local",
        });
        let path = self.signature_path(&loaded.snapshot.id, &loaded.snapshot.version);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, serde_json::to_vec_pretty(&record)?)?;
        let mut value = record;
        value["verified"] = Value::Bool(true);
        Ok(value)
    }

    async fn signature_info(
        &self,
        skill_id: &str,
        version: &str,
    ) -> Result<Option<Value>, SkillServiceError> {
        let path = self.signature_path(skill_id, version);
        let Ok(bytes) = std::fs::read(path) else {
            return Ok(None);
        };
        let mut record: Value = match serde_json::from_slice(&bytes) {
            Ok(record) => record,
            Err(error) => {
                return Ok(Some(serde_json::json!({
                    "verified": false,
                    "error": format!("invalid signature record: {error}"),
                })));
            }
        };
        let (_, content_hash) = self.signature_target(skill_id, version).await?;
        let verified = verify_skill_signature(&record, skill_id, version, &content_hash);
        record["verified"] = Value::Bool(verified);
        Ok(Some(record))
    }

    async fn signature_target(
        &self,
        skill_id: &str,
        version: &str,
    ) -> Result<(String, String), SkillServiceError> {
        let row = self
            .db
            .get_skill_version(skill_id, version)
            .await?
            .ok_or_else(|| {
                SkillServiceError::NotFound(format!("{skill_id}@{version} is not imported"))
            })?;
        Ok((row.skill_id, row.content_hash))
    }

    fn signature_path(&self, skill_id: &str, version: &str) -> PathBuf {
        self.data_dir
            .join("skills")
            .join("signatures")
            .join(skill_id)
            .join(format!("{version}.json"))
    }

    async fn local_signing_key(&self) -> Result<LocalSigningKey, SkillServiceError> {
        let keys_dir = self.data_dir.join("keys");
        std::fs::create_dir_all(&keys_dir)?;
        let secret_path = keys_dir.join("plugin-ed25519.key");
        let signing_key = if secret_path.exists() {
            let encoded = std::fs::read_to_string(&secret_path)?;
            let bytes = BASE64.decode(encoded.trim()).map_err(|error| {
                SkillServiceError::invalid(
                    "SKILL_SIGNING_KEY",
                    format!("invalid local signing key: {error}"),
                    None,
                )
            })?;
            let bytes: [u8; 32] = bytes.try_into().map_err(|_| {
                SkillServiceError::invalid(
                    "SKILL_SIGNING_KEY",
                    "local signing key must be 32 bytes",
                    None,
                )
            })?;
            SigningKey::from_bytes(&bytes)
        } else {
            let mut bytes = [0u8; 32];
            getrandom::getrandom(&mut bytes).map_err(|error| {
                SkillServiceError::invalid(
                    "SKILL_SIGNING_KEY",
                    format!("cannot generate local signing key: {error}"),
                    None,
                )
            })?;
            std::fs::write(&secret_path, BASE64.encode(bytes))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ =
                    std::fs::set_permissions(&secret_path, std::fs::Permissions::from_mode(0o600));
            }
            SigningKey::from_bytes(&bytes)
        };
        let public_bytes = signing_key.verifying_key().to_bytes();
        let mut hasher = Sha256::new();
        hasher.update(public_bytes);
        let key_id = format!("ed25519-{}", &format!("{:x}", hasher.finalize())[..16]);
        Ok(LocalSigningKey {
            key_id,
            public_key: BASE64.encode(public_bytes),
            signing_key,
        })
    }

    pub async fn save_draft(
        &self,
        skill_id: &str,
        base_version: &str,
        new_version: &str,
        edits: BTreeMap<String, String>,
    ) -> Result<ImportOutcome, SkillServiceError> {
        let loaded = self.load(skill_id, Some(base_version)).await?;
        if !hpa_skill::manifest::valid_semver(new_version) {
            return Err(SkillServiceError::invalid(
                "SKILL_VERSION",
                format!("new version `{new_version}` is not valid semver"),
                None,
            ));
        }
        if new_version == base_version {
            return Err(SkillServiceError::invalid(
                "SKILL_VERSION_FROZEN",
                "edit drafts must use a new version; stored versions are immutable",
                None,
            ));
        }
        if self
            .db
            .get_skill_version(skill_id, new_version)
            .await?
            .is_some()
        {
            return Err(SkillServiceError::invalid(
                "SKILL_VERSION_EXISTS",
                format!("skill version `{skill_id}@{new_version}` already exists"),
                None,
            ));
        }
        let draft_root = self
            .data_dir
            .join("skills")
            .join("drafts")
            .join(skill_id)
            .join(format!("{new_version}-{}", uuid::Uuid::now_v7()));
        if draft_root.exists() {
            return Err(SkillServiceError::invalid(
                "SKILL_DRAFT_EXISTS",
                format!("draft directory `{}` already exists", draft_root.display()),
                None,
            ));
        }
        std::fs::create_dir_all(&draft_root)?;
        let mut files = loaded
            .snapshot
            .files
            .into_iter()
            .map(|file| (file.path, file.content))
            .collect::<BTreeMap<_, _>>();
        let mut total_bytes = files.values().map(Vec::len).sum::<usize>();
        for (path, content) in edits {
            validate_draft_path(&path)?;
            total_bytes = total_bytes
                .saturating_sub(files.get(&path).map(Vec::len).unwrap_or(0))
                .saturating_add(content.len());
            if content.len() > hpa_skill::loader::MAX_FILE_BYTES as usize {
                return Err(SkillServiceError::invalid(
                    "SKILL_FILE_TOO_LARGE",
                    format!("draft file `{path}` exceeds the per-file limit"),
                    None,
                ));
            }
            files.insert(path, content.into_bytes());
        }
        if total_bytes > hpa_skill::loader::MAX_TOTAL_BYTES as usize {
            return Err(SkillServiceError::invalid(
                "SKILL_TOO_LARGE",
                "draft exceeds the Skill package size limit",
                None,
            ));
        }
        let mut manifest = loaded.snapshot.manifest;
        if let Some(edited) = files
            .get("hpa.skill.json")
            .and_then(|content| serde_json::from_slice::<Value>(content).ok())
        {
            manifest = edited;
        }
        let Some(manifest_object) = manifest.as_object_mut() else {
            return Err(SkillServiceError::invalid(
                "SKILL_MANIFEST",
                "skill manifest must be a JSON object",
                None,
            ));
        };
        manifest_object.insert(
            "version".to_string(),
            Value::String(new_version.to_string()),
        );
        manifest_object
            .entry("schemaVersion".to_string())
            .or_insert_with(|| Value::String("hpa.skill/v1".to_string()));
        files.insert(
            "hpa.skill.json".to_string(),
            serde_json::to_vec_pretty(&manifest)?,
        );
        for (relative, content) in files {
            validate_draft_path(&relative)?;
            let target = draft_root.join(&relative);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(target, content)?;
        }
        self.import(&draft_root).await
    }

    pub async fn run(
        &self,
        skill_id: &str,
        version: Option<&str>,
        script_id: Option<&str>,
        input: Value,
    ) -> Result<RunOutcome, SkillServiceError> {
        let loaded = self.load(skill_id, version).await?;
        let script = script_id
            .map(str::to_string)
            .or_else(|| default_script(&loaded.snapshot, &loaded.manifest))
            .ok_or_else(|| {
                SkillServiceError::invalid(
                    "SKILL_SCRIPT_MISSING",
                    "skill declares no deterministic script",
                    None,
                )
            })?;
        let request = SkillRunRequest {
            snapshot: &loaded.snapshot,
            entry: loaded.manifest.entry.kind(),
            permissions: &loaded.manifest.permissions,
            scripts: &loaded.manifest.scripts,
            script_id: script,
            input: input.clone(),
        };
        let outcome = run_script(&self.runner_config(), request).await?;
        self.db.insert_skill_run(&outcome, &input).await?;
        Ok(outcome)
    }

    pub async fn test(
        &self,
        skill_id: &str,
        version: Option<&str>,
        case: Option<&str>,
    ) -> Result<Vec<TestResult>, SkillServiceError> {
        let loaded = self.load(skill_id, version).await?;
        let tests: Vec<&SkillTest> = loaded
            .manifest
            .tests
            .iter()
            .filter(|test| case.is_none_or(|name| test.name == name))
            .collect();
        if tests.is_empty() {
            return Err(SkillServiceError::NotFound(match case {
                Some(name) => format!("test `{name}` is not declared"),
                None => format!("skill `{skill_id}` declares no tests"),
            }));
        }
        let mut results = Vec::new();
        for test in tests {
            let script_id = test.script.clone().unwrap_or_default();
            let request = SkillRunRequest {
                snapshot: &loaded.snapshot,
                entry: EntryKind::Script,
                permissions: &loaded.manifest.permissions,
                scripts: &loaded.manifest.scripts,
                script_id: script_id.clone(),
                input: test.input.clone(),
            };
            let outcome = match run_script(&self.runner_config(), request).await {
                Ok(outcome) => outcome,
                Err(error) => {
                    results.push(TestResult {
                        name: test.name.clone(),
                        passed: false,
                        failures: vec![format!("{}: {error}", error.code())],
                        run: None,
                        error: Some(error.to_string()),
                    });
                    continue;
                }
            };
            if let Err(error) = self.db.insert_skill_run(&outcome, &test.input).await {
                results.push(TestResult {
                    name: test.name.clone(),
                    passed: false,
                    failures: vec![format!("cannot persist run: {error}")],
                    run: Some(outcome),
                    error: None,
                });
                continue;
            }
            let mut failures = Vec::new();
            let expected_exit = test.expect.exit_code.unwrap_or(0);
            if outcome.exit_code != Some(expected_exit) {
                failures.push(format!(
                    "expected exit code {expected_exit}, got {}",
                    outcome
                        .exit_code
                        .map(|code| code.to_string())
                        .unwrap_or_else(|| "signal".to_string())
                ));
            }
            if let Some(schema) = &test.expect.output_schema {
                match &outcome.output {
                    None => failures.push("expected JSON output".to_string()),
                    Some(output) => match jsonschema::validator_for(schema) {
                        Ok(validator) if validator.is_valid(output) => {}
                        Ok(_) => failures.push("output does not match outputSchema".to_string()),
                        Err(error) => failures.push(format!("invalid outputSchema: {error}")),
                    },
                }
            }
            for artifact in &test.expect.artifacts {
                if !outcome.artifacts.iter().any(|item| &item.name == artifact) {
                    failures.push(format!("missing artifact `{artifact}`"));
                }
            }
            if outcome.status == hpa_skill::RunStatus::TimedOut {
                failures.push("script timed out".to_string());
            }
            results.push(TestResult {
                name: test.name.clone(),
                passed: failures.is_empty(),
                failures,
                run: Some(outcome),
                error: None,
            });
        }
        Ok(results)
    }

    pub async fn runs(
        &self,
        skill_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<SkillRunRow>, SkillServiceError> {
        Ok(self.db.list_skill_runs(skill_id, limit).await?)
    }

    pub async fn artifacts(
        &self,
        run_id: &str,
    ) -> Result<Vec<crate::db::SkillArtifactRow>, SkillServiceError> {
        Ok(self.db.list_skill_run_artifacts(run_id).await?)
    }

    pub async fn skill_artifact(
        &self,
        run_id: &str,
        artifact_id: &str,
    ) -> Result<(crate::db::SkillArtifactRow, Vec<u8>), SkillServiceError> {
        let artifacts = self.db.list_skill_run_artifacts(run_id).await?;
        let artifact = artifacts
            .into_iter()
            .find(|artifact| artifact.id == artifact_id)
            .ok_or_else(|| SkillServiceError::NotFound(artifact_id.to_string()))?;
        let content = std::fs::read(&artifact.stored_path)?;
        Ok((artifact, content))
    }

    async fn resolve_dependencies(
        &self,
        manifest: &SkillManifest,
    ) -> Result<(), SkillServiceError> {
        for dependency in &manifest.dependencies {
            let Some(object) = dependency.as_object() else {
                continue;
            };
            if object.get("type").and_then(Value::as_str) != Some("skill") {
                continue;
            }
            let Some(name) = object
                .get("id")
                .or_else(|| object.get("name"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            let range = object
                .get("range")
                .or_else(|| object.get("version"))
                .and_then(Value::as_str)
                .unwrap_or("*");
            let dependency_skill = match self.db.get_skill(name).await? {
                Some(skill) => Some(skill),
                None => self
                    .db
                    .list_skills()
                    .await?
                    .into_iter()
                    .find(|skill| skill.name == name),
            };
            let Some(dependency_skill) = dependency_skill else {
                return Err(SkillServiceError::invalid(
                    "SKILL_DEPENDENCY",
                    format!("skill dependency `{name}` is not installed"),
                    None,
                ));
            };
            if !version_satisfies(&dependency_skill.latest_version, range) {
                return Err(SkillServiceError::invalid(
                    "SKILL_DEPENDENCY",
                    format!(
                        "skill dependency `{name}` requires `{range}`, installed version is {}",
                        dependency_skill.latest_version
                    ),
                    None,
                ));
            }
        }
        Ok(())
    }
}

fn validate_draft_path(path: &str) -> Result<(), SkillServiceError> {
    let candidate = Path::new(path);
    if path.is_empty()
        || path.contains('\\')
        || path.contains('\0')
        || candidate.is_absolute()
        || candidate
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(SkillServiceError::invalid(
            "SKILL_PATH_ESCAPE",
            format!("draft path `{path}` must be a normalized relative path"),
            None,
        ));
    }
    Ok(())
}

struct LocalSigningKey {
    key_id: String,
    public_key: String,
    signing_key: SigningKey,
}

fn skill_signature_payload(skill_id: &str, version: &str, content_hash: &str) -> String {
    format!("hpa.skill/v1\n{skill_id}@{version}\n{content_hash}")
}

fn verify_skill_signature(
    record: &Value,
    skill_id: &str,
    version: &str,
    content_hash: &str,
) -> bool {
    if record.get("algorithm").and_then(Value::as_str) != Some("ed25519")
        || record.get("contentHash").and_then(Value::as_str) != Some(content_hash)
    {
        return false;
    }
    let Some(public_key) = record.get("publicKey").and_then(Value::as_str) else {
        return false;
    };
    let Some(signature) = record.get("value").and_then(Value::as_str) else {
        return false;
    };
    let Ok(public_key) = BASE64.decode(public_key) else {
        return false;
    };
    let Ok(public_key) = <[u8; 32]>::try_from(public_key) else {
        return false;
    };
    let Ok(verifying_key) = VerifyingKey::from_bytes(&public_key) else {
        return false;
    };
    let Ok(signature) = BASE64.decode(signature) else {
        return false;
    };
    let Ok(signature) = Signature::from_slice(&signature) else {
        return false;
    };
    let mut hasher = Sha256::new();
    hasher.update(public_key);
    let expected_key_id = format!("ed25519-{}", &format!("{:x}", hasher.finalize())[..16]);
    if record.get("keyId").and_then(Value::as_str) != Some(expected_key_id.as_str()) {
        return false;
    }
    let payload = skill_signature_payload(skill_id, version, content_hash);
    verifying_key.verify(payload.as_bytes(), &signature).is_ok()
}

pub fn default_script(snapshot: &SkillSnapshot, manifest: &SkillManifest) -> Option<String> {
    snapshot
        .manifest
        .get("entry")
        .and_then(|entry| entry.get("defaultScript"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| manifest.scripts.first().map(|script| script.id.clone()))
}

pub fn sandbox_label(kind: SandboxKind) -> &'static str {
    match kind {
        SandboxKind::MacosSeatbelt => "macos_seatbelt",
        SandboxKind::LinuxBubblewrap => "linux_bubblewrap",
        SandboxKind::Process => "process",
    }
}

fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use serde_json::json;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/skills")
            .join(name)
    }

    fn temp_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "hpa-skill-service-{label}-{}",
            uuid::Uuid::now_v7()
        ));
        std::fs::create_dir_all(&path).expect("temp dir");
        path
    }

    fn copy_dir(source: &Path, target: &Path) {
        std::fs::create_dir_all(target).expect("copy dir");
        for entry in std::fs::read_dir(source).expect("read dir") {
            let entry = entry.expect("entry");
            let destination = target.join(entry.file_name());
            if entry.file_type().expect("file type").is_dir() {
                copy_dir(&entry.path(), &destination);
            } else {
                std::fs::copy(entry.path(), destination).expect("copy file");
            }
        }
    }

    #[tokio::test]
    async fn changed_content_for_same_version_is_frozen() {
        let root = temp_dir("frozen");
        let db = Db::open(&root.join("hpa.db")).await.expect("db");
        let service = SkillService::new(db, root.clone());
        let source = fixture("invoice-audit");
        service.import(&source).await.expect("import");
        let modified = root.join("modified");
        copy_dir(&source, &modified);
        let reference = modified.join("references/policy.md");
        let mut content = std::fs::read_to_string(&reference).expect("read reference");
        content.push_str("\nchanged\n");
        std::fs::write(&reference, content).expect("write reference");
        let error = service
            .import(&modified)
            .await
            .expect_err("changed content must be frozen");
        assert!(
            matches!(
                error,
                SkillServiceError::Db(DbError::SkillVersionFrozen { .. })
            ),
            "{error:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn source_editor_creates_a_new_immutable_version() {
        let root = temp_dir("editor");
        let db = Db::open(&root.join("hpa.db")).await.expect("db");
        let service = SkillService::new(db, root.clone());
        service
            .import(&fixture("invoice-audit"))
            .await
            .expect("import");
        let source = service
            .source("skill_invoice_audit", Some("1.0.0"))
            .await
            .expect("source");
        assert!(source.files.contains_key("SKILL.md"));
        let mut edits = BTreeMap::new();
        let mut skill_md = source.files.get("SKILL.md").expect("SKILL.md").clone();
        skill_md.push_str("\nEdited through the API.\n");
        edits.insert("SKILL.md".to_string(), skill_md);
        let outcome = service
            .save_draft("skill_invoice_audit", "1.0.0", "1.1.0", edits)
            .await
            .expect("save draft");
        assert_eq!(outcome.version.version, "1.1.0");
        let loaded = service
            .load("skill_invoice_audit", Some("1.1.0"))
            .await
            .expect("load edited version");
        assert!(loaded.snapshot.skill_md.contains("Edited through the API."));
        assert!(
            service
                .save_draft("skill_invoice_audit", "1.0.0", "1.0.0", BTreeMap::new())
                .await
                .is_err()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn local_signature_verifies_and_tampering_blocks_publish() {
        let root = temp_dir("signature");
        let db = Db::open(&root.join("hpa.db")).await.expect("db");
        let service = SkillService::new(db, root.clone());
        service
            .import(&fixture("invoice-audit"))
            .await
            .expect("import");
        let signature = service
            .sign("skill_invoice_audit", Some("1.0.0"))
            .await
            .expect("sign");
        assert_eq!(signature["verified"], true);
        let loaded = service
            .load("skill_invoice_audit", Some("1.0.0"))
            .await
            .expect("load");
        assert_eq!(
            loaded
                .signature
                .as_ref()
                .and_then(|value| value["verified"].as_bool()),
            Some(true)
        );
        service
            .publish("skill_invoice_audit", Some("1.0.0"))
            .await
            .expect("publish signed");

        let path = service.signature_path("skill_invoice_audit", "1.0.0");
        let mut record: Value =
            serde_json::from_slice(&std::fs::read(&path).expect("signature")).expect("json");
        record["value"] = json!(BASE64.encode([0u8; 64]));
        std::fs::write(&path, serde_json::to_vec_pretty(&record).unwrap()).expect("tamper");
        let loaded = service
            .load("skill_invoice_audit", Some("1.0.0"))
            .await
            .expect("load tampered");
        assert_eq!(
            loaded
                .signature
                .as_ref()
                .and_then(|value| value["verified"].as_bool()),
            Some(false)
        );
        let publish = service
            .publish("skill_invoice_audit", Some("1.0.0"))
            .await
            .expect_err("tampered signature must fail");
        assert!(publish.to_string().contains("signature"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn import_publish_test_run_and_export_roundtrip() {
        let root = temp_dir("roundtrip");
        let db = Db::open(&root.join("hpa.db")).await.expect("db");
        let service = SkillService::new(db, root.clone());
        let imported = service
            .import(&fixture("invoice-audit"))
            .await
            .expect("import");
        assert!(imported.inserted);
        let again = service
            .import(&fixture("invoice-audit"))
            .await
            .expect("reimport");
        assert!(!again.inserted);

        assert!(
            service
                .publish("skill_invoice_audit", Some("1.0.0"))
                .await
                .expect("publish")
        );
        let outcome = service
            .run(
                "skill_invoice_audit",
                Some("1.0.0"),
                None,
                json!({
                    "invoiceId": "inv_service",
                    "items": [{"name": "a", "amount": 4.25}]
                }),
            )
            .await
            .expect("run");
        assert!(outcome.succeeded(), "{:?}", outcome.error);
        assert_eq!(outcome.output.as_ref().unwrap()["total"], json!(4.25));
        assert_eq!(outcome.artifacts.len(), 1);

        let results = service
            .test("skill_invoice_audit", Some("1.0.0"), None)
            .await
            .expect("test");
        assert!(results.iter().all(|result| result.passed), "{results:?}");

        let target = root.join("export");
        service
            .export("skill_invoice_audit", Some("1.0.0"), &target)
            .await
            .expect("export");
        assert!(target.join("SKILL.md").is_file());
        assert!(target.join("NOTICE").is_file());
        let exported = hpa_skill::inspect(&target).expect("inspect export");
        assert!(exported.report.is_valid(), "{:?}", exported.report.issues);

        let runs = service
            .runs(Some("skill_invoice_audit"), 20)
            .await
            .expect("runs");
        assert!(runs.len() >= 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn proprietary_skill_cannot_be_published_or_exported() {
        let root = temp_dir("license");
        let db = Db::open(&root.join("hpa.db")).await.expect("db");
        let service = SkillService::new(db, root.clone());
        service
            .import(&fixture("proprietary-demo"))
            .await
            .expect("import");
        let publish = service
            .publish("skill_proprietary_demo", Some("1.0.0"))
            .await
            .expect_err("publish must fail");
        assert!(
            publish.to_string().contains("cannot be published"),
            "{publish}"
        );
        let export = service
            .export(
                "skill_proprietary_demo",
                Some("1.0.0"),
                &root.join("export"),
            )
            .await
            .expect_err("export must fail");
        assert!(matches!(
            export,
            SkillServiceError::Skill(SkillError::LicenseDenied(_))
        ));
        let _ = std::fs::remove_dir_all(&root);
    }
}
