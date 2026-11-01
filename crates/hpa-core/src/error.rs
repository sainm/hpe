use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("YAML error: {0}")]
    Yaml(String),
    #[error("unsupported schema version `{found}` (expected `{expected}`)")]
    SchemaVersion {
        found: String,
        expected: &'static str,
    },
    #[error("IR schema validation failed: {0}")]
    Schema(String),
    #[error("expression `{expression}` is invalid: {message}")]
    Expression { expression: String, message: String },
    #[error("expression `{expression}` is not allowed: {message}")]
    ExpressionForbidden { expression: String, message: String },
    #[error("expression `{expression}` produced {count} values; expected exactly one")]
    ExpressionArity { expression: String, count: usize },
    #[error("expression `{expression}` produced a value that is not valid JSON: {message}")]
    ExpressionValue { expression: String, message: String },
    #[error("invalid workflow: {0}")]
    Invalid(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("runtime error: {0}")]
    Runtime(String),
}

pub type CoreResult<T> = Result<T, CoreError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    pub code: String,
    pub path: String,
    pub message: String,
    pub severity: Severity,
}

impl Issue {
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
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationReport {
    pub issues: Vec<Issue>,
}

impl ValidationReport {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, issue: Issue) {
        self.issues.push(issue);
    }

    pub fn extend(&mut self, issues: impl IntoIterator<Item = Issue>) {
        self.issues.extend(issues);
    }

    pub fn is_valid(&self) -> bool {
        !self
            .issues
            .iter()
            .any(|issue| issue.severity == Severity::Error)
    }

    pub fn error_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|issue| issue.severity == Severity::Error)
            .count()
    }

    pub fn warning_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|issue| issue.severity == Severity::Warning)
            .count()
    }

    pub fn has_code(&self, code: &str) -> bool {
        self.issues.iter().any(|issue| issue.code == code)
    }

    pub fn errors(&self) -> impl Iterator<Item = &Issue> {
        self.issues
            .iter()
            .filter(|issue| issue.severity == Severity::Error)
    }

    pub fn warnings(&self) -> impl Iterator<Item = &Issue> {
        self.issues
            .iter()
            .filter(|issue| issue.severity == Severity::Warning)
    }

    pub fn sort(&mut self) {
        self.issues
            .sort_by(|a, b| (&a.path, &a.code, &a.message).cmp(&(&b.path, &b.code, &b.message)));
    }
}
