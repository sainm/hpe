use hpa_skill::{RunOutcome, RunStatus, SandboxKind, SkillReport, SkillSnapshot};
use serde_json::Value;
use sqlx::Row;
use sqlx::postgres::{PgPool, PgRow};
use sqlx::types::Json;
use thiserror::Error;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

#[derive(Debug, Error)]
pub enum PostgresSkillError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid skill data: {0}")]
    Invalid(String),
    #[error("skill version `{skill_id}@{version}` is frozen")]
    VersionFrozen { skill_id: String, version: String },
}

#[derive(Debug, Clone)]
pub struct SkillSummaryRecord {
    pub id: String,
    pub name: String,
    pub latest_version: String,
    pub license: Option<String>,
    pub status: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct SkillVersionRecord {
    pub skill_id: String,
    pub version: String,
    pub name: String,
    pub license: Option<String>,
    pub manifest_json: String,
    pub skill_md: String,
    pub report_json: String,
    pub content_hash: String,
    pub status: String,
    pub source_path: Option<String>,
    pub imported_at: String,
    pub published_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SkillFileRecord {
    pub path: String,
    pub sha256: String,
    pub size: i64,
    pub content: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct SkillRunRecord {
    pub id: String,
    pub skill_id: String,
    pub version: String,
    pub script_id: String,
    pub status: String,
    pub input_json: String,
    pub output_json: Option<String>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub exit_code: Option<i64>,
    pub error_json: Option<String>,
    pub sandbox: String,
    pub duration_ms: i64,
    pub started_at: String,
    pub finished_at: String,
}

#[derive(Debug, Clone)]
pub struct SkillArtifactRecord {
    pub id: String,
    pub run_id: String,
    pub name: String,
    pub sha256: String,
    pub size: i64,
    pub mime_type: String,
    pub stored_path: String,
    pub created_at: String,
}

#[derive(Clone)]
pub struct PostgresSkillStore {
    pool: PgPool,
    tenant_id: String,
}

impl PostgresSkillStore {
    pub fn new(pool: PgPool, tenant_id: impl Into<String>) -> Self {
        Self {
            pool,
            tenant_id: tenant_id.into(),
        }
    }

    async fn begin(&self) -> Result<sqlx::Transaction<'_, sqlx::Postgres>, PostgresSkillError> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query("SET LOCAL ROLE hpa_tenant_runtime")
            .execute(&mut *transaction)
            .await?;
        sqlx::query("SELECT set_config('hpa.tenant_id', $1, true)")
            .bind(&self.tenant_id)
            .execute(&mut *transaction)
            .await?;
        Ok(transaction)
    }

    pub async fn import_skill(
        &self,
        snapshot: &SkillSnapshot,
        report: &SkillReport,
        source_path: Option<&str>,
        imported_at: &str,
    ) -> Result<bool, PostgresSkillError> {
        let mut transaction = self.begin().await?;
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT content_hash FROM hpa_tenant.skill_versions
             WHERE tenant_id = $1 AND skill_id = $2 AND version = $3",
        )
        .bind(&self.tenant_id)
        .bind(&snapshot.id)
        .bind(&snapshot.version)
        .fetch_optional(&mut *transaction)
        .await?;
        if let Some(hash) = existing {
            if hash == snapshot.content_hash {
                transaction.commit().await?;
                return Ok(false);
            }
            return Err(PostgresSkillError::VersionFrozen {
                skill_id: snapshot.id.clone(),
                version: snapshot.version.clone(),
            });
        }
        let report_json = serde_json::to_value(report)?;
        sqlx::query(
            "INSERT INTO hpa_tenant.skills
             (tenant_id, id, name, latest_version, license, status, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, 'imported', $6, $6)
             ON CONFLICT (tenant_id, id) DO UPDATE
             SET name = EXCLUDED.name,
                 latest_version = EXCLUDED.latest_version,
                 license = EXCLUDED.license,
                 status = EXCLUDED.status,
                 updated_at = EXCLUDED.updated_at",
        )
        .bind(&self.tenant_id)
        .bind(&snapshot.id)
        .bind(&snapshot.name)
        .bind(&snapshot.version)
        .bind(&snapshot.license)
        .bind(parse_time(imported_at)?)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.skill_versions
             (tenant_id, skill_id, version, name, license, manifest_json, skill_md,
              report_json, content_hash, status, source_path, imported_at, published_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'imported', $10, $11, NULL)",
        )
        .bind(&self.tenant_id)
        .bind(&snapshot.id)
        .bind(&snapshot.version)
        .bind(&snapshot.name)
        .bind(&snapshot.license)
        .bind(Json(&snapshot.manifest))
        .bind(&snapshot.skill_md)
        .bind(Json(&report_json))
        .bind(&snapshot.content_hash)
        .bind(source_path)
        .bind(parse_time(imported_at)?)
        .execute(&mut *transaction)
        .await?;
        for file in &snapshot.files {
            sqlx::query(
                "INSERT INTO hpa_tenant.skill_files
                 (tenant_id, skill_id, version, path, sha256, size, content)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)",
            )
            .bind(&self.tenant_id)
            .bind(&snapshot.id)
            .bind(&snapshot.version)
            .bind(&file.path)
            .bind(&file.sha256)
            .bind(file.size as i64)
            .bind(&file.content)
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(true)
    }

