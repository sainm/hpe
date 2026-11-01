use std::path::{Path, PathBuf};

use hpa_skill::{
    EntryKind, RunOutcome, RunnerConfig, SandboxKind, SkillError, SkillManifest, SkillReport,
    SkillRunRequest, SkillSnapshot, SkillTest, publish_snapshot_issues, run_script,
    version_satisfies,
};
use serde::Serialize;
use serde_json::Value;
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
        Ok(LoadedSkill {
            version: row,
            snapshot,
            manifest,
            report,
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
