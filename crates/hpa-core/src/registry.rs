use std::collections::BTreeSet;
use std::sync::OnceLock;

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::ir::Node;

pub const REGISTRY_VERSION: &str = "2026.11.1";
pub const CURRENT_NODE_VERSION: &str = "1.0.0";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeVersionDefinition {
    pub version: String,
    pub deprecated: bool,
    pub config_schema: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeDefinition {
    #[serde(rename = "type")]
    pub node_type: String,
    pub versions: Vec<NodeVersionDefinition>,
    pub default_version: String,
    pub output_ports: Vec<String>,
    pub permission_kinds: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedVersion {
    pub version: String,
    pub deprecated: bool,
}

#[derive(Debug, Clone)]
pub struct NetworkRule {
    pub host: String,
    pub port: Option<u16>,
    pub wildcard: bool,
}

#[derive(Debug, Clone, Default)]
pub struct NetworkPolicy {
    markers: Vec<NetworkRule>,
    implicit_local: bool,
}

pub fn definitions() -> &'static [NodeDefinition] {
    static DEFINITIONS: OnceLock<Vec<NodeDefinition>> = OnceLock::new();
    DEFINITIONS.get_or_init(build_definitions)
}

pub fn definition(node_type: &str) -> Option<&'static NodeDefinition> {
    definitions()
        .iter()
        .find(|definition| definition.node_type == node_type)
}

pub fn resolve_version(node_type: &str, requested: &str) -> Result<ResolvedVersion, String> {
    let definition =
        definition(node_type).ok_or_else(|| format!("unknown node type `{node_type}`"))?;
    let normalized = normalize_version(requested);
    definition
        .versions
        .iter()
        .find(|version| version.version == normalized)
        .map(|version| ResolvedVersion {
            version: version.version.clone(),
            deprecated: version.deprecated,
        })
        .ok_or_else(|| {
            let supported: Vec<&str> = definition
                .versions
                .iter()
                .map(|version| version.version.as_str())
                .collect();
            format!(
                "node `{node_type}` does not support typeVersion `{requested}`; supported: {}",
                supported.join(", ")
            )
        })
}

pub fn normalize_version(requested: &str) -> String {
    match requested.split('.').count() {
        1 => format!("{requested}.0.0"),
        2 => format!("{requested}.0"),
        _ => requested.to_string(),
    }
}

pub fn migrate_node(node: &Node) -> Result<Node, String> {
    if node.node_type != "http.request" || normalize_version(&node.type_version) != "0.9.0" {
        return Ok(node.clone());
    }
    let mut migrated = node.clone();
    let config = migrated
        .config
        .as_object_mut()
        .ok_or_else(|| "http.request config must be an object".to_string())?;
    if let Some(uri) = config.remove("uri")
        && !config.contains_key("url")
    {
        config.insert("url".to_string(), uri);
    }
    if let Some(verb) = config.remove("verb")
        && !config.contains_key("method")
    {
        config.insert("method".to_string(), verb);
    }
    config
        .entry("method".to_string())
        .or_insert_with(|| json!("GET"));
    migrated.type_version = CURRENT_NODE_VERSION.to_string();
    Ok(migrated)
}

impl NetworkPolicy {
    pub fn from_permissions(permissions: Option<&Value>) -> Result<Self, String> {
        let Some(permissions) = permissions else {
            return Ok(Self {
                markers: Vec::new(),
                implicit_local: true,
            });
        };
        let object = permissions
            .as_object()
            .ok_or_else(|| "permissions must be an object".to_string())?;
        let Some(network) = object.get("network") else {
            return Ok(Self::default());
        };
        let entries = network
            .as_array()
            .ok_or_else(|| "permissions.network must be an array".to_string())?;
        let mut markers = Vec::with_capacity(entries.len());
        for entry in entries {
            let raw = entry
                .as_str()
                .ok_or_else(|| "network entries must be strings".to_string())?;
            markers.push(parse_network_rule(raw)?);
        }
        Ok(Self {
            markers,
            implicit_local: false,
        })
    }

    pub fn allows(&self, host: &str, port: u16) -> bool {
        if self.markers.iter().any(|marker| marker.matches(host, port)) {
            return true;
        }
        self.implicit_local && is_loopback_host(host)
    }

