use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Issue, Severity};
use crate::expression::{self, ExpressionScope, TemplatePart};
use crate::ir::{
    ApprovalConfig, Branch, ConditionConfig, Edge, ErrorHandlingMode, ErrorMode, Graph, HttpConfig,
    JoinConfig, LoopConfig, Node, NodeKind, ParallelConfig, ScriptConfig, SetVariableConfig,
    Subflow, SubflowConfig, SwitchConfig, TransformConfig, Workflow,
};
use crate::registry;

pub const ROOT_GRAPH: &str = "root";
pub const ERROR_GRAPH: &str = "error";
pub const MAX_NODES: usize = 500;
pub const MAX_EDGES: usize = 2_000;
pub const MAX_NESTING_DEPTH: usize = 10;
pub const MAX_SUBFLOW_DEPTH: usize = 20;

pub fn loop_graph_id(node_id: &str) -> String {
    format!("loop:{node_id}")
}

pub fn parallel_graph_id(node_id: &str, branch_id: &str) -> String {
    format!("parallel:{node_id}:{branch_id}")
}

pub fn subflow_graph_id(subflow_id: &str) -> String {
    format!("subflow:{subflow_id}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphKind {
    Root,
    LoopBody,
    ParallelBranch,
    Subflow,
    ErrorGraph,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanLimits {
    pub max_nodes: usize,
    pub max_edges: usize,
    pub max_nesting_depth: usize,
    pub max_subflow_depth: usize,
}

impl Default for PlanLimits {
    fn default() -> Self {
        Self {
            max_nodes: MAX_NODES,
            max_edges: MAX_EDGES,
            max_nesting_depth: MAX_NESTING_DEPTH,
            max_subflow_depth: MAX_SUBFLOW_DEPTH,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompiledGraph {
    pub id: String,
    pub kind: GraphKind,
    pub owner: Option<String>,
    pub nodes: BTreeMap<String, Node>,
    pub order: Vec<String>,
    pub edges: Vec<Edge>,
    pub entries: Vec<String>,
    pub exits: Vec<String>,
    pub outgoing: BTreeMap<String, Vec<Edge>>,
    pub incoming: BTreeMap<String, Vec<Edge>>,
    pub topological: Vec<String>,
}

impl CompiledGraph {
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.get(id)
    }

    pub fn targets(&self, node_id: &str, port: &str) -> Vec<&str> {
        self.outgoing
            .get(node_id)
            .into_iter()
            .flatten()
            .filter(|edge| edge.source_port == port)
            .map(|edge| edge.target.as_str())
            .collect()
    }

    pub fn all_targets(&self, node_id: &str) -> Vec<&str> {
        self.outgoing
            .get(node_id)
            .into_iter()
            .flatten()
            .map(|edge| edge.target.as_str())
            .collect()
    }

    pub fn is_exit(&self, node_id: &str) -> bool {
        self.exits.iter().any(|id| id == node_id)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionPlan {
    pub workflow_id: String,
    pub workflow_version: String,
    pub registry_version: String,
    pub root: String,
    pub graphs: BTreeMap<String, CompiledGraph>,
    pub subflows: BTreeMap<String, String>,
    pub limits: PlanLimits,
}

impl ExecutionPlan {
    pub fn graph(&self, id: &str) -> Option<&CompiledGraph> {
        self.graphs.get(id)
    }

    pub fn root_graph(&self) -> Option<&CompiledGraph> {
        self.graphs.get(&self.root)
    }

    pub fn subflow_graph(&self, subflow_id: &str) -> Option<&CompiledGraph> {
        self.subflows
            .get(subflow_id)
            .and_then(|id| self.graphs.get(id))
    }
}

#[derive(Debug, Clone)]
pub struct Compilation {
    pub plan: ExecutionPlan,
    pub issues: Vec<Issue>,
}

pub fn compile_workflow(workflow: &Workflow) -> Result<Compilation, Vec<Issue>> {
    Builder::new(workflow).run()
}

struct Builder<'a> {
    workflow: &'a Workflow,
    issues: Vec<Issue>,
    node_ids: BTreeSet<String>,
    edge_ids: BTreeSet<String>,
    graphs: BTreeMap<String, CompiledGraph>,
    subflow_graph_ids: BTreeMap<String, String>,
}

impl<'a> Builder<'a> {
    fn new(workflow: &'a Workflow) -> Self {
        Self {
            workflow,
            issues: Vec::new(),
            node_ids: BTreeSet::new(),
            edge_ids: BTreeSet::new(),
            graphs: BTreeMap::new(),
            subflow_graph_ids: BTreeMap::new(),
        }
    }

    fn run(mut self) -> Result<Compilation, Vec<Issue>> {
        self.check_triggers();
        self.check_subflow_ids();
        let root_graph = self.workflow.root_graph();
        self.collect_graph_ids(&root_graph, "$");
        let subflows = self.workflow.subflows.clone();
        for (index, subflow) in subflows.iter().enumerate() {
            self.collect_graph_ids(&subflow.graph, &format!("$.subflows[{index}].graph"));
        }
        if let Some(error_handling) = &self.workflow.error_handling
            && let Some(graph) = &error_handling.graph
        {
            self.collect_graph_ids(graph, "$.errorHandling.graph");
        }

        let root_scope = self.root_scope();
        self.build_graph(
            &root_graph,
            ROOT_GRAPH,
            GraphKind::Root,
            None,
            &root_scope,
            "$",
            0,
        );

        for (index, subflow) in subflows.iter().enumerate() {
            let scope = self.subflow_scope(subflow);
            let graph_id = subflow_graph_id(&subflow.id);
            self.subflow_graph_ids
                .insert(subflow.id.clone(), graph_id.clone());
            self.build_graph(
                &subflow.graph,
                &graph_id,
                GraphKind::Subflow,
                Some(subflow.id.clone()),
                &scope,
                &format!("$.subflows[{index}].graph"),
                0,
            );
        }

        if let Some(error_handling) = &self.workflow.error_handling {
            match (&error_handling.mode, &error_handling.graph) {
                (Some(ErrorHandlingMode::Graph), Some(graph)) => {
                    let graph = graph.clone();
                    self.build_graph(
                        &graph,
                        ERROR_GRAPH,
                        GraphKind::ErrorGraph,
                        None,
                        &root_scope,
                        "$.errorHandling.graph",
                        0,
                    );
                }
                (Some(ErrorHandlingMode::Graph), None) => {
                    self.error(
                        "ERROR_GRAPH_MISSING",
                        "$.errorHandling",
                        "mode `graph` requires an errorHandling.graph",
                    );
                }
                (_, Some(_)) => {
                    self.warning(
                        "ERROR_GRAPH_UNUSED",
                        "$.errorHandling",
                        "errorHandling.graph is ignored unless mode is `graph`",
                    );
                }
                _ => {}
            }
        }

        self.check_subflow_recursion(&subflows);
        self.check_limits();
        self.validate_workflow_policy();
        let empty_subflow =
            self.workflow
                .subflows
                .iter()
                .enumerate()
                .find_map(|(index, subflow)| {
                    subflow
                        .graph
                        .nodes
                        .is_empty()
                        .then(|| (index, subflow.id.clone()))
                });
        if let Some((index, id)) = empty_subflow {
            self.error(
                "SUBFLOW_EMPTY",
                format!("$.subflows[{index}]"),
                format!("subflow `{id}` has an empty graph"),
            );
        }

        let has_errors = self
            .issues
            .iter()
            .any(|issue| issue.severity == Severity::Error);
        let issues = self.issues;
        if has_errors {
            return Err(issues);
        }

        let plan = ExecutionPlan {
            workflow_id: self.workflow.id.clone(),
            workflow_version: self.workflow.workflow_version.clone(),
            registry_version: registry::REGISTRY_VERSION.to_string(),
            root: ROOT_GRAPH.to_string(),
            graphs: self.graphs,
            subflows: self.subflow_graph_ids,
            limits: PlanLimits::default(),
        };
        Ok(Compilation { plan, issues })
    }

    fn root_scope(&self) -> ExpressionScope {
        ExpressionScope::new(
            self.workflow.input_names().into_iter().map(str::to_string),
            self.workflow
                .variable_names()
                .into_iter()
                .map(str::to_string),
            self.node_ids.iter().cloned(),
        )
    }

    fn subflow_scope(&self, subflow: &Subflow) -> ExpressionScope {
        ExpressionScope::new(
            subflow.inputs.iter().map(|input| input.name.clone()),
            self.workflow
                .variable_names()
                .into_iter()
                .map(str::to_string),
            self.node_ids.iter().cloned(),
        )
    }

    fn check_subflow_ids(&mut self) {
        let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
        for (index, subflow) in self.workflow.subflows.iter().enumerate() {
            if let Some(first) = seen.insert(subflow.id.as_str(), index) {
                self.error(
                    "DUP_SUBFLOW_ID",
                    format!("$.subflows[{index}]"),
                    format!(
                        "duplicate subflow id `{}` (first declared at index {first})",
                        subflow.id
                    ),
                );
            }
        }
    }

    fn collect_graph_ids(&mut self, graph: &Graph, base: &str) {
        for (index, node) in graph.nodes.iter().enumerate() {
            let path = format!("{base}.nodes[{index}]");
            if !self.node_ids.insert(node.id.clone()) {
                self.error(
                    "DUP_NODE_ID",
                    &path,
                    format!("duplicate node id `{}`", node.id),
                );
            }
            if let Some(body) = &node.body {
                self.collect_graph_ids(body, &format!("{path}.body"));
            }
            if let Some(branches) = &node.branches {
                for (branch_index, branch) in branches.iter().enumerate() {
                    self.collect_graph_ids(
                        &branch.graph,
                        &format!("{path}.branches[{branch_index}].graph"),
                    );
                }
            }
        }
        for (index, edge) in graph.edges.iter().enumerate() {
            if !self.edge_ids.insert(edge.id.clone()) {
                self.error(
                    "DUP_EDGE_ID",
                    format!("{base}.edges[{index}]"),
                    format!("duplicate edge id `{}`", edge.id),
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_graph(
        &mut self,
        graph: &Graph,
        graph_id: &str,
        kind: GraphKind,
        owner: Option<String>,
        scope: &ExpressionScope,
        base_path: &str,
        depth: usize,
    ) {
        if depth > MAX_NESTING_DEPTH {
            self.error(
                "NESTING_DEPTH",
                base_path,
                format!("container nesting exceeds {MAX_NESTING_DEPTH} levels"),
            );
            return;
        }
        if graph.nodes.is_empty() {
            self.error("GRAPH_EMPTY", base_path, "graph has no nodes");
            return;
        }

        let mut nodes: BTreeMap<String, Node> = BTreeMap::new();
        let mut order = Vec::with_capacity(graph.nodes.len());
        for node in &graph.nodes {
            if nodes.insert(node.id.clone(), node.clone()).is_some() {
                self.error(
                    "DUP_NODE_ID",
                    base_path,
                    format!("duplicate node id `{}` in graph", node.id),
                );
            }
            order.push(node.id.clone());
        }

        if kind == GraphKind::Root {
            let starts: Vec<&Node> = graph
                .nodes
                .iter()
                .filter(|node| node.kind() == Some(NodeKind::Start))
                .collect();
            let ends: Vec<&Node> = graph
                .nodes
                .iter()
                .filter(|node| node.kind() == Some(NodeKind::End))
                .collect();
            if starts.len() != 1 {
                self.error(
                    "ROOT_START",
                    base_path,
                    format!(
                        "workflow must have exactly one start node, found {}",
                        starts.len()
                    ),
                );
            }
            if ends.len() != 1 {
                self.error(
                    "ROOT_END",
                    base_path,
                    format!(
                        "workflow must have exactly one end node, found {}",
                        ends.len()
                    ),
                );
            }
        }

        let mut outgoing: BTreeMap<String, Vec<Edge>> = BTreeMap::new();
        let mut incoming: BTreeMap<String, Vec<Edge>> = BTreeMap::new();
        for (index, edge) in graph.edges.iter().enumerate() {
            let path = format!("{base_path}.edges[{index}]");
            let Some(source) = nodes.get(&edge.source) else {
                self.error(
                    "EDGE_DANGLING",
                    &path,
                    format!("edge source `{}` does not exist in this graph", edge.source),
                );
                continue;
            };
            let Some(target) = nodes.get(&edge.target) else {
                self.error(
                    "EDGE_DANGLING",
                    &path,
                    format!("edge target `{}` does not exist in this graph", edge.target),
                );
                continue;
            };
            if edge.target_port != "in" {
                self.error(
                    "EDGE_PORT",
                    &path,
                    format!(
                        "target port `{}` is invalid; only `in` is supported",
                        edge.target_port
                    ),
                );
            }
            if target.kind() == Some(NodeKind::Start) {
                self.error(
                    "EDGE_TARGET_START",
                    &path,
                    "start nodes cannot have incoming edges",
                );
            }
            if source.kind() == Some(NodeKind::End) {
                self.error(
                    "EDGE_SOURCE_END",
                    &path,
                    "end nodes cannot have outgoing edges",
                );
            }
            if let Some(ports) = source.output_ports()
                && !ports.contains(&edge.source_port)
            {
                self.error(
                    "EDGE_PORT",
                    &path,
                    format!(
                        "node `{}` has no output port `{}`; available: {}",
                        source.id,
                        edge.source_port,
                        ports.join(", ")
                    ),
                );
            }
            outgoing
                .entry(edge.source.clone())
                .or_default()
                .push(edge.clone());
            incoming
                .entry(edge.target.clone())
                .or_default()
                .push(edge.clone());
        }

        let entries: Vec<String> = graph
            .nodes
            .iter()
            .filter(|node| !incoming.contains_key(&node.id))
            .map(|node| node.id.clone())
            .collect();
        let exits: Vec<String> = graph
            .nodes
            .iter()
            .filter(|node| !outgoing.contains_key(&node.id))
            .map(|node| node.id.clone())
            .collect();

        if entries.is_empty() {
            self.error("GRAPH_NO_ENTRY", base_path, "graph has no entry node");
        }

        let reachable = self.reachable(&order, &outgoing, &entries);
        for (index, node) in graph.nodes.iter().enumerate() {
            if !reachable.contains(&node.id) {
                self.error(
                    "GRAPH_UNREACHABLE",
                    format!("{base_path}.nodes[{index}]"),
                    format!("node `{}` is not reachable from any entry", node.id),
                );
            }
        }

        let topological = topological_order(&order, &incoming, &outgoing);
        if topological.len() != order.len() {
            let remaining: Vec<String> = order
                .iter()
                .filter(|id| !topological.contains(id))
                .cloned()
                .collect();
            self.error(
                "GRAPH_CYCLE",
                base_path,
                format!("graph contains a cycle involving: {}", remaining.join(", ")),
            );
        }

        for (index, node) in graph.nodes.iter().enumerate() {
            let node_path = format!("{base_path}.nodes[{index}]");
            self.validate_node(node, &node_path, scope, depth);
            self.validate_node_edges(node, &node_path, &outgoing);
        }

        if kind == GraphKind::Root {
            for (index, node) in graph.nodes.iter().enumerate() {
                let node_path = format!("{base_path}.nodes[{index}]");
                match node.kind() {
                    Some(NodeKind::Start) if !outgoing.contains_key(&node.id) => {
                        self.error(
                            "START_ISOLATED",
                            &node_path,
                            "start node must connect to the workflow",
                        );
                    }
                    Some(NodeKind::End) if !incoming.contains_key(&node.id) => {
                        self.error("END_ISOLATED", &node_path, "end node must be reachable");
                    }
                    _ => {}
                }
            }
        }

        self.graphs.insert(
            graph_id.to_string(),
            CompiledGraph {
                id: graph_id.to_string(),
                kind,
                owner,
                nodes,
                order,
                edges: graph.edges.clone(),
                entries,
                exits,
                outgoing,
                incoming,
                topological,
            },
        );
    }

    fn reachable(
        &self,
        order: &[String],
        outgoing: &BTreeMap<String, Vec<Edge>>,
        entries: &[String],
    ) -> BTreeSet<String> {
        let mut seen = BTreeSet::new();
        let mut queue: VecDeque<&str> = entries.iter().map(String::as_str).collect();
        while let Some(current) = queue.pop_front() {
            if !seen.insert(current.to_string()) {
                continue;
            }
            if let Some(edges) = outgoing.get(current) {
                for edge in edges {
                    if order.iter().any(|id| id == &edge.target) {
                        queue.push_back(edge.target.as_str());
                    }
                }
            }
        }
        seen
    }

    fn validate_node(&mut self, node: &Node, path: &str, scope: &ExpressionScope, depth: usize) {
        let Some(kind) = node.kind() else {
            self.error(
                "NODE_TYPE",
                path,
                format!("unsupported node type `{}`", node.node_type),
            );
            self.walk_expressions(&node.config, path, scope);
            return;
        };
        if !self.validate_registry_definition(node, path) {
            self.walk_expressions(&node.config, path, scope);
            return;
        }
        self.validate_permissions(node, path, kind);
        self.validate_execution_policy(node, path);

        if let Some(mapping) = &node.input_mapping {
            if !mapping.is_object() {
                self.error(
                    "NODE_CONFIG",
                    format!("{path}.inputMapping"),
                    "inputMapping must be an object",
                );
            } else {
                self.walk_expressions(mapping, &format!("{path}.inputMapping"), scope);
            }
        }

        if let Some(schema) = &node.output_schema
            && jsonschema::validator_for(schema).is_err()
        {
            self.error(
                "OUTPUT_SCHEMA",
                format!("{path}.outputSchema"),
                "outputSchema is not a valid JSON Schema",
            );
        }

        match kind {
            NodeKind::Start => {
                if node.body.is_some() || node.branches.is_some() {
                    self.warning(
                        "NODE_GRAPH_IGNORED",
                        path,
                        "start nodes do not use body or branches",
                    );
                }
            }
            NodeKind::End => {
                let config = self.parse_config::<crate::ir::EndConfig>(node, path);
                if let Some(config) = config
                    && let Some(output) = config.output
                {
                    self.walk_expressions(&output, &format!("{path}.config.output"), scope);
                }
            }
            NodeKind::Transform => {
                let Some(config) = self.parse_config::<TransformConfig>(node, path) else {
                    return;
                };
                if !config.mapping.is_object() {
                    self.error(
                        "NODE_CONFIG",
                        format!("{path}.config.mapping"),
                        "transform mapping must be an object",
                    );
                } else {
                    self.walk_expressions(
                        &config.mapping,
                        &format!("{path}.config.mapping"),
                        scope,
                    );
                }
            }
            NodeKind::Condition => {
                let Some(config) = self.parse_config::<ConditionConfig>(node, path) else {
                    return;
                };
                let _ = self.require_expr(
                    &config.condition,
                    &format!("{path}.config.condition"),
                    "condition",
                );
                self.walk_expressions(&node.config, &format!("{path}.config"), scope);
            }
            NodeKind::Switch => {
                let Some(config) = self.parse_config::<SwitchConfig>(node, path) else {
                    return;
                };
                if config.cases.is_empty() {
                    self.error(
                        "SWITCH_CASES",
                        format!("{path}.config.cases"),
                        "switch requires at least one case",
                    );
                }
                let mut ports = BTreeSet::new();
                for (index, case) in config.cases.iter().enumerate() {
                    if case.port.is_empty() {
                        self.error(
                            "SWITCH_PORT",
                            format!("{path}.config.cases[{index}].port"),
                            "case port must not be empty",
                        );
                    } else if !ports.insert(case.port.clone()) {
                        self.error(
                            "SWITCH_PORT",
                            format!("{path}.config.cases[{index}].port"),
                            format!("duplicate switch port `{}`", case.port),
                        );
                    }
                    let _ = self.require_expr(
                        &case.condition,
                        &format!("{path}.config.cases[{index}].condition"),
                        "switch case",
                    );
                }
                match &config.default_port {
                    None => self.error("SWITCH_DEFAULT", path, "switch requires defaultPort"),
                    Some(port) if port.is_empty() => self.error(
                        "SWITCH_PORT",
                        format!("{path}.config.defaultPort"),
                        "defaultPort must not be empty",
                    ),
                    Some(port) if ports.contains(port) => self.error(
                        "SWITCH_PORT",
                        format!("{path}.config.defaultPort"),
                        format!("defaultPort `{port}` duplicates a case port"),
                    ),
                    Some(_) => {}
                }
                self.walk_expressions(&node.config, &format!("{path}.config"), scope);
            }
            NodeKind::Loop => {
                let Some(config) = self.parse_config::<LoopConfig>(node, path) else {
                    return;
                };
                match config.mode.as_deref() {
                    Some("forEach") => {
                        if config.items.is_none() {
                            self.error(
                                "LOOP_ITEMS",
                                format!("{path}.config.items"),
                                "forEach loop requires items",
                            );
                        }
                    }
                    Some("while") => {
                        if let Some(condition) = &config.condition {
                            let _ = self.require_expr(
                                condition,
                                &format!("{path}.config.condition"),
                                "while loop",
                            );
                        } else {
                            self.error(
                                "LOOP_CONDITION",
                                format!("{path}.config.condition"),
                                "while loop requires a condition",
                            );
                        }
                    }
                    Some(other) => {
                        self.error(
                            "LOOP_MODE",
                            format!("{path}.config.mode"),
                            format!("unsupported loop mode `{other}`"),
                        );
                    }
                    None => {
                        self.error(
                            "LOOP_MODE",
                            format!("{path}.config.mode"),
                            "loop requires mode `forEach` or `while`",
                        );
                    }
                }
                if matches!(config.max_iterations, Some(0)) {
                    self.error(
                        "LOOP_LIMIT",
                        format!("{path}.config.maxIterations"),
                        "maxIterations must be at least 1",
                    );
                }
                if matches!(config.concurrency, Some(0)) {
                    self.error(
                        "LOOP_CONCURRENCY",
                        format!("{path}.config.concurrency"),
                        "loop concurrency must be at least 1",
                    );
                }
                if config.item_variable.as_deref() == Some("")
                    || config.index_variable.as_deref() == Some("")
                {
                    self.error(
                        "LOOP_VARIABLE",
                        format!("{path}.config"),
                        "loop variable names must not be empty",
                    );
                }
                self.walk_expressions(&node.config, &format!("{path}.config"), scope);
                match &node.body {
                    Some(body) => {
                        let mut body_scope = scope.clone();
                        body_scope.allow_loop = true;
                        self.build_graph(
                            body,
                            &loop_graph_id(&node.id),
                            GraphKind::LoopBody,
                            Some(node.id.clone()),
                            &body_scope,
                            &format!("{path}.body"),
                            depth + 1,
                        );
                    }
                    None => self.error("LOOP_BODY", path, "loop node requires a body graph"),
                }
            }
            NodeKind::Parallel => {
                let Some(config) = self.parse_config::<ParallelConfig>(node, path) else {
                    return;
                };
                let join = config.join();
                if !matches!(join, "all" | "any" | "n_of") {
                    self.error(
                        "PARALLEL_JOIN",
                        format!("{path}.config.join"),
                        format!("unsupported join mode `{join}`"),
                    );
                }
                if matches!(config.max_concurrency, Some(0)) {
                    self.error(
                        "PARALLEL_CONCURRENCY",
                        format!("{path}.config.maxConcurrency"),
                        "maxConcurrency must be at least 1",
                    );
                }
                self.walk_expressions(&node.config, &format!("{path}.config"), scope);

                let branches: Vec<Branch> = match (&node.branches, &node.body) {
                    (Some(_), Some(_)) => {
                        self.error(
                            "PARALLEL_AMBIGUOUS",
                            path,
                            "parallel node cannot define both branches and body",
                        );
                        node.branches.clone().unwrap_or_default()
                    }
                    (Some(branches), None) => branches.clone(),
                    (None, Some(body)) => vec![Branch {
                        id: "main".to_string(),
                        graph: body.clone(),
                    }],
                    (None, None) => {
                        self.error(
                            "PARALLEL_BRANCHES",
                            path,
                            "parallel node requires branches or body",
                        );
                        Vec::new()
                    }
                };

                if join == "n_of" {
                    let n_of = config.n_of.unwrap_or(0);
                    if n_of == 0 || n_of as usize > branches.len() {
                        self.error(
                            "PARALLEL_N_OF",
                            format!("{path}.config.nOf"),
                            format!(
                                "nOf must be between 1 and the branch count ({})",
                                branches.len()
                            ),
                        );
                    }
                }

                let mut branch_ids = BTreeSet::new();
                for (index, branch) in branches.iter().enumerate() {
                    if branch.id.is_empty() {
                        self.error(
                            "PARALLEL_BRANCH",
                            format!("{path}.branches[{index}]"),
                            "branch id must not be empty",
                        );
                    } else if !branch_ids.insert(branch.id.clone()) {
                        self.error(
                            "PARALLEL_BRANCH",
                            format!("{path}.branches[{index}]"),
                            format!("duplicate branch id `{}`", branch.id),
                        );
                    }
                    let mut branch_scope = scope.clone();
                    branch_scope.allow_parallel = true;
                    self.build_graph(
                        &branch.graph,
                        &parallel_graph_id(&node.id, &branch.id),
                        GraphKind::ParallelBranch,
                        Some(node.id.clone()),
                        &branch_scope,
                        &format!("{path}.branches[{index}].graph"),
                        depth + 1,
                    );
                }
            }
            NodeKind::HumanApproval => {
                let Some(config) = self.parse_config::<ApprovalConfig>(node, path) else {
                    return;
                };
                if let Some(title) = &config.title
                    && title.trim().is_empty()
                {
                    self.error(
                        "APPROVAL_TITLE",
                        format!("{path}.config.title"),
                        "approval title must not be empty",
                    );
                }
                if let Some(form_schema) = &config.form_schema
                    && !form_schema.is_object()
                {
                    self.error(
                        "APPROVAL_FORM",
                        format!("{path}.config.formSchema"),
                        "formSchema must be an object",
                    );
                }
                if !matches!(config.on_timeout(), "approve" | "reject") {
                    self.error(
                        "APPROVAL_TIMEOUT",
                        format!("{path}.config.onTimeout"),
                        "onTimeout must be `approve` or `reject`",
                    );
                }
                self.walk_expressions(&node.config, &format!("{path}.config"), scope);
            }
            NodeKind::Subflow => {
                let Some(config) = self.parse_config::<SubflowConfig>(node, path) else {
                    return;
                };
                match &config.subflow_id {
                    Some(id) if self.workflow.find_subflow(id).is_some() => {}
                    Some(id) => self.error(
                        "SUBFLOW_MISSING",
                        format!("{path}.config.subflowId"),
                        format!("subflow `{id}` does not exist"),
                    ),
                    None => self.error(
                        "SUBFLOW_MISSING",
                        format!("{path}.config.subflowId"),
                        "subflow node requires subflowId",
                    ),
                }
                self.walk_expressions(&node.config, &format!("{path}.config"), scope);
            }
            NodeKind::HttpRequest => {
                let Some(config) = self.parse_config::<HttpConfig>(node, path) else {
                    return;
                };
                let method = config.method.as_deref().unwrap_or("GET");
                if !matches!(
                    method,
                    "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS"
                ) {
                    self.error(
                        "HTTP_METHOD",
                        format!("{path}.config.method"),
                        format!("unsupported HTTP method `{method}`"),
                    );
                }
                match &config.url {
                    None => self.error(
                        "HTTP_URL",
                        format!("{path}.config.url"),
                        "http.request requires a url",
                    ),
                    Some(Value::String(url)) if url.trim().is_empty() => self.error(
                        "HTTP_URL",
                        format!("{path}.config.url"),
                        "url must not be empty",
                    ),
                    Some(Value::String(_)) | Some(Value::Object(_)) => {}
                    Some(_) => self.error(
                        "HTTP_URL",
                        format!("{path}.config.url"),
                        "url must be a string or a $expr object",
                    ),
                }
                self.walk_expressions(&node.config, &format!("{path}.config"), scope);
            }
            NodeKind::Script => {
                let Some(config) = self.parse_config::<ScriptConfig>(node, path) else {
                    return;
                };
                let language = config.language.as_deref().unwrap_or("jq");
                if language != "jq" {
                    self.error(
                        "SCRIPT_LANGUAGE",
                        format!("{path}.config.language"),
                        format!("M0 supports only jq scripts, got `{language}`"),
                    );
                }
                match config.source.as_deref() {
                    Some(source) if !source.trim().is_empty() => {
                        self.check_expression_at(source, &format!("{path}.config.source"), scope);
                    }
                    _ => self.error(
                        "SCRIPT_SOURCE",
                        format!("{path}.config.source"),
                        "script requires a non-empty source",
                    ),
                }
                self.walk_expressions(&node.config, &format!("{path}.config"), scope);
            }
            NodeKind::SetVariable => {
                let Some(config) = self.parse_config::<SetVariableConfig>(node, path) else {
                    return;
                };
                match config.name.as_deref() {
                    Some(name) if !name.is_empty() => {}
                    _ => self.error(
                        "SET_VARIABLE_NAME",
                        format!("{path}.config.name"),
                        "setVariable requires a name",
                    ),
                }
                if let Some(value) = &config.value {
                    self.walk_expressions(value, &format!("{path}.config.value"), scope);
                }
            }
            NodeKind::Join => {
                let _ = self.parse_config::<JoinConfig>(node, path);
                self.walk_expressions(&node.config, &format!("{path}.config"), scope);
            }
        }

        if !matches!(kind, NodeKind::Loop | NodeKind::Parallel)
            && (node.body.is_some() || node.branches.is_some())
        {
            self.warning(
                "NODE_GRAPH_IGNORED",
                path,
                "body/branches are only used by loop and parallel nodes",
            );
        }
    }

    fn validate_node_edges(
        &mut self,
        node: &Node,
        path: &str,
        outgoing: &BTreeMap<String, Vec<Edge>>,
    ) {
        let edges = outgoing.get(&node.id).cloned().unwrap_or_default();
        let has_port = |port: &str| edges.iter().any(|edge| edge.source_port == port);
        match node.kind() {
            Some(NodeKind::Condition) => {
                if !has_port("true") {
                    self.warning("BRANCH_MISSING", path, "condition has no `true` branch");
                }
                if !has_port("false") {
                    self.warning("BRANCH_MISSING", path, "condition has no `false` branch");
                }
            }
            Some(NodeKind::HumanApproval) if !has_port("approved") && !has_port("rejected") => {
                self.warning(
                    "BRANCH_MISSING",
                    path,
                    "approval node has no approved or rejected branch",
                );
            }
            _ => {}
        }
    }

    fn validate_registry_definition(&mut self, node: &Node, path: &str) -> bool {
        let Some(definition) = registry::definition(&node.node_type) else {
            self.error(
                "NODE_TYPE",
                path,
                format!("node type `{}` is not registered", node.node_type),
            );
            return false;
        };
        let resolved = match registry::resolve_version(&node.node_type, &node.type_version) {
            Ok(resolved) => resolved,
            Err(message) => {
                self.error("NODE_VERSION", path, message);
                return false;
            }
        };
        if resolved.deprecated {
            self.warning(
                "NODE_VERSION_DEPRECATED",
                path,
                format!(
                    "node `{}` uses deprecated typeVersion `{}`",
                    node.node_type, resolved.version
                ),
            );
        }
        let Some(version) = definition
            .versions
            .iter()
            .find(|version| version.version == resolved.version)
        else {
            return true;
        };
        match jsonschema::validator_for(&version.config_schema) {
            Ok(validator) => {
                for error in validator.iter_errors(&node.config) {
                    let pointer = error.instance_path().to_string();
                    self.error(
                        "NODE_CONFIG_SCHEMA",
                        format!("{path}.config{pointer}"),
                        error.to_string(),
                    );
                }
            }
            Err(error) => {
                self.error(
                    "NODE_CONFIG_SCHEMA",
                    path,
                    format!("node config schema does not compile: {error}"),
                );
            }
        }
        true
    }

    fn validate_permissions(&mut self, node: &Node, path: &str, kind: NodeKind) {
        if let Some(permissions) = &node.permissions {
            let Some(object) = permissions.as_object() else {
                self.error(
                    "PERMISSION_SHAPE",
                    format!("{path}.permissions"),
                    "permissions must be an object",
                );
                return;
            };
            for key in registry::invalid_permission_keys(permissions) {
                self.error(
                    "PERMISSION_UNSUPPORTED",
                    format!("{path}.permissions.{key}"),
                    format!("unknown permission `{key}`"),
                );
            }
            let allowed = registry::permission_kinds(&node.node_type);
            for key in object.keys() {
                if !allowed.contains(key.as_str()) {
                    self.error(
                        "PERMISSION_UNSUPPORTED",
                        format!("{path}.permissions.{key}"),
                        format!(
                            "node type `{}` cannot request permission `{key}`",
                            node.node_type
                        ),
                    );
                }
            }
            if let Some(network) = object.get("network") {
                let Some(entries) = network.as_array() else {
                    self.error(
                        "PERMISSION_NETWORK",
                        format!("{path}.permissions.network"),
                        "network permissions must be an array",
                    );
                    return;
                };
                for (index, entry) in entries.iter().enumerate() {
                    let Some(raw) = entry.as_str() else {
                        self.error(
                            "PERMISSION_NETWORK",
                            format!("{path}.permissions.network[{index}]"),
                            "network permission must be a string",
                        );
                        continue;
                    };
                    if let Err(message) = registry::parse_network_rule(raw) {
                        self.error(
                            "PERMISSION_NETWORK",
                            format!("{path}.permissions.network[{index}]"),
                            message,
                        );
                    }
                }
            }
            for key in [
                "filesystem",
                "commands",
                "connectors",
                "models",
                "mcpServers",
            ] {
                if object.contains_key(key) {
                    self.error(
                        "PERMISSION_UNSUPPORTED",
                        format!("{path}.permissions.{key}"),
                        format!("`{key}` permissions are not executable in the local runtime"),
                    );
                }
            }
        }
        if kind == NodeKind::HttpRequest {
            self.validate_literal_url_permission(node, path);
        }
    }

    fn validate_execution_policy(&mut self, node: &Node, path: &str) {
        if node.timeout_ms == Some(0) {
            self.error(
                "POLICY_TIMEOUT",
                format!("{path}.timeoutMs"),
                "timeoutMs must be at least 1",
            );
        }
        if let Some(retry) = &node.retry {
            if retry.max_attempts == Some(0) {
                self.error(
                    "POLICY_RETRY",
                    format!("{path}.retry.maxAttempts"),
                    "maxAttempts must be at least 1",
                );
            }
            if let Some(backoff) = &retry.backoff {
                if backoff.base_ms == Some(0) {
                    self.error(
                        "POLICY_RETRY",
                        format!("{path}.retry.backoff.baseMs"),
                        "baseMs must be at least 1",
                    );
                }
                if backoff.max_ms == Some(0) {
                    self.error(
                        "POLICY_RETRY",
                        format!("{path}.retry.backoff.maxMs"),
                        "maxMs must be at least 1",
                    );
                }
                if let (Some(base), Some(max)) = (backoff.base_ms, backoff.max_ms)
                    && base > max
                {
                    self.error(
                        "POLICY_RETRY",
                        format!("{path}.retry.backoff"),
                        "maxMs must be greater than or equal to baseMs",
                    );
                }
            }
        }
        if let Some(policy) = &node.error_policy {
            if policy.mode == ErrorMode::Compensate {
                self.error(
                    "POLICY_UNSUPPORTED",
                    format!("{path}.errorPolicy.mode"),
                    "compensate mode is not supported by the local runtime",
                );
            }
            if policy.mode == ErrorMode::Fallback {
                match policy.fallback_port.as_deref() {
                    Some(port)
                        if node
                            .output_ports()
                            .map(|ports| ports.iter().any(|candidate| candidate == port))
                            .unwrap_or(false) => {}
                    Some(port) => self.error(
                        "POLICY_ERROR_HANDLING",
                        format!("{path}.errorPolicy.fallbackPort"),
                        format!("`{port}` is not an output port of `{}`", node.node_type),
                    ),
                    None => self.error(
                        "POLICY_ERROR_HANDLING",
                        format!("{path}.errorPolicy.fallbackPort"),
                        "fallback mode requires a fallbackPort",
                    ),
                }
            }
        }
    }

    fn validate_workflow_policy(&mut self) {
        if let Some(timeout) = &self.workflow.timeout
            && timeout.execution_ms == Some(0)
        {
            self.error(
                "POLICY_TIMEOUT",
                "$.timeout.executionMs",
                "executionMs must be at least 1",
            );
        }
        let Some(policies) = &self.workflow.policies else {
            return;
        };
        if let Some(concurrency) = &policies.concurrency
            && concurrency.limit == Some(0)
        {
            self.error(
                "POLICY_LIMIT",
                "$.policies.concurrency.limit",
                "concurrency limit must be at least 1",
            );
        }
        if policies.data_retention_days == Some(0) {
            self.error(
                "POLICY_LIMIT",
                "$.policies.dataRetentionDays",
                "dataRetentionDays must be at least 1",
            );
        }
        if let Some(budget) = &policies.budget {
            if budget.max_cost_usd.is_some_and(|value| value < 0.0) {
                self.error(
                    "POLICY_LIMIT",
                    "$.policies.budget.maxCostUsd",
                    "maxCostUsd cannot be negative",
                );
            }
            if budget.max_tokens == Some(0) {
                self.error(
                    "POLICY_LIMIT",
                    "$.policies.budget.maxTokens",
                    "maxTokens must be at least 1",
                );
            }
        }
    }

    fn validate_literal_url_permission(&mut self, node: &Node, path: &str) {
        let Ok(config) = node.config_as::<HttpConfig>() else {
            return;
        };
        let Some(Value::String(raw)) = config.url else {
            return;
        };
        let url = match reqwest::Url::parse(&raw) {
            Ok(url) => url,
            Err(error) => {
                self.error(
                    "HTTP_URL",
                    format!("{path}.config.url"),
                    format!("invalid URL: {error}"),
                );
                return;
            }
        };
        if !matches!(url.scheme(), "http" | "https") {
            self.error(
                "HTTP_URL",
                format!("{path}.config.url"),
                "only http:// and https:// URLs are supported",
            );
            return;
        }
        if !url.username().is_empty() || url.password().is_some() {
            self.error(
                "HTTP_URL",
                format!("{path}.config.url"),
                "credentials in URLs are not allowed",
            );
            return;
        }
        let Some(host) = url.host_str() else {
            self.error("HTTP_URL", format!("{path}.config.url"), "URL has no host");
            return;
        };
        let Some(port) = url.port_or_known_default() else {
            self.error("HTTP_URL", format!("{path}.config.url"), "URL has no port");
            return;
        };
        let policy = match registry::NetworkPolicy::from_permissions(node.permissions.as_ref()) {
            Ok(policy) => policy,
            Err(message) => {
                self.error("PERMISSION_NETWORK", format!("{path}.permissions"), message);
                return;
            }
        };
        if !policy.allows(host, port) {
            self.error(
                "PERMISSION_NETWORK",
                format!("{path}.config.url"),
                format!(
                    "URL host `{host}:{port}` is not allowed; declare permissions.network or use a loopback host"
                ),
            );
        }
    }

    fn parse_config<T: DeserializeOwned>(&mut self, node: &Node, path: &str) -> Option<T> {
        match node.config_as::<T>() {
            Ok(config) => Some(config),
            Err(error) => {
                self.error(
                    "NODE_CONFIG",
                    format!("{path}.config"),
                    format!("invalid config for `{}`: {error}", node.node_type),
                );
                None
            }
        }
    }

    fn require_expr(&mut self, value: &Value, path: &str, purpose: &str) -> Option<String> {
        let Some(map) = value.as_object() else {
            self.error(
                "EXPR_SHAPE",
                path,
                format!("{purpose} requires an object with `$expr`"),
            );
            return None;
        };
        if map.contains_key("$template") {
            self.error(
                "EXPR_SHAPE",
                path,
                format!("{purpose} requires `$expr`, not `$template`"),
            );
            return None;
        }
        match map.get("$expr") {
            Some(Value::String(source)) if map.len() == 1 => Some(source.clone()),
            Some(Value::String(_)) => {
                self.error(
                    "EXPR_SHAPE",
                    path,
                    format!("{purpose} expression object must contain only `$expr`"),
                );
                None
            }
            Some(_) => {
                self.error("EXPR_SHAPE", path, "`$expr` must be a string");
                None
            }
            None => {
                self.error("EXPR_SHAPE", path, format!("{purpose} requires `$expr`"));
                None
            }
        }
    }

    fn walk_expressions(&mut self, value: &Value, path: &str, scope: &ExpressionScope) {
        match value {
            Value::Object(map) => {
                let has_expr = map.contains_key("$expr");
                let has_template = map.contains_key("$template");
                if has_expr || has_template {
                    if has_expr && has_template {
                        self.error(
                            "EXPR_SHAPE",
                            path,
                            "expression object cannot contain both `$expr` and `$template`",
                        );
                    }
                    if map.len() != 1 {
                        self.error(
                            "EXPR_SHAPE",
                            path,
                            "expression object must contain exactly one key",
                        );
                    }
                    if has_expr {
                        match map.get("$expr") {
                            Some(Value::String(source)) => {
                                self.check_expression_at(source, path, scope);
                            }
                            _ => self.error("EXPR_SHAPE", path, "`$expr` must be a string"),
                        }
                    }
                    if has_template {
                        match map.get("$template") {
                            Some(Value::String(template)) => {
                                self.check_template_at(template, path, scope);
                            }
                            _ => self.error("EXPR_SHAPE", path, "`$template` must be a string"),
                        }
                    }
                    return;
                }
                for (key, child) in map {
                    let child_path = format!("{path}.{}", escape_path(key));
                    self.walk_expressions(child, &child_path, scope);
                }
            }
            Value::Array(items) => {
                for (index, child) in items.iter().enumerate() {
                    self.walk_expressions(child, &format!("{path}[{index}]"), scope);
                }
            }
            _ => {}
        }
    }

    fn check_template_at(&mut self, template: &str, path: &str, scope: &ExpressionScope) {
        match expression::parse_template(template) {
            Ok(parts) => {
                for part in parts {
                    if let TemplatePart::Expression(source) = part {
                        self.check_expression_at(&source, path, scope);
                    }
                }
            }
            Err(message) => self.error("EXPR_PARSE", path, message),
        }
    }

    fn check_expression_at(&mut self, source: &str, path: &str, scope: &ExpressionScope) {
        if let Err(message) = expression::check_forbidden(source) {
            self.error("EXPR_FORBIDDEN", path, message);
            return;
        }
        if let Err(message) = expression::check_expression(source, scope) {
            self.error("EXPR_INVALID", path, message);
        }
    }

    fn check_subflow_recursion(&mut self, subflows: &[Subflow]) {
        let mut references: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for subflow in subflows {
            let mut refs = Vec::new();
            collect_subflow_refs(&subflow.graph, &mut refs);
            refs.sort();
            refs.dedup();
            references.insert(subflow.id.clone(), refs);
        }

        let mut reported = BTreeSet::new();
        for subflow in subflows {
            let mut stack = Vec::new();
            let mut path = Vec::new();
            find_cycles(
                &subflow.id,
                &references,
                &mut stack,
                &mut path,
                &mut reported,
            );
        }

        for cycle in &reported {
            let first = cycle.first().cloned().unwrap_or_default();
            let index = subflows
                .iter()
                .position(|subflow| subflow.id == first)
                .unwrap_or(0);
            self.error(
                "SUBFLOW_RECURSION",
                format!("$.subflows[{index}]"),
                format!("subflow recursion detected: {}", cycle.join(" -> ")),
            );
        }

        if !reported.is_empty() {
            return;
        }

        for subflow in subflows {
            let mut memo = BTreeMap::new();
            let depth = subflow_depth(&subflow.id, &references, &mut memo);
            if depth > MAX_SUBFLOW_DEPTH {
                let index = subflows
                    .iter()
                    .position(|candidate| candidate.id == subflow.id)
                    .unwrap_or(0);
                self.error(
                    "SUBFLOW_DEPTH",
                    format!("$.subflows[{index}]"),
                    format!("subflow nesting depth {depth} exceeds {MAX_SUBFLOW_DEPTH}"),
                );
            }
        }
    }

    fn check_limits(&mut self) {
        if self.node_ids.len() > MAX_NODES {
            self.error(
                "LIMIT_NODES",
                "$",
                format!(
                    "workflow has {} nodes; the limit is {MAX_NODES}",
                    self.node_ids.len()
                ),
            );
        }
        if self.edge_ids.len() > MAX_EDGES {
            self.error(
                "LIMIT_EDGES",
                "$",
                format!(
                    "workflow has {} edges; the limit is {MAX_EDGES}",
                    self.edge_ids.len()
                ),
            );
        }
    }

    fn check_triggers(&mut self) {
        let triggers = self.workflow.triggers.clone();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for (index, trigger) in triggers.iter().enumerate() {
            let path = format!("$.triggers[{index}]");
            if !seen.insert(trigger.id.clone()) {
                self.error(
                    "TRIGGER_ID_DUPLICATE",
                    format!("{path}.id"),
                    format!("trigger id `{}` is used more than once", trigger.id),
                );
            }
            match crate::trigger::parse_trigger(trigger) {
                Ok(crate::trigger::TriggerSpec::Event) => {
                    self.warning(
                        "TRIGGER_EVENT_NOT_WIRED",
                        path,
                        "event triggers are accepted but no event bus is implemented yet; use schedule or webhook",
                    );
                }
                Ok(_) => {}
                Err(message) => {
                    self.error("TRIGGER_INVALID", path, message);
                }
            }
        }
    }

    fn error(&mut self, code: &str, path: impl Into<String>, message: impl Into<String>) {
        self.issues.push(Issue::error(code, path, message));
    }

    fn warning(&mut self, code: &str, path: impl Into<String>, message: impl Into<String>) {
        self.issues.push(Issue::warning(code, path, message));
    }
}

fn topological_order(
    order: &[String],
    incoming: &BTreeMap<String, Vec<Edge>>,
    outgoing: &BTreeMap<String, Vec<Edge>>,
) -> Vec<String> {
    let mut indegree: BTreeMap<&str, usize> = order
        .iter()
        .map(|id| (id.as_str(), incoming.get(id).map(Vec::len).unwrap_or(0)))
        .collect();
    let mut queue: VecDeque<&str> = order
        .iter()
        .filter(|id| indegree.get(id.as_str()).copied().unwrap_or(0) == 0)
        .map(String::as_str)
        .collect();
    let mut result = Vec::with_capacity(order.len());

    while let Some(current) = queue.pop_front() {
        result.push(current.to_string());
        if let Some(edges) = outgoing.get(current) {
            for edge in edges {
                if let Some(value) = indegree.get_mut(edge.target.as_str()) {
                    *value -= 1;
                    if *value == 0 {
                        queue.push_back(edge.target.as_str());
                    }
                }
            }
        }
    }
    result
}

fn collect_subflow_refs(graph: &Graph, out: &mut Vec<String>) {
    for node in &graph.nodes {
        if node.node_type == "subflow"
            && let Ok(config) = node.config_as::<SubflowConfig>()
            && let Some(id) = config.subflow_id
        {
            out.push(id);
        }
        if let Some(body) = &node.body {
            collect_subflow_refs(body, out);
        }
        if let Some(branches) = &node.branches {
            for branch in branches {
                collect_subflow_refs(&branch.graph, out);
            }
        }
    }
}

fn find_cycles(
    id: &str,
    references: &BTreeMap<String, Vec<String>>,
    stack: &mut Vec<String>,
    path: &mut Vec<String>,
    reported: &mut BTreeSet<Vec<String>>,
) {
    if stack.iter().any(|current| current == id) {
        if let Some(start) = stack.iter().position(|current| current == id) {
            let mut cycle: Vec<String> = stack[start..].to_vec();
            cycle.push(id.to_string());
            cycle.sort();
            cycle.dedup();
            let mut canonical = cycle;
            canonical.sort();
            reported.insert(canonical);
        }
        return;
    }
    if path.iter().any(|current| current == id) {
        return;
    }
    stack.push(id.to_string());
    path.push(id.to_string());
    if let Some(children) = references.get(id) {
        for child in children {
            find_cycles(child, references, stack, path, reported);
        }
    }
    path.pop();
    stack.pop();
}

fn subflow_depth(
    id: &str,
    references: &BTreeMap<String, Vec<String>>,
    memo: &mut BTreeMap<String, usize>,
) -> usize {
    if let Some(value) = memo.get(id) {
        return *value;
    }
    let depth = references
        .get(id)
        .map(|children| {
            children
                .iter()
                .map(|child| subflow_depth(child, references, memo) + 1)
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0);
    memo.insert(id.to_string(), depth);
    depth
}

fn escape_path(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}
