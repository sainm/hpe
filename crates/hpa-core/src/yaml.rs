use saphyr::{
    LoadableYamlNode, MarkedYamlOwned, Scalar, ScalarOwned, Yaml, YamlDataOwned, YamlEmitter,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};
use sha2::{Digest, Sha256};

use crate::error::{CoreError, CoreResult};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcePosition {
    /// UTF-8 byte offset in the original source.
    pub offset: usize,
    /// One-based source line.
    pub line: usize,
    /// Zero-based character column.
    pub column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceLocation {
    /// JSON-pointer-like path rooted at `$`.
    pub path: String,
    pub start: SourcePosition,
    pub end: SourcePosition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceMap {
    pub source_hash: String,
    pub entries: Vec<SourceLocation>,
}

impl SourceMap {
    pub fn location(&self, path: &str) -> Option<&SourceLocation> {
        self.entries.iter().find(|entry| entry.path == path)
    }
}

pub fn yaml_to_json(text: &str) -> CoreResult<Value> {
    yaml_to_json_with_source_map(text).map(|(value, _)| value)
}

pub fn yaml_to_json_with_source_map(text: &str) -> CoreResult<(Value, SourceMap)> {
    let documents =
        MarkedYamlOwned::load_from_str(text).map_err(|error| CoreError::Yaml(error.to_string()))?;
    let document = documents
        .first()
        .ok_or_else(|| CoreError::Yaml("document is empty".to_string()))?;
    if documents.len() > 1 {
        return Err(CoreError::Yaml(format!(
            "expected a single YAML document, found {}",
            documents.len()
        )));
    }
    let mut entries = Vec::new();
    let value = convert_marked(document, "$", text, &mut entries)?;
    let source_hash = format!("{:x}", Sha256::digest(text.as_bytes()));
    Ok((
        value,
        SourceMap {
            source_hash,
            entries,
        },
    ))
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

fn convert_marked(
    node: &MarkedYamlOwned,
    path: &str,
    source: &str,
    entries: &mut Vec<SourceLocation>,
) -> CoreResult<Value> {
    entries.push(SourceLocation {
        path: path.to_string(),
        start: source_position(
            source,
            node.span.start.index(),
            node.span.start.line(),
            node.span.start.col(),
        ),
        end: source_position(
            source,
            node.span.end.index(),
            node.span.end.line(),
            node.span.end.col(),
        ),
    });
    match &node.data {
        YamlDataOwned::Value(scalar) => scalar_to_json(&scalar.as_scalar(), path),
        YamlDataOwned::Representation(value, style, tag) => {
            let tag = tag.as_ref().map(std::borrow::Cow::Borrowed);
            let parsed =
                Scalar::parse_from_cow_and_metadata(value.clone().into(), *style, tag.as_ref())
                    .ok_or_else(|| CoreError::Yaml(format!("cannot resolve scalar at {path}")))?;
            scalar_to_json(&parsed, path)
        }
        YamlDataOwned::Sequence(items) => {
            let mut out = Vec::with_capacity(items.len());
            for (index, item) in items.iter().enumerate() {
                out.push(convert_marked(
                    item,
                    &format!("{path}[{index}]"),
                    source,
                    entries,
                )?);
            }
            Ok(Value::Array(out))
        }
        YamlDataOwned::Mapping(mapping) => {
            let mut out = Map::new();
            for (key, value) in mapping.iter() {
                let key = marked_key_to_string(key, path)?;
                let child_path = format!("{path}.{}", escape_pointer(&key));
                if out.contains_key(&key) {
                    return Err(CoreError::Yaml(format!("duplicate key `{key}` at {path}")));
                }
                out.insert(key, convert_marked(value, &child_path, source, entries)?);
            }
            Ok(Value::Object(out))
        }
        YamlDataOwned::Tagged(tag, _) => Err(CoreError::Yaml(format!(
            "custom YAML tag `{tag:?}` at {path} is not supported"
        ))),
        YamlDataOwned::Alias(_) => Err(CoreError::Yaml(format!(
            "YAML aliases are not supported at {path}"
        ))),
        YamlDataOwned::BadValue => Err(CoreError::Yaml(format!("invalid scalar value at {path}"))),
    }
}

fn marked_key_to_string(key: &MarkedYamlOwned, path: &str) -> CoreResult<String> {
    match &key.data {
        YamlDataOwned::Value(ScalarOwned::String(value)) => Ok(value.to_string()),
        YamlDataOwned::Representation(value, style, tag) => {
            let tag = tag.as_ref().map(std::borrow::Cow::Borrowed);
            let parsed =
                Scalar::parse_from_cow_and_metadata(value.clone().into(), *style, tag.as_ref())
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

fn source_position(source: &str, char_index: usize, line: usize, column: usize) -> SourcePosition {
    let offset = source
        .char_indices()
        .nth(char_index)
        .map(|(offset, _)| offset)
        .unwrap_or(source.len());
    SourcePosition {
        offset,
        line,
        column,
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

fn escape_pointer(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod source_map_tests {
    use super::*;

    #[test]
    fn source_map_tracks_unicode_offsets_and_paths() {
        let source = "name: 日本語\nnodes:\n  - id: first\n    type: start\n";
        let (value, map) = yaml_to_json_with_source_map(source).expect("yaml");
        assert_eq!(value["nodes"][0]["id"], "first");
        let location = map.location("$.name").expect("name location");
        assert_eq!(location.start.line, 1);
        assert_eq!(location.start.column, 6);
        assert_eq!(
            &source[location.start.offset..location.end.offset],
            "日本語"
        );
        let id = map.location("$.nodes[0].id").expect("id location");
        assert_eq!(id.start.line, 3);
        assert_eq!(&source[id.start.offset..id.end.offset], "first");
        assert_eq!(map.source_hash.len(), 64);
    }
}
