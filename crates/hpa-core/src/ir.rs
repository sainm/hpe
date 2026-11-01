use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};

use crate::error::{CoreError, CoreResult};

pub const SCHEMA_VERSION: &str = "hpa.dev/v1";
pub const IR_SCHEMA_ID: &str = "https://hpa.dev/schema/ir/v0.1/workflow.schema.json";
pub const SUPPORTED_NODE_TYPES: &[&str] = &[
    "start",
    "end",
    "condition",
    "switch",
    "loop",
    "parallel",
    "join",
    "http.request",
    "script",
    "transform",
    "setVariable",
    "human.approval",
    "subflow",
];

pub fn empty_object() -> Value {
    Value::Object(Map::new())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind {
    Start,
    End,
    Condition,
    Switch,
    Loop,
    Parallel,
    Join,
    HttpRequest,
    Script,
    Transform,
    SetVariable,
    HumanApproval,
    Subflow,
}

impl NodeKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "start" => Some(Self::Start),
            "end" => Some(Self::End),
            "condition" => Some(Self::Condition),
            "switch" => Some(Self::Switch),
            "loop" => Some(Self::Loop),
            "parallel" => Some(Self::Parallel),
            "join" => Some(Self::Join),
            "http.request" => Some(Self::HttpRequest),
            "script" => Some(Self::Script),
            "transform" => Some(Self::Transform),
            "setVariable" => Some(Self::SetVariable),
            "human.approval" => Some(Self::HumanApproval),
            "subflow" => Some(Self::Subflow),
            _ => None,
        }
    }

    pub fn is_container(self) -> bool {
        matches!(self, Self::Loop | Self::Parallel)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Workflow {
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub schema_version: String,
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub workflow_version: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<Io>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<Io>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variables: Vec<Variable>,
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub edges: Vec<Edge>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<Trigger>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policies: Option<Policies>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_handling: Option<ErrorHandling>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<Retry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<Timeout>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subflows: Vec<Subflow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

impl Workflow {
    pub fn from_json_str(text: &str) -> CoreResult<Self> {
        Ok(serde_json::from_str(text)?)
    }

    pub fn from_yaml_str(text: &str) -> CoreResult<Self> {
        let value = crate::yaml::yaml_to_json(text)?;
        Self::from_value(&value)
    }

    pub fn from_value(value: &Value) -> CoreResult<Self> {
        let workflow: Self = serde_json::from_value(value.clone())?;
        workflow.ensure_supported_schema()?;
        Ok(workflow)
    }

    pub fn ensure_supported_schema(&self) -> CoreResult<()> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(CoreError::SchemaVersion {
                found: self.schema_version.clone(),
                expected: SCHEMA_VERSION,
            });
        }
        Ok(())
    }

    pub fn to_value(&self) -> CoreResult<Value> {
        Ok(serde_json::to_value(self)?)
    }

    pub fn to_json_string(&self, pretty: bool) -> CoreResult<String> {
        if pretty {
            Ok(serde_json::to_string_pretty(self)?)
        } else {
            Ok(serde_json::to_string(self)?)
        }
    }

    pub fn to_yaml_string(&self) -> CoreResult<String> {
        let value = self.to_value()?;
        crate::yaml::json_to_yaml(&value)
    }

    pub fn canonical_value(&self) -> CoreResult<Value> {
        let value = self.to_value()?;
        Ok(canonicalize(&value))
    }

    pub fn root_graph(&self) -> Graph {
        Graph {
            nodes: self.nodes.clone(),
            edges: self.edges.clone(),
        }
    }

    pub fn find_subflow(&self, id: &str) -> Option<&Subflow> {
        self.subflows.iter().find(|subflow| subflow.id == id)
    }

    pub fn find_node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|node| node.id == id)
    }

    pub fn contains_node_recursive(&self, id: &str) -> bool {
        fn walk(graph: &Graph, id: &str) -> bool {
            for node in &graph.nodes {
                if node.id == id {
                    return true;
                }
                if let Some(body) = &node.body
                    && walk(body, id)
                {
                    return true;
                }
                if let Some(branches) = &node.branches {
                    for branch in branches {
                        if walk(&branch.graph, id) {
                            return true;
                        }
                    }
                }
            }
            false
        }
        walk(&self.root_graph(), id) || self.subflows.iter().any(|subflow| walk(&subflow.graph, id))
    }

    pub fn input_names(&self) -> Vec<&str> {
        self.inputs
            .iter()
            .map(|input| input.name.as_str())
            .collect()
    }

    pub fn variable_names(&self) -> Vec<&str> {
        self.variables
            .iter()
            .map(|variable| variable.name.as_str())
            .collect()
    }
}