    pub fn explicitly_allows(&self, host: &str, port: u16) -> bool {
        self.markers.iter().any(|marker| marker.matches(host, port))
    }

    pub fn allows_private_resolution(&self, host: &str, port: u16) -> bool {
        (self.implicit_local && is_loopback_host(host))
            || self.markers.iter().any(|marker| {
                marker.matches(host, port)
                    && (marker.host == "localhost"
                        || marker.host.parse::<std::net::IpAddr>().is_ok())
            })
    }

    pub fn is_empty(&self) -> bool {
        self.markers.is_empty()
    }
}

impl NetworkRule {
    pub fn matches(&self, host: &str, port: u16) -> bool {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        let host_matches = if self.host == "*" {
            true
        } else if self.wildcard {
            host.ends_with(&self.host) && host.len() > self.host.len()
        } else {
            host == self.host
        };
        host_matches && self.port.is_none_or(|value| value == port)
    }
}

pub fn parse_network_rule(raw: &str) -> Result<NetworkRule, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("network rule must not be empty".to_string());
    }
    if raw == "*" {
        return Ok(NetworkRule {
            host: "*".to_string(),
            port: None,
            wildcard: false,
        });
    }
    let (host, port) = if let Some(rest) = raw.strip_prefix('[') {
        let end = rest
            .find(']')
            .ok_or_else(|| format!("invalid network rule `{raw}`"))?;
        let host = &rest[..end];
        let port = rest[end + 1..]
            .strip_prefix(':')
            .map(parse_port)
            .transpose()?;
        (host.to_string(), port)
    } else if let Some((host, port)) = raw.rsplit_once(':')
        && !host.contains(':')
        && port.bytes().all(|byte| byte.is_ascii_digit())
    {
        (host.to_string(), Some(parse_port(port)?))
    } else {
        (raw.to_string(), None)
    };
    if host.is_empty() {
        return Err(format!("invalid network rule `{raw}`"));
    }
    let wildcard = host.starts_with("*.");
    let host = host
        .trim_start_matches("*.")
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if host.is_empty()
        || host.contains(char::is_whitespace)
        || (!raw.starts_with('[') && host.contains(':'))
    {
        return Err(format!("invalid network rule `{raw}`"));
    }
    Ok(NetworkRule {
        host,
        port,
        wildcard,
    })
}

fn parse_port(port: &str) -> Result<u16, String> {
    port.parse::<u16>()
        .map_err(|_| format!("invalid port `{port}`"))
}

pub fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_matches(['[', ']']);
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .map(|address| address.is_loopback())
            .unwrap_or(false)
}

