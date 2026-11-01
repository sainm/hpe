use serde_json::Value;

pub const IR_SCHEMA_TEXT: &str = include_str!("../../../schema/ir/v0.1/workflow.schema.json");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaIssue {
    pub path: String,
    pub message: String,
}

pub fn schema_value() -> Result<Value, SchemaIssue> {
    serde_json::from_str(IR_SCHEMA_TEXT).map_err(|error| SchemaIssue {
        path: "$".to_string(),
        message: format!("embedded IR schema is not valid JSON: {error}"),
    })
}

pub fn validate_value(instance: &Value) -> Result<(), Vec<SchemaIssue>> {
    let schema = schema_value().map_err(|issue| vec![issue])?;
    let validator = jsonschema::validator_for(&schema).map_err(|error| {
        vec![SchemaIssue {
            path: "$".to_string(),
            message: format!("embedded IR schema does not compile: {error}"),
        }]
    })?;

    let issues: Vec<SchemaIssue> = validator
        .iter_errors(instance)
        .map(|error| {
            let pointer = error.instance_path().to_string();
            let path = if pointer.is_empty() {
                "$".to_string()
            } else {
                format!("${pointer}")
            };
            SchemaIssue {
                path,
                message: error.to_string(),
            }
        })
        .collect();

    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues)
    }
}