    pub async fn list_skills(&self) -> Result<Vec<SkillSummaryRecord>, PostgresSkillError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT id, name, latest_version, license, status, updated_at
             FROM hpa_tenant.skills WHERE tenant_id = $1 ORDER BY updated_at DESC",
        )
        .bind(&self.tenant_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.iter().map(skill_summary_from_row).collect()
    }

    pub async fn get_skill(
        &self,
        skill_id: &str,
    ) -> Result<Option<SkillSummaryRecord>, PostgresSkillError> {
        let mut transaction = self.begin().await?;
        let row = sqlx::query(
            "SELECT id, name, latest_version, license, status, updated_at
             FROM hpa_tenant.skills WHERE tenant_id = $1 AND id = $2",
        )
        .bind(&self.tenant_id)
        .bind(skill_id)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.as_ref().map(skill_summary_from_row).transpose()
    }

    pub async fn list_skill_versions(
        &self,
        skill_id: &str,
    ) -> Result<Vec<SkillVersionRecord>, PostgresSkillError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT skill_id, version, name, license, manifest_json, skill_md, report_json,
                    content_hash, status, source_path, imported_at, published_at
             FROM hpa_tenant.skill_versions
             WHERE tenant_id = $1 AND skill_id = $2 ORDER BY imported_at DESC",
        )
        .bind(&self.tenant_id)
        .bind(skill_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.iter().map(skill_version_from_row).collect()
    }

    pub async fn get_skill_version(
        &self,
        skill_id: &str,
        version: &str,
    ) -> Result<Option<SkillVersionRecord>, PostgresSkillError> {
        let mut transaction = self.begin().await?;
        let row = sqlx::query(
            "SELECT skill_id, version, name, license, manifest_json, skill_md, report_json,
                    content_hash, status, source_path, imported_at, published_at
             FROM hpa_tenant.skill_versions
             WHERE tenant_id = $1 AND skill_id = $2 AND version = $3",
        )
        .bind(&self.tenant_id)
        .bind(skill_id)
        .bind(version)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.as_ref().map(skill_version_from_row).transpose()
    }

    pub async fn get_skill_files(
        &self,
        skill_id: &str,
        version: &str,
    ) -> Result<Vec<SkillFileRecord>, PostgresSkillError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT path, sha256, size, content FROM hpa_tenant.skill_files
             WHERE tenant_id = $1 AND skill_id = $2 AND version = $3 ORDER BY path ASC",
        )
        .bind(&self.tenant_id)
        .bind(skill_id)
        .bind(version)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.iter()
            .map(|row| {
                Ok(SkillFileRecord {
                    path: row.try_get("path")?,
                    sha256: row.try_get("sha256")?,
                    size: row.try_get("size")?,
                    content: row.try_get("content")?,
                })
            })
            .collect()
    }

    pub async fn publish_skill_version(
        &self,
        skill_id: &str,
        version: &str,
        published_at: &str,
    ) -> Result<bool, PostgresSkillError> {
        let published_at = parse_time(published_at)?;
        let mut transaction = self.begin().await?;
        let result = sqlx::query(
            "UPDATE hpa_tenant.skill_versions
             SET status = 'published', published_at = $1
             WHERE tenant_id = $2 AND skill_id = $3 AND version = $4
               AND status != 'published'",
        )
        .bind(published_at)
        .bind(&self.tenant_id)
        .bind(skill_id)
        .bind(version)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE hpa_tenant.skills
             SET status = 'published', updated_at = $1
             WHERE tenant_id = $2 AND id = $3 AND latest_version = $4",
        )
        .bind(published_at)
        .bind(&self.tenant_id)
        .bind(skill_id)
        .bind(version)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn insert_skill_run(
        &self,
        outcome: &RunOutcome,
        input: &Value,
    ) -> Result<(), PostgresSkillError> {
        let status = match outcome.status {
            RunStatus::Succeeded => "succeeded",
            RunStatus::Failed => "failed",
            RunStatus::TimedOut => "timed_out",
        };
        let sandbox = match outcome.sandbox {
            SandboxKind::MacosSeatbelt => "macos_seatbelt",
            SandboxKind::LinuxBubblewrap => "linux_bubblewrap",
            SandboxKind::Process => "process",
        };
        let error_json = outcome
            .error
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?;
        let mut transaction = self.begin().await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.skill_runs
             (tenant_id, id, skill_id, version, script_id, status, input_json, output_json,
              stdout, stderr, exit_code, error_json, sandbox, duration_ms, started_at,
              finished_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)",
        )
        .bind(&self.tenant_id)
        .bind(&outcome.id)
        .bind(&outcome.skill_id)
        .bind(&outcome.version)
        .bind(&outcome.script_id)
        .bind(status)
        .bind(Json(input))
        .bind(outcome.output.as_ref().map(Json))
        .bind(&outcome.stdout)
        .bind(&outcome.stderr)
        .bind(outcome.exit_code.map(i64::from))
        .bind(error_json.map(Json))
        .bind(sandbox)
        .bind(outcome.duration_ms as i64)
        .bind(parse_time(&outcome.started_at)?)
        .bind(parse_time(&outcome.finished_at)?)
        .execute(&mut *transaction)
        .await?;
        for artifact in &outcome.artifacts {
            sqlx::query(
                "INSERT INTO hpa_tenant.skill_run_artifacts
                 (tenant_id, id, run_id, name, sha256, size, mime_type, stored_path, created_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
            )
            .bind(&self.tenant_id)
            .bind(&artifact.id)
            .bind(&outcome.id)
            .bind(&artifact.name)
            .bind(&artifact.sha256)
            .bind(artifact.size as i64)
            .bind(&artifact.mime_type)
            .bind(artifact.stored_path.display().to_string())
            .bind(parse_time(&outcome.finished_at)?)
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn list_skill_runs(
        &self,
        skill_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<SkillRunRecord>, PostgresSkillError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT id, skill_id, version, script_id, status, input_json, output_json, stdout,
                    stderr, exit_code, error_json, sandbox, duration_ms, started_at, finished_at
             FROM hpa_tenant.skill_runs
             WHERE tenant_id = $1 AND ($2::text IS NULL OR skill_id = $2)
             ORDER BY started_at DESC LIMIT $3",
        )
        .bind(&self.tenant_id)
        .bind(skill_id)
        .bind(limit.clamp(1, 200))
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.iter().map(skill_run_from_row).collect()
    }

    pub async fn get_skill_run(
        &self,
        run_id: &str,
    ) -> Result<Option<SkillRunRecord>, PostgresSkillError> {
        let mut transaction = self.begin().await?;
        let row = sqlx::query(
            "SELECT id, skill_id, version, script_id, status, input_json, output_json, stdout,
                    stderr, exit_code, error_json, sandbox, duration_ms, started_at, finished_at
             FROM hpa_tenant.skill_runs WHERE tenant_id = $1 AND id = $2",
        )
        .bind(&self.tenant_id)
        .bind(run_id)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.as_ref().map(skill_run_from_row).transpose()
    }

    pub async fn list_skill_run_artifacts(
        &self,
        run_id: &str,
    ) -> Result<Vec<SkillArtifactRecord>, PostgresSkillError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT id, run_id, name, sha256, size, mime_type, stored_path, created_at
             FROM hpa_tenant.skill_run_artifacts
             WHERE tenant_id = $1 AND run_id = $2 ORDER BY name ASC",
        )
        .bind(&self.tenant_id)
        .bind(run_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.iter().map(skill_artifact_from_row).collect()
    }

    pub async fn count_skills(&self) -> Result<i64, PostgresSkillError> {
        let mut transaction = self.begin().await?;
        let count: i64 = sqlx::query_scalar(
            "SELECT count(DISTINCT id) FROM hpa_tenant.skills WHERE tenant_id = $1",
        )
        .bind(&self.tenant_id)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(count)
    }
}

