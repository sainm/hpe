use saphyr::{LoadableYamlNode, Scalar, Yaml, YamlEmitter};
use serde_json::{Map, Number, Value};

use crate::error::{CoreError, CoreResult};

pub fn yaml_to_json(text: &str) -> CoreResult<Value> {
    let documents =
        Yaml::load_from_str(text).map_err(|error| CoreError::Yaml(error.to_string()))?;
    let document = documents
        .first()
        .ok_or_else(|| CoreError::Yaml("document is empty".to_string()))?;
    if documents.len() > 1 {
        return Err(CoreError::Yaml(format!(
            "expected a single YAML document, found {}",
            documents.len()
        )));
    }
    convert(document, "$")
}

pub fn json_to_yaml(value: &Value) -> CoreResult<String> {
    let json = serde_json::to_string_pretty(value)?;
    let documents = Yaml::load_from_str(&json)
        .map_err(|error| CoreError::Yaml(format!("cannot prepare YAML output: {error}")))?;
    let document = documents
        .first()
        .ok_or_else(|| CoreError::Yaml("document is empty".to_string()))?;
    let mut out = String::new();
    {
        let mut emitter = YamlEmitter::new(&mut out);
        emitter
            .dump(document)
            .map_err(|error| CoreError::Yaml(format!("cannot emit YAML: {error:?}")))?;
    }
    if let Some(rest) = out.strip_prefix("---\n") {
        out = rest.to_string();
    } else if let Some(rest) = out.strip_prefix("---") {
        out = rest.to_string();
    }
    Ok(out)
}

fn convert(node: &Yaml<'_>, path: &str) -> CoreResult<Value> {
    match node {
        Yaml::Value(scalar) => scalar_to_json(scalar, path),
        Yaml::Representation(value, style, tag) => {
            let parsed =
                Scalar::parse_from_cow_and_metadata(value.clone(), *style, tag.as_ref())
                    .ok_or_else(|| CoreError::Yaml(format!("cannot resolve scalar at {path}")))?;
            scalar_to_json(&parsed, path)
        }
        Yaml::Sequence(items) => {
            let mut out = Vec::with_capacity(items.len());
            for (index, item) in items.iter().enumerate() {
                out.push(convert(item, &format!("{path}[{index}]"))?);
            }
            Ok(Value::Array(out))
        }
        Yaml::Mapping(mapping) => {
            let mut out = Map::new();
            for (key, value) in mapping.iter() {
                let key = key_to_string(key, path)?;
                let child_path = format!("{path}.{}", escape_pointer(&key));
                if out.contains_key(&key) {
                    return Err(CoreError::Yaml(format!("duplicate key `{key}` at {path}")));
                }
                out.insert(key, convert(value, &child_path)?);
            }
            Ok(Value::Object(out))
        }
        Yaml::Tagged(tag, _) => Err(CoreError::Yaml(format!(
            "custom YAML tag `{tag:?}` at {path} is not supported"
        ))),
        Yaml::Alias(_) => Err(CoreError::Yaml(format!(
            "YAML aliases are not supported at {path}"
        ))),
        Yaml::BadValue => Err(CoreError::Yaml(format!("invalid scalar value at {path}"))),
    }
}

fn scalar_to_json(scalar: &Scalar<'_>, path: &str) -> CoreResult<Value> {
    match scalar {
        Scalar::Null => Ok(Value::Null),
        Scalar::Boolean(value) => Ok(Value::Bool(*value)),
        Scalar::Integer(value) => Ok(Value::Number(Number::from(*value))),
        Scalar::FloatingPoint(value) => Number::from_f64(value.into_inner())
            .map(Value::Number)
            .ok_or_else(|| CoreError::Yaml(format!("non-finite number at {path}"))),
        Scalar::String(value) => Ok(Value::String(value.to_string())),
    }
}

fn key_to_string(key: &Yaml<'_>, path: &str) -> CoreResult<String> {
    match key {
        Yaml::Value(Scalar::String(value)) => Ok(value.to_string()),
        Yaml::Representation(value, style, tag) => {
            let parsed = Scalar::parse_from_cow_and_metadata(value.clone(), *style, tag.as_ref())
                .ok_or_else(|| {
                CoreError::Yaml(format!("cannot resolve mapping key at {path}"))
            })?;
            match parsed {
                Scalar::String(value) => Ok(value.to_string()),
                _ => Err(CoreError::Yaml(format!(
                    "mapping key at {path} must be a string"
                ))),
            }
        }
        _ => Err(CoreError::Yaml(format!(
            "mapping key at {path} must be a string"
        ))),
    }
}

fn escape_pointer(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}
