use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillIssue {
    pub code: String,
    pub path: String,
    pub message: String,
    pub severity: Severity,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix_hint: Option<String>,
}

impl SkillIssue {
    pub fn error(
        code: impl Into<String>,
        path: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            path: path.into(),
            message: message.into(),
            severity: Severity::Error,
            fix_hint: None,
        }
    }

    pub fn warning(
        code: impl Into<String>,
        path: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            path: path.into(),
            message: message.into(),
            severity: Severity::Warning,
            fix_hint: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.fix_hint = Some(hint.into());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SkillReport {
    pub issues: Vec<SkillIssue>,
}

impl SkillReport {
    pub fn push(&mut self, issue: SkillIssue) {
        self.issues.push(issue);
    }

    pub fn extend(&mut self, issues: impl IntoIterator<Item = SkillIssue>) {
        self.issues.extend(issues);
    }

    pub fn is_valid(&self) -> bool {
        !self
            .issues
            .iter()
            .any(|issue| issue.severity == Severity::Error)
    }

    pub fn has_code(&self, code: &str) -> bool {
        self.issues.iter().any(|issue| issue.code == code)
    }

    pub fn sort(&mut self) {
        self.issues.sort_by(|left, right| {
            (left.severity as u8, &left.path, &left.code).cmp(&(
                right.severity as u8,
                &right.path,
                &right.code,
            ))
        });
    }
}

#[derive(Debug, Error)]
pub enum SkillError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{code}: {message}")]
    Invalid { code: String, message: String },
    #[error("skill `{0}` was not found")]
    NotFound(String),
    #[error("skill version `{skill_id}@{version}` is frozen")]
    VersionFrozen { skill_id: String, version: String },
    #[error("script `{0}` is not declared by the skill")]
    UnknownScript(String),
    #[error("runtime `{0}` is not available on this host")]
    RuntimeMissing(String),
    #[error("sandbox is unavailable: {0}")]
    SandboxUnavailable(String),
    #[error("sandbox policy rejected the execution: {0}")]
    Permission(String),
    #[error("script timed out after {0} ms")]
    Timeout(u64),
    #[error("script exited with code {code}: {message}")]
    ScriptExit { code: i32, message: String },
    #[error("script output is invalid: {0}")]
    OutputInvalid(String),
    #[error("license `{0}` cannot be published or exported")]
    LicenseDenied(String),
}

impl SkillError {
    pub fn invalid(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Invalid {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn code(&self) -> &str {
        match self {
            Self::Io(_) => "SKILL_IO",
            Self::Json(_) => "SKILL_JSON",
            Self::Invalid { code, .. } => code,
            Self::NotFound(_) => "SKILL_NOT_FOUND",
            Self::VersionFrozen { .. } => "SKILL_VERSION_FROZEN",
            Self::UnknownScript(_) => "SKILL_SCRIPT_NOT_FOUND",
            Self::RuntimeMissing(_) => "SKILL_RUNTIME_MISSING",
            Self::SandboxUnavailable(_) => "SKILL_SANDBOX",
            Self::Permission(_) => "SKILL_PERMISSION",
            Self::Timeout(_) => "SCRIPT_TIMEOUT",
            Self::ScriptExit { .. } => "SCRIPT_EXIT",
            Self::OutputInvalid(_) => "SCRIPT_OUTPUT_INVALID",
            Self::LicenseDenied(_) => "SKILL_LICENSE",
        }
    }
}

pub type SkillResult<T> = Result<T, SkillError>;
