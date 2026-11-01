use std::path::Path;

use crate::error::{Severity, SkillError, SkillIssue, SkillResult};
use crate::loader::safe_relative_path;
use crate::manifest::{
    SkillInspection, SkillManifest, SkillSnapshot, allowed_license, denied_license,
};

pub fn publish_snapshot_issues(
    snapshot: &SkillSnapshot,
    manifest: &SkillManifest,
) -> Vec<SkillIssue> {
    let mut issues = Vec::new();
    match snapshot.license.as_deref() {
        None => issues.push(SkillIssue::error(
            "SKILL_LICENSE_MISSING",
            "hpa.skill.json.license",
            "a license is required before publishing",
        )),
        Some(value) if denied_license(value) => issues.push(SkillIssue::error(
            "SKILL_LICENSE_DENIED",
            "hpa.skill.json.license",
            format!("license `{value}` cannot be published"),
        )),
        Some(value) if !allowed_license(value) => issues.push(SkillIssue::error(
            "SKILL_LICENSE_UNKNOWN",
            "hpa.skill.json.license",
            format!("license `{value}` is not permitted for local publishing"),
        )),
        _ => {}
    }
    if !manifest.scripts.is_empty() && manifest.tests.is_empty() {
        issues.push(SkillIssue::error(
            "SKILL_TEST_MISSING",
            "hpa.skill.json.tests",
            "at least one smoke test is required before publishing a scripted skill",
        ));
    }
    issues
}

pub fn publish_issues(inspection: &SkillInspection) -> Vec<SkillIssue> {
    publish_snapshot_issues(&inspection.snapshot, &inspection.manifest())
}

pub fn ensure_publishable(inspection: &SkillInspection) -> SkillResult<()> {
    if !inspection.report.is_valid() {
        return Err(SkillError::invalid(
            "SKILL_INVALID",
            "skill has validation errors",
        ));
    }
    let issues = publish_issues(inspection);
    if let Some(issue) = issues
        .iter()
        .find(|issue| issue.severity == Severity::Error)
    {
        return Err(SkillError::invalid(
            issue.code.clone(),
            issue.message.clone(),
        ));
    }
    Ok(())
}

pub fn export_snapshot(snapshot: &SkillSnapshot, target: &Path) -> SkillResult<()> {
    let license = snapshot.license.as_deref();
    match license {
        Some(value) if allowed_license(value) => {}
        Some(value) => return Err(SkillError::LicenseDenied(value.to_string())),
        None => {
            return Err(SkillError::LicenseDenied("missing license".to_string()));
        }
    }
    if !snapshot.files.iter().any(|file| file.path == "SKILL.md") {
        return Err(SkillError::invalid(
            "SKILL_MD_MISSING",
            "snapshot has no SKILL.md",
        ));
    }
    if target.exists() {
        let mut entries = std::fs::read_dir(target)?;
        if entries.next().is_some() {
            return Err(SkillError::invalid(
                "SKILL_EXPORT_EXISTS",
                format!("export target `{}` is not empty", target.display()),
            ));
        }
    } else {
        std::fs::create_dir_all(target)?;
    }
    for file in &snapshot.files {
        if !safe_relative_path(&file.path) {
            return Err(SkillError::invalid(
                "SKILL_PATH",
                format!("unsafe export path `{}`", file.path),
            ));
        }
        let destination = target.join(&file.path);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(destination, &file.content)?;
    }
    Ok(())
}