pub fn is_private_address(address: std::net::IpAddr) -> bool {
    match address {
        std::net::IpAddr::V4(address) => {
            address.is_private()
                || address.is_loopback()
                || address.is_link_local()
                || address.is_unspecified()
                || address.is_broadcast()
        }
        std::net::IpAddr::V6(address) => {
            address.is_loopback()
                || address.is_unspecified()
                || address.is_multicast()
                || (address.segments()[0] & 0xfe00) == 0xfc00
                || (address.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

fn build_definitions() -> Vec<NodeDefinition> {
    let simple = json!({
        "type": "object",
        "additionalProperties": true
    });
    let version = |schema: Value| NodeVersionDefinition {
        version: CURRENT_NODE_VERSION.to_string(),
        deprecated: false,
        config_schema: schema,
    };
    vec![
        definition_of("start", version(simple.clone()), "out", &[]),
        definition_of("end", version(simple.clone()), "", &[]),
        definition_of(
            "transform",
            version(json!({
                "type": "object",
                "required": ["mapping"],
                "properties": { "mapping": { "type": "object" } },
                "additionalProperties": true
            })),
            "out",
            &[],
        ),
        definition_of(
            "condition",
            version(json!({
                "type": "object",
                "required": ["condition"],
                "properties": { "condition": { "type": "object" } },
                "additionalProperties": true
            })),
            "true,false",
            &[],
        ),
        definition_of(
            "switch",
            version(json!({
                "type": "object",
                "required": ["cases", "defaultPort"],
                "properties": {
                    "cases": { "type": "array", "minItems": 1 },
                    "defaultPort": { "type": "string" }
                },
                "additionalProperties": true
            })),
            "case_*,default",
            &[],
        ),
        definition_of(
            "loop",
            version(json!({
                "type": "object",
                "required": ["mode"],
                "properties": {
                    "mode": { "enum": ["forEach", "while"] },
                    "items": { "type": "object" },
                    "condition": { "type": "object" },
                    "maxIterations": { "type": "integer", "minimum": 1 }
                },
                "additionalProperties": true
            })),
            "out,empty,error",
            &[],
        ),
        definition_of(
            "parallel",
            version(json!({
                "type": "object",
                "properties": {
                    "join": { "enum": ["all", "any", "n_of"] },
                    "maxConcurrency": { "type": "integer", "minimum": 1 }
                },
                "additionalProperties": true
            })),
            "out,partial,error",
            &[],
        ),
        definition_of("join", version(simple.clone()), "out,error", &[]),
        definition_of(
            "http.request",
            version(json!({
                "type": "object",
                "required": ["url"],
                "properties": {
                    "method": { "enum": ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"] },
                    "url": { "type": ["string", "object"] },
                    "headers": { "type": "object" },
                    "body": {},
                    "query": { "type": "object" },
                    "expectedStatus": { "type": "array", "items": { "type": "integer" } },
                    "maxResponseBytes": { "type": "integer", "minimum": 1 }
                },
                "additionalProperties": true
            })),
            "out,error",
            &["network"],
        ),
        definition_of(
            "script",
            version(json!({
                "type": "object",
                "required": ["source"],
                "properties": {
                    "language": { "const": "jq" },
                    "source": { "type": "string", "minLength": 1 }
                },
                "additionalProperties": true
            })),
            "out,error",
            &[],
        ),
        definition_of(
            "setVariable",
            version(json!({
                "type": "object",
                "required": ["name"],
                "properties": { "name": { "type": "string", "minLength": 1 } },
                "additionalProperties": true
            })),
            "out,error",
            &[],
        ),
        definition_of(
            "human.approval",
            version(json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "formSchema": { "type": "object" },
                    "slaMs": { "type": "integer", "minimum": 1 },
                    "onTimeout": { "enum": ["approve", "reject"] }
                },
                "additionalProperties": true
            })),
            "approved,rejected,timeout",
            &[],
        ),
        definition_of(
            "subflow",
            version(json!({
                "type": "object",
                "required": ["subflowId"],
                "properties": { "subflowId": { "type": "string", "minLength": 1 } },
                "additionalProperties": true
            })),
            "out,error",
            &[],
        ),
    ]
}

fn definition_of(
    node_type: &str,
    version: NodeVersionDefinition,
    ports: &str,
    permission_kinds: &[&str],
) -> NodeDefinition {
    let output_ports = ports
        .split(',')
        .filter(|port| !port.is_empty())
        .map(str::to_string)
        .collect();
    NodeDefinition {
        node_type: node_type.to_string(),
        versions: vec![version],
        default_version: CURRENT_NODE_VERSION.to_string(),
        output_ports,
        permission_kinds: permission_kinds
            .iter()
            .map(|kind| (*kind).to_string())
            .collect(),
    }
}

pub fn permission_kinds(node_type: &str) -> BTreeSet<&'static str> {
    definition(node_type)
        .map(|definition| {
            definition
                .permission_kinds
                .iter()
                .map(String::as_str)
                .collect()
        })
        .unwrap_or_default()
}

pub fn invalid_permission_keys(permissions: &Value) -> Vec<String> {
    let Some(object) = permissions.as_object() else {
        return vec!["permissions".to_string()];
    };
    object
        .keys()
        .filter(|key| {
            !matches!(
                key.as_str(),
                "network" | "filesystem" | "commands" | "connectors" | "models" | "mcpServers"
            )
        })
        .cloned()
        .collect()
}

pub fn config_map(config: &Value) -> Map<String, Value> {
    match config {
        Value::Object(map) => map.clone(),
        _ => Map::new(),
    }
}