pub fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(canonicalize).collect()),
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = Map::new();
            for key in keys {
                out.insert(key.clone(), canonicalize(&map[key]));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Io {
    pub name: String,
    #[serde(rename = "type")]
    pub io_type: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Variable {
    pub name: String,
    #[serde(rename = "type")]
    pub var_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Graph {
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub edges: Vec<Edge>,
}

impl Graph {
    pub fn entries(&self) -> Vec<&Node> {
        let targets: std::collections::BTreeSet<&str> =
            self.edges.iter().map(|edge| edge.target.as_str()).collect();
        self.nodes
            .iter()
            .filter(|node| !targets.contains(node.id.as_str()))
            .collect()
    }

    pub fn exits(&self) -> Vec<&Node> {
        let sources: std::collections::BTreeSet<&str> =
            self.edges.iter().map(|edge| edge.source.as_str()).collect();
        self.nodes
            .iter()
            .filter(|node| !sources.contains(node.id.as_str()))
            .collect()
    }

    pub fn incoming(&self, node_id: &str) -> Vec<&Edge> {
        self.edges
            .iter()
            .filter(|edge| edge.target == node_id)
            .collect()
    }

    pub fn outgoing(&self, node_id: &str) -> Vec<&Edge> {
        self.edges
            .iter()
            .filter(|edge| edge.source == node_id)
            .collect()
    }

    pub fn node(&self, node_id: &str) -> Option<&Node> {
        self.nodes.iter().find(|node| node.id == node_id)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Node {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: String,
    pub type_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default = "empty_object")]
    pub config: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_mapping: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<Retry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_policy: Option<ErrorPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Graph>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branches: Option<Vec<Branch>>,
}

impl Node {
    pub fn kind(&self) -> Option<NodeKind> {
        NodeKind::parse(&self.node_type)
    }

    pub fn config_as<T: DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_value(self.config.clone())
    }

    pub fn is_container(&self) -> bool {
        self.kind().map(NodeKind::is_container).unwrap_or(false)
    }

    pub fn output_ports(&self) -> Option<Vec<String>> {
        let kind = self.kind()?;
        let ports = match kind {
            NodeKind::Start => vec!["out".to_string()],
            NodeKind::End => Vec::new(),
            NodeKind::Condition => vec!["true".to_string(), "false".to_string()],
            NodeKind::Switch => {
                let config: SwitchConfig = self.config_as().ok()?;
                let mut ports: Vec<String> =
                    config.cases.into_iter().map(|case| case.port).collect();
                if let Some(default_port) = config.default_port {
                    ports.push(default_port);
                }
                ports
            }
            NodeKind::HumanApproval => vec![
                "approved".to_string(),
                "rejected".to_string(),
                "timeout".to_string(),
            ],
            NodeKind::Loop => vec!["out".to_string(), "empty".to_string(), "error".to_string()],
            NodeKind::Parallel => vec![
                "out".to_string(),
                "partial".to_string(),
                "error".to_string(),
            ],
            NodeKind::Join => vec!["out".to_string(), "error".to_string()],
            NodeKind::HttpRequest
            | NodeKind::Script
            | NodeKind::Transform
            | NodeKind::SetVariable
            | NodeKind::Subflow => vec!["out".to_string(), "error".to_string()],
        };
        Some(ports)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Branch {
    pub id: String,
    pub graph: Graph,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Edge {
    pub id: String,
    pub source: String,
    #[serde(default = "default_source_port")]
    pub source_port: String,
    pub target: String,
    #[serde(default = "default_target_port")]
    pub target_port: String,
}

fn default_source_port() -> String {
    "out".to_string()
}

fn default_target_port() -> String {
    "in".to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Retry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backoff: Option<Backoff>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retry_on: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub non_retryable: Vec<String>,
}

impl Retry {
    pub fn attempts(&self) -> u32 {
        self.max_attempts.unwrap_or(1).max(1)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Backoff {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<BackoffKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jitter: Option<bool>,
}

impl Backoff {
    pub fn kind(&self) -> BackoffKind {
        self.kind.unwrap_or_default()
    }

    pub fn base_ms(&self) -> u64 {
        self.base_ms.unwrap_or(500).max(1)
    }

    pub fn max_ms(&self) -> u64 {
        self.max_ms.unwrap_or(10_000).max(1)
    }

    pub fn jitter(&self) -> bool {
        self.jitter.unwrap_or(true)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum BackoffKind {
    Constant,
    Linear,
    #[default]
    Exponential,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ErrorPolicy {
    #[serde(default)]
    pub mode: ErrorMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_port: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compensation_node_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ErrorMode {
    #[default]
    Fail,
    Skip,
    Continue,
    Fallback,
    Compensate,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ErrorHandling {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<ErrorHandlingMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph: Option<Graph>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ErrorHandlingMode {
    Graph,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Timeout {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Policies {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub durability: Option<Durability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<ConcurrencyPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<BudgetPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk_level: Option<RiskLevel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_retention_days: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Durability {
    Standard,
    Durable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConcurrencyPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BudgetPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Trigger {
    pub id: String,
    #[serde(rename = "type")]
    pub trigger_type: TriggerType,
    #[serde(default = "empty_object")]
    pub config: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriggerType {
    Schedule,
    Webhook,
    Event,
    Mcp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Subflow {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<Io>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<Io>,
    pub graph: Graph,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransformConfig {
    #[serde(default = "empty_object")]
    pub mapping: Value,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConditionConfig {
    #[serde(default)]
    pub condition: Value,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchConfig {
    #[serde(default)]
    pub cases: Vec<SwitchCase>,
    #[serde(default)]
    pub default_port: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchCase {
    #[serde(default)]
    pub port: String,
    #[serde(default)]
    pub condition: Value,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopConfig {
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub items: Option<Value>,
    #[serde(default)]
    pub condition: Option<Value>,
    #[serde(default)]
    pub item_variable: Option<String>,
    #[serde(default)]
    pub index_variable: Option<String>,
    #[serde(default)]
    pub max_iterations: Option<u64>,
    #[serde(default)]
    pub concurrency: Option<u32>,
    #[serde(default)]
    pub fail_fast: Option<bool>,
    #[serde(default)]
    pub continue_on_error: Option<bool>,
}

impl LoopConfig {
    pub fn item_variable(&self) -> &str {
        self.item_variable.as_deref().unwrap_or("item")
    }

    pub fn index_variable(&self) -> &str {
        self.index_variable.as_deref().unwrap_or("index")
    }

    pub fn max_iterations(&self) -> u64 {
        self.max_iterations.unwrap_or(1_000).max(1)
    }

    pub fn concurrency(&self) -> u32 {
        self.concurrency.unwrap_or(1).max(1)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParallelConfig {
    #[serde(default)]
    pub join: Option<String>,
    #[serde(default)]
    pub n_of: Option<u32>,
    #[serde(default)]
    pub fail_fast: Option<bool>,
    #[serde(default)]
    pub max_concurrency: Option<u32>,
}

impl ParallelConfig {
    pub fn join(&self) -> &str {
        self.join.as_deref().unwrap_or("all")
    }

    pub fn fail_fast(&self) -> bool {
        self.fail_fast.unwrap_or(false)
    }

    pub fn max_concurrency(&self) -> u32 {
        self.max_concurrency.unwrap_or(4).max(1)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalConfig {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub form_schema: Option<Value>,
    #[serde(default)]
    pub assignees: Vec<Value>,
    #[serde(default)]
    pub sla_ms: Option<u64>,
    #[serde(default)]
    pub on_timeout: Option<String>,
    #[serde(default)]
    pub escalation: Option<Value>,
}

impl ApprovalConfig {
    pub fn on_timeout(&self) -> &str {
        self.on_timeout.as_deref().unwrap_or("reject")
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubflowConfig {
    #[serde(default)]
    pub subflow_id: Option<String>,
    #[serde(default)]
    pub input: Option<Value>,
    #[serde(default)]
    pub outputs: Option<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpConfig {
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub url: Option<Value>,
    #[serde(default)]
    pub headers: Option<Value>,
    #[serde(default)]
    pub body: Option<Value>,
    #[serde(default)]
    pub query: Option<Value>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub expected_status: Vec<u16>,
    #[serde(default)]
    pub max_response_bytes: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptConfig {
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetVariableConfig {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub value: Option<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinConfig {
    #[serde(default)]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EndConfig {
    #[serde(default)]
    pub output: Option<Value>,
}