fn skill_summary_from_row(row: &PgRow) -> Result<SkillSummaryRecord, PostgresSkillError> {
    Ok(SkillSummaryRecord {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        latest_version: row.try_get("latest_version")?,
        license: row.try_get("license")?,
        status: row.try_get("status")?,
        updated_at: format_time(row.try_get("updated_at")?),
    })
}

fn skill_version_from_row(row: &PgRow) -> Result<SkillVersionRecord, PostgresSkillError> {
    let manifest: Value = row.try_get("manifest_json")?;
    let report: Value = row.try_get("report_json")?;
    Ok(SkillVersionRecord {
        skill_id: row.try_get("skill_id")?,
        version: row.try_get("version")?,
        name: row.try_get("name")?,
        license: row.try_get("license")?,
        manifest_json: serde_json::to_string(&manifest)?,
        skill_md: row.try_get("skill_md")?,
        report_json: serde_json::to_string(&report)?,
        content_hash: row.try_get("content_hash")?,
        status: row.try_get("status")?,
        source_path: row.try_get("source_path")?,
        imported_at: format_time(row.try_get("imported_at")?),
        published_at: row
            .try_get::<Option<OffsetDateTime>, _>("published_at")?
            .map(format_time),
    })
}

fn skill_run_from_row(row: &PgRow) -> Result<SkillRunRecord, PostgresSkillError> {
    let input: Value = row.try_get("input_json")?;
    let output: Option<Value> = row.try_get("output_json")?;
    let error: Option<Value> = row.try_get("error_json")?;
    Ok(SkillRunRecord {
        id: row.try_get("id")?,
        skill_id: row.try_get("skill_id")?,
        version: row.try_get("version")?,
        script_id: row.try_get("script_id")?,
        status: row.try_get("status")?,
        input_json: serde_json::to_string(&input)?,
        output_json: output
            .map(|value| serde_json::to_string(&value))
            .transpose()?,
        stdout: row.try_get("stdout")?,
        stderr: row.try_get("stderr")?,
        exit_code: row.try_get::<Option<i32>, _>("exit_code")?.map(i64::from),
        error_json: error
            .map(|value| serde_json::to_string(&value))
            .transpose()?,
        sandbox: row.try_get("sandbox")?,
        duration_ms: row.try_get("duration_ms")?,
        started_at: format_time(row.try_get("started_at")?),
        finished_at: format_time(row.try_get("finished_at")?),
    })
}

fn skill_artifact_from_row(row: &PgRow) -> Result<SkillArtifactRecord, PostgresSkillError> {
    Ok(SkillArtifactRecord {
        id: row.try_get("id")?,
        run_id: row.try_get("run_id")?,
        name: row.try_get("name")?,
        sha256: row.try_get("sha256")?,
        size: row.try_get("size")?,
        mime_type: row.try_get("mime_type")?,
        stored_path: row.try_get("stored_path")?,
        created_at: format_time(row.try_get("created_at")?),
    })
}

fn parse_time(raw: &str) -> Result<OffsetDateTime, PostgresSkillError> {
    OffsetDateTime::parse(raw, &Rfc3339)
        .map_err(|error| PostgresSkillError::Invalid(format!("invalid timestamp `{raw}`: {error}")))
}

fn format_time(value: OffsetDateTime) -> String {
    value.format(&Rfc3339).unwrap_or_default()
}
