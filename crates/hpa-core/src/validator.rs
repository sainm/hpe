use serde_json::Value;

use crate::compiler::{Compilation, compile_workflow};
use crate::error::{Issue, ValidationReport};
use crate::ir::Workflow;
use crate::{schema, yaml};

pub fn validate_value(value: &Value) -> ValidationReport {
    match check_value(value) {
        Ok(compilation) => ValidationReport {
            issues: compilation.issues,
        },
        Err(report) => report,
    }
}

pub fn validate_json(text: &str) -> ValidationReport {
    match check_json(text) {
        Ok(compilation) => ValidationReport {
            issues: compilation.issues,
        },
        Err(report) => report,
    }
}

pub fn validate_yaml(text: &str) -> ValidationReport {
    match check_yaml(text) {
        Ok(compilation) => ValidationReport {
            issues: compilation.issues,
        },
        Err(report) => report,
    }
}

pub fn check_value(value: &Value) -> Result<Compilation, ValidationReport> {
    let mut report = ValidationReport::new();
    if let Err(issues) = schema::validate_value(value) {
        report.extend(
            issues
                .into_iter()
                .map(|issue| Issue::error("SCHEMA_INVALID", issue.path, issue.message)),
        );
        report.sort();
        return Err(report);
    }
    let workflow = match Workflow::from_value(value) {
        Ok(workflow) => workflow,
        Err(error) => {
            report.push(Issue::error("IR_INVALID", "$", error.to_string()));
            return Err(report);
        }
    };
    check_workflow(&workflow)
}

pub fn check_workflow(workflow: &Workflow) -> Result<Compilation, ValidationReport> {
    let mut report = ValidationReport::new();
    match workflow.to_value() {
        Ok(value) => {
            if let Err(issues) = schema::validate_value(&value) {
                report.extend(
                    issues
                        .into_iter()
                        .map(|issue| Issue::error("SCHEMA_INVALID", issue.path, issue.message)),
                );
                report.sort();
                return Err(report);
            }
        }
        Err(error) => {
            report.push(Issue::error("IR_INVALID", "$", error.to_string()));
            return Err(report);
        }
    }

    match compile_workflow(workflow) {
        Ok(compilation) => {
            report.extend(compilation.issues.clone());
            report.sort();
            if report.is_valid() {
                Ok(compilation)
            } else {
                Err(report)
            }
        }
        Err(issues) => {
            report.extend(issues);
            report.sort();
            Err(report)
        }
    }
}

pub fn check_json(text: &str) -> Result<Compilation, ValidationReport> {
    let value: Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(error) => return Err(parse_report("JSON_PARSE", error.to_string())),
    };
    check_value(&value)
}

pub fn check_yaml(text: &str) -> Result<Compilation, ValidationReport> {
    let value = match yaml::yaml_to_json(text) {
        Ok(value) => value,
        Err(error) => return Err(parse_report("YAML_PARSE", error.to_string())),
    };
    check_value(&value)
}

fn parse_report(code: &str, message: String) -> ValidationReport {
    let mut report = ValidationReport::new();
    report.push(Issue::error(code, "$", message));
    report
}
