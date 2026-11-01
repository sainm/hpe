mod http;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::compiler::{CompiledGraph, ERROR_GRAPH, ExecutionPlan, GraphKind};
use crate::error::CoreError;
use crate::expression;
use crate::ir::{
    ApprovalConfig, ConditionConfig, EndConfig, ErrorHandlingMode, ErrorMode, HttpConfig,
    JoinConfig, LoopConfig, Node, NodeKind, ParallelConfig, ScriptConfig, SetVariableConfig,
    SubflowConfig, SwitchConfig, TransformConfig, Workflow,
};
use crate::registry;

const MAX_STEPS: u64 = 100_000;
const DEFAULT_NODE_TIMEOUT_MS: u64 = 30_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl ExecutionError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable: false,
            details: None,
        }
    }

    pub fn retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }
}

impl fmt::Display for ExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl From<CoreError> for ExecutionError {
    fn from(error: CoreError) -> Self {
        let code = match &error {
            CoreError::ExpressionForbidden { .. } => "EXPR_FORBIDDEN",
            CoreError::ExpressionArity { .. } => "EXPR_ARITY",
            CoreError::ExpressionValue { .. } => "EXPR_VALUE",
            CoreError::Expression { .. } => "EXPR_ERROR",
            CoreError::Json(_) | CoreError::Schema(_) | CoreError::SchemaVersion { .. } => {
                "IR_INVALID"
            }
            CoreError::Yaml(_) => "YAML_INVALID",
            CoreError::Io(_) => "IO_ERROR",
            CoreError::Invalid(_) => "INVALID",
            CoreError::Runtime(_) => "RUNTIME",
        };
        Self::new(code, error.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    Pending,
    Running,
    WaitingApproval,
    Succeeded,
    Failed,
    Cancelled,
}

impl ExecutionStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Skipped,
    Waiting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeStatus {
    Pending,
    Activated,
    Deactivated,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopContext {
    pub item: Value,
    pub index: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphFrame {
    pub graph_id: String,
    pub graph_kind: GraphKind,
    pub input: Value,
    pub vars: BTreeMap<String, Value>,
    pub loop_context: Option<LoopContext>,
    pub branch: Option<String>,
    pub statuses: BTreeMap<String, NodeStatus>,
    pub edges: BTreeMap<String, EdgeStatus>,
    pub attempts: BTreeMap<String, u32>,
    pub error: Option<ExecutionError>,
    pub started_ms: u64,
}

impl GraphFrame {
    fn new(
        graph: &CompiledGraph,
        graph_kind: GraphKind,
        input: Value,
        vars: BTreeMap<String, Value>,
        loop_context: Option<LoopContext>,
        branch: Option<String>,
    ) -> Self {
        let statuses = graph
            .order
            .iter()
            .map(|id| (id.clone(), NodeStatus::Pending))
            .collect();
        let edges = graph
            .edges
            .iter()
            .map(|edge| (edge.id.clone(), EdgeStatus::Pending))
            .collect();
        Self {
            graph_id: graph.id.clone(),
            graph_kind,
            input,
            vars,
            loop_context,
            branch,
            statuses,
            edges,
            attempts: BTreeMap::new(),
            error: None,
            started_ms: now_ms(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphResult {
    pub success: bool,
    pub output: Value,
    pub exits: BTreeMap<String, Value>,
    pub error: Option<ExecutionError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopFrame {
    pub node_id: String,
    pub body_graph_id: String,
    pub mode: String,
    pub condition: Option<String>,
    pub items: Vec<Value>,
    pub index: usize,
    pub active_index: usize,
    pub max_iterations: usize,
    pub item_variable: String,
    pub index_variable: String,
    pub results: Vec<Value>,
    pub failed: Vec<Value>,
    pub continue_on_error: bool,
    pub pending_result: Option<GraphResult>,
    pub started_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParallelBranchState {
    pub id: String,
    pub graph_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConcurrentBranchState {
    pub id: String,
    pub state: ExecutionState,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub cancelled: bool,
    #[serde(default)]
    pub sleep_until_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParallelFrame {
    pub node_id: String,
    pub branches: Vec<ParallelBranchState>,
    pub current: usize,
    pub pending_branch: Option<usize>,
    pub join: String,
    pub n_of: usize,
    pub fail_fast: bool,
    #[serde(default)]
    pub concurrent: bool,
    #[serde(default = "default_parallel_concurrency")]
    pub max_concurrency: usize,
    #[serde(default)]
    pub concurrent_states: Vec<ConcurrentBranchState>,
    pub outputs: BTreeMap<String, Value>,
    pub errors: Vec<ExecutionError>,
    pub pending_result: Option<GraphResult>,
    pub stopped: bool,
    pub started_ms: u64,
}

fn default_parallel_concurrency() -> usize {
    4
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubflowFrame {
    pub node_id: String,
    pub subflow_id: String,
    pub pending_result: Option<GraphResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "frame", rename_all = "camelCase")]
pub enum Frame {
    Graph(GraphFrame),
    Loop(LoopFrame),
    Parallel(ParallelFrame),
    Subflow(SubflowFrame),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequest {
    pub id: String,
    pub node_id: String,
    pub graph_id: String,
    pub title: String,
    pub form_schema: Option<Value>,
    pub assignees: Vec<Value>,
    pub sla_ms: Option<u64>,
    pub on_timeout: String,
    pub requested_at: String,
}

impl ApprovalRequest {
    pub fn is_overdue(&self, now: OffsetDateTime) -> bool {
        let Some(sla_ms) = self.sla_ms else {
            return false;
        };
        let Ok(requested) = OffsetDateTime::parse(&self.requested_at, &Rfc3339) else {
            return false;
        };
        let elapsed =
            now.unix_timestamp_nanos() / 1_000_000 - requested.unix_timestamp_nanos() / 1_000_000;
        elapsed >= sla_ms as i128
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionState {
    pub id: String,
    pub workflow_id: String,
    pub workflow_version: String,
    #[serde(default = "default_registry_version")]
    pub registry_version: String,
    pub status: ExecutionStatus,
    pub input: Value,
    pub variables: BTreeMap<String, Value>,
    pub node_outputs: BTreeMap<String, Value>,
    pub frames: Vec<Frame>,
    pub waiting: Option<ApprovalRequest>,
    pub output: Option<Value>,
    pub error: Option<ExecutionError>,
    #[serde(default)]
    pub pending_error: Option<ExecutionError>,
    #[serde(default)]
    pub error_graph_ran: bool,
    pub started_at: String,
    pub updated_at: String,
    pub now: String,
    pub started_ms: u64,
    pub sequence: u64,
    pub step_count: u64,
    pub tenant_id: String,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionEvent {
    pub id: String,
    pub execution_id: String,
    pub sequence: u64,
    pub kind: String,
    pub at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
    pub data: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum StepStatus {
    Continue,
    Retry { delay_ms: u64 },
    WaitingApproval,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepResult {
    pub status: StepStatus,
    pub events: Vec<ExecutionEvent>,
}

pub fn new_state(
    workflow: &Workflow,
    plan: &ExecutionPlan,
    execution_id: impl Into<String>,
    input: Value,
    env: BTreeMap<String, String>,
    tenant_id: impl Into<String>,
) -> Result<ExecutionState, ExecutionError> {
    let input = prepare_input(workflow, input)?;
    let root = plan.root_graph().ok_or_else(|| {
        ExecutionError::new("PLAN_MISSING_ROOT", "execution plan has no root graph")
    })?;
    let mut vars = BTreeMap::new();
    for variable in &workflow.variables {
        if let Some(initial) = &variable.initial {
            vars.insert(variable.name.clone(), initial.clone());
        }
    }
    let frame = GraphFrame::new(
        root,
        GraphKind::Root,
        input.clone(),
        vars.clone(),
        None,
        None,
    );
    let now = now_rfc3339();
    Ok(ExecutionState {
        id: execution_id.into(),
        workflow_id: workflow.id.clone(),
        workflow_version: workflow.workflow_version.clone(),
        registry_version: plan.registry_version.clone(),
        status: ExecutionStatus::Running,
        input,
        variables: vars,
        node_outputs: BTreeMap::new(),
        frames: vec![Frame::Graph(frame)],
        waiting: None,
        output: None,
        error: None,
        pending_error: None,
        error_graph_ran: false,
        started_at: now.clone(),
        updated_at: now.clone(),
        now,
        started_ms: now_ms(),
        sequence: 0,
        step_count: 0,
        tenant_id: tenant_id.into(),
        env,
    })
}

fn prepare_input(workflow: &Workflow, input: Value) -> Result<Value, ExecutionError> {
    let mut map = match input {
        Value::Object(map) => map,
        Value::Null => Map::new(),
        other => {
            return Err(ExecutionError::new(
                "INPUT_TYPE",
                format!("workflow input must be an object, got {other}"),
            ));
        }
    };
    for io in &workflow.inputs {
        if !map.contains_key(&io.name) {
            if let Some(default) = &io.default {
                map.insert(io.name.clone(), default.clone());
            } else if io.required {
                return Err(ExecutionError::new(
                    "INPUT_REQUIRED",
                    format!("required input `{}` is missing", io.name),
                ));
            }
        }
    }
    Ok(Value::Object(map))
}

pub async fn step(
    workflow: &Workflow,
    plan: &ExecutionPlan,
    state: &mut ExecutionState,
) -> StepResult {
    let mut events = Vec::new();

    if state.status.is_terminal() {
        return StepResult {
            status: terminal_status(state.status),
            events,
        };
    }
    if state.status == ExecutionStatus::WaitingApproval {
        return StepResult {
            status: StepStatus::WaitingApproval,
            events,
        };
    }
    if state.step_count >= MAX_STEPS {
        let error = ExecutionError::new(
            "LIMIT_STEPS",
            format!("execution exceeded {MAX_STEPS} steps"),
        );
        fail_execution(state, error, &mut events);
        return StepResult {
            status: StepStatus::Failed,
            events,
        };
    }
    if let Some(limit) = workflow
        .timeout
        .as_ref()
        .and_then(|timeout| timeout.execution_ms)
        && now_ms().saturating_sub(state.started_ms) > limit
    {
        let error = ExecutionError::new("TIMEOUT", format!("execution exceeded {limit} ms"));
        fail_execution(state, error, &mut events);
        return StepResult {
            status: StepStatus::Failed,
            events,
        };
    }

    state.step_count += 1;
    state.updated_at = now_rfc3339();

    loop {
        let Some(frame) = state.frames.last().cloned() else {
            let error = ExecutionError::new("STATE_INVALID", "execution has no frames");
            fail_execution(state, error, &mut events);
            return StepResult {
                status: StepStatus::Failed,
                events,
            };
        };

        match frame {
            Frame::Graph(mut graph_frame) => {
                let Some(compiled) = plan.graph(&graph_frame.graph_id).cloned() else {
                    let error = ExecutionError::new(
                        "PLAN_MISSING_GRAPH",
                        format!("graph `{}` is missing from the plan", graph_frame.graph_id),
                    );
                    fail_execution(state, error, &mut events);
                    return StepResult {
                        status: StepStatus::Failed,
                        events,
                    };
                };

                if graph_complete(&compiled, &graph_frame) {
                    state.frames.pop();
                    let result = graph_result(state, &compiled, &graph_frame);
                    if state.frames.is_empty() {
                        state.variables = graph_frame.vars.clone();
                        if let Some(original) = state.pending_error.take() {
                            emit(
                                state,
                                &mut events,
                                "error_graph.completed",
                                None,
                                Some(ERROR_GRAPH),
                                None,
                                json!({"success": result.success, "error": result.error}),
                                None,
                            );
                            let failure = GraphResult {
                                success: false,
                                output: Value::Null,
                                exits: BTreeMap::new(),
                                error: Some(original),
                            };
                            finish_execution(state, failure, &mut events);
                            return StepResult {
                                status: terminal_status(state.status),
                                events,
                            };
                        }
                        if !result.success
                            && !state.error_graph_ran
                            && workflow
                                .error_handling
                                .as_ref()
                                .and_then(|handling| handling.mode)
                                == Some(ErrorHandlingMode::Graph)
                            && let Some(error_graph) = plan.graph(ERROR_GRAPH)
                        {
                            let original = result.error.clone().unwrap_or_else(|| {
                                ExecutionError::new("EXECUTION_FAILED", "workflow execution failed")
                            });
                            state.pending_error = Some(original.clone());
                            state.error_graph_ran = true;
                            let frame = GraphFrame::new(
                                error_graph,
                                GraphKind::ErrorGraph,
                                state.input.clone(),
                                graph_frame.vars.clone(),
                                None,
                                None,
                            );
                            state.frames.push(Frame::Graph(frame));
                            emit(
                                state,
                                &mut events,
                                "error_graph.started",
                                None,
                                Some(ERROR_GRAPH),
                                None,
                                json!({"error": original}),
                                None,
                            );
                            continue;
                        }
                        finish_execution(state, result, &mut events);
                        return StepResult {
                            status: terminal_status(state.status),
                            events,
                        };
                    }
                    if !attach_child_result(state, result) {
                        let error = ExecutionError::new(
                            "STATE_INVALID",
                            "nested graph completed without a container frame",
                        );
                        fail_execution(state, error, &mut events);
                        return StepResult {
                            status: StepStatus::Failed,
                            events,
                        };
                    }
                    continue;
                }

                let skipped = resolve_skips(&compiled, &mut graph_frame);
                if !skipped.is_empty() {
                    replace_top(state, Frame::Graph(graph_frame));
                    for node_id in skipped {
                        emit(
                            state,
                            &mut events,
                            "node.skipped",
                            Some(&node_id),
                            Some(&compiled.id),
                            None,
                            json!({"reason": "all incoming edges deactivated"}),
                            None,
                        );
                    }
                    continue;
                }

                if reset_stale_running(&mut graph_frame) {
                    replace_top(state, Frame::Graph(graph_frame));
                    continue;
                }

                let Some(node_id) = next_ready(&compiled, &graph_frame) else {
                    let pending = graph_frame
                        .statuses
                        .values()
                        .filter(|status| matches!(status, NodeStatus::Pending))
                        .count();
                    let error = if pending == 0 {
                        ExecutionError::new("GRAPH_DEADLOCK", "graph has no runnable nodes")
                    } else {
                        ExecutionError::new(
                            "GRAPH_DEADLOCK",
                            format!("{pending} node(s) are waiting on deadlocked inputs"),
                        )
                    };
                    graph_frame.error = Some(error.clone());
                    fail_remaining(&compiled, &mut graph_frame);
                    replace_top(state, Frame::Graph(graph_frame));
                    emit(
                        state,
                        &mut events,
                        "graph.deadlock",
                        None,
                        Some(&compiled.id),
                        None,
                        json!({"error": error}),
                        None,
                    );
                    continue;
                };

                let Some(node) = compiled.node(&node_id).cloned() else {
                    let error = ExecutionError::new(
                        "PLAN_MISSING_NODE",
                        format!("node `{node_id}` is missing from the plan"),
                    );
                    graph_frame.error = Some(error.clone());
                    fail_remaining(&compiled, &mut graph_frame);
                    replace_top(state, Frame::Graph(graph_frame));
                    emit(
                        state,
                        &mut events,
                        "graph.failed",
                        None,
                        Some(&compiled.id),
                        None,
                        json!({"error": error}),
                        None,
                    );
                    continue;
                };

                let context = context_for(state, &graph_frame);

                match node.kind() {
                    Some(NodeKind::Loop) => {
                        match start_loop(
                            state,
                            plan,
                            &mut graph_frame,
                            &node,
                            &context,
                            &mut events,
                        ) {
                            Ok(action) => match action {
                                ContainerAction::Started => {
                                    replace_frame_at_depth(state, Frame::Graph(graph_frame), 1);
                                    return StepResult {
                                        status: StepStatus::Continue,
                                        events,
                                    };
                                }
                                ContainerAction::Completed => {
                                    replace_top(state, Frame::Graph(graph_frame));
                                    return StepResult {
                                        status: StepStatus::Continue,
                                        events,
                                    };
                                }
                            },
                            Err(error) => {
                                let status = handle_failure(
                                    state,
                                    &compiled,
                                    &mut graph_frame,
                                    &node,
                                    error,
                                    &mut events,
                                );
                                replace_top(state, Frame::Graph(graph_frame));
                                return StepResult { status, events };
                            }
                        }
                    }
                    Some(NodeKind::Parallel) => {
                        match start_parallel(
                            state,
                            plan,
                            &mut graph_frame,
                            &node,
                            &context,
                            &mut events,
                        ) {
                            Ok(()) => {
                                replace_frame_at_depth(state, Frame::Graph(graph_frame), 1);
                                return StepResult {
                                    status: StepStatus::Continue,
                                    events,
                                };
                            }
                            Err(error) => {
                                let status = handle_failure(
                                    state,
                                    &compiled,
                                    &mut graph_frame,
                                    &node,
                                    error,
                                    &mut events,
                                );
                                replace_top(state, Frame::Graph(graph_frame));
                                return StepResult { status, events };
                            }
                        }
                    }
                    Some(NodeKind::Subflow) => {
                        match start_subflow(
                            state,
                            plan,
                            &mut graph_frame,
                            &node,
                            &context,
                            &mut events,
                        ) {
                            Ok(()) => {
                                replace_frame_at_depth(state, Frame::Graph(graph_frame), 2);
                                return StepResult {
                                    status: StepStatus::Continue,
                                    events,
                                };
                            }
                            Err(error) => {
                                let status = handle_failure(
                                    state,
                                    &compiled,
                                    &mut graph_frame,
                                    &node,
                                    error,
                                    &mut events,
                                );
                                replace_top(state, Frame::Graph(graph_frame));
                                return StepResult { status, events };
                            }
                        }
                    }
                    Some(NodeKind::HumanApproval) => {
                        match start_approval(state, &mut graph_frame, &node, &mut events) {
                            Ok(()) => {
                                replace_top(state, Frame::Graph(graph_frame));
                                return StepResult {
                                    status: StepStatus::WaitingApproval,
                                    events,
                                };
                            }
                            Err(error) => {
                                let status = handle_failure(
                                    state,
                                    &compiled,
                                    &mut graph_frame,
                                    &node,
                                    error,
                                    &mut events,
                                );
                                replace_top(state, Frame::Graph(graph_frame));
                                return StepResult { status, events };
                            }
                        }
                    }
                    Some(_) => {
                        let attempt = graph_frame
                            .attempts
                            .get(&node.id)
                            .copied()
                            .unwrap_or(0)
                            .saturating_add(1);
                        graph_frame.attempts.insert(node.id.clone(), attempt);
                        graph_frame
                            .statuses
                            .insert(node.id.clone(), NodeStatus::Running);
                        emit(
                            state,
                            &mut events,
                            "node.started",
                            Some(&node.id),
                            Some(&compiled.id),
                            Some(attempt),
                            json!({"type": node.node_type}),
                            None,
                        );
                        replace_top(state, Frame::Graph(graph_frame.clone()));

                        let timeout_ms = node.timeout_ms.unwrap_or(DEFAULT_NODE_TIMEOUT_MS).max(1);
                        let mut working = graph_frame;
                        let result = tokio::time::timeout(
                            Duration::from_millis(timeout_ms),
                            execute_leaf(&node, &context, &mut working),
                        )
                        .await;
                        let outcome = match result {
                            Ok(result) => result,
                            Err(_) => Err(ExecutionError::new(
                                "TIMEOUT",
                                format!("node `{}` timed out after {timeout_ms} ms", node.id),
                            )
                            .retryable(true)),
                        };

                        match outcome {
                            Ok(leaf) => {
                                complete_node(
                                    state,
                                    &compiled,
                                    &mut working,
                                    &node.id,
                                    leaf.output.clone(),
                                    &leaf.ports,
                                    NodeStatus::Succeeded,
                                );
                                emit(
                                    state,
                                    &mut events,
                                    "node.succeeded",
                                    Some(&node.id),
                                    Some(&compiled.id),
                                    Some(attempt),
                                    json!({"output": leaf.output, "ports": leaf.ports}),
                                    None,
                                );
                                replace_top(state, Frame::Graph(working));
                                return StepResult {
                                    status: StepStatus::Continue,
                                    events,
                                };
                            }
                            Err(error) => {
                                if should_retry(&node, &error, attempt) {
                                    working
                                        .statuses
                                        .insert(node.id.clone(), NodeStatus::Pending);
                                    let delay = retry_delay(&node, attempt);
                                    emit(
                                        state,
                                        &mut events,
                                        "node.retrying",
                                        Some(&node.id),
                                        Some(&compiled.id),
                                        Some(attempt),
                                        json!({"error": error, "delayMs": delay}),
                                        None,
                                    );
                                    replace_top(state, Frame::Graph(working));
                                    return StepResult {
                                        status: StepStatus::Retry { delay_ms: delay },
                                        events,
                                    };
                                }
                                emit(
                                    state,
                                    &mut events,
                                    "node.failed",
                                    Some(&node.id),
                                    Some(&compiled.id),
                                    Some(attempt),
                                    json!({"error": error}),
                                    None,
                                );
                                let status = apply_error_policy(
                                    state,
                                    &compiled,
                                    &mut working,
                                    &node,
                                    error,
                                );
                                replace_top(state, Frame::Graph(working));
                                return StepResult { status, events };
                            }
                        }
                    }
                    None => {
                        let error = ExecutionError::new(
                            "NODE_TYPE",
                            format!("unsupported node type `{}`", node.node_type),
                        );
                        let status = handle_failure(
                            state,
                            &compiled,
                            &mut graph_frame,
                            &node,
                            error,
                            &mut events,
                        );
                        replace_top(state, Frame::Graph(graph_frame));
                        return StepResult { status, events };
                    }
                }
            }
            Frame::Loop(mut loop_frame) => {
                if loop_frame.pending_result.is_some() {
                    let result = loop_frame.pending_result.take().unwrap_or(GraphResult {
                        success: false,
                        output: Value::Null,
                        exits: BTreeMap::new(),
                        error: Some(ExecutionError::new("STATE_INVALID", "missing loop result")),
                    });
                    if result.success {
                        loop_frame.results.push(result.output);
                    } else if loop_frame.continue_on_error {
                        loop_frame.failed.push(json!({
                            "index": loop_frame.active_index,
                            "error": result.error,
                        }));
                    } else {
                        let error = result.error.unwrap_or_else(|| {
                            ExecutionError::new("LOOP_BODY_FAILED", "loop body failed")
                        });
                        let node_id = loop_frame.node_id.clone();
                        state.frames.pop();
                        let status =
                            finish_container_failure(state, plan, &node_id, error, &mut events);
                        return StepResult { status, events };
                    }
                    loop_frame.index = loop_frame.index.saturating_add(1);
                }

                let Some(parent) = parent_graph_frame(state) else {
                    let error = ExecutionError::new(
                        "STATE_INVALID",
                        "loop frame has no parent graph frame",
                    );
                    fail_execution(state, error, &mut events);
                    return StepResult {
                        status: StepStatus::Failed,
                        events,
                    };
                };
                let node_id = loop_frame.node_id.clone();
                let Some(parent_compiled) = plan.graph(&parent.graph_id).cloned() else {
                    state.frames.pop();
                    let error = ExecutionError::new(
                        "PLAN_MISSING_GRAPH",
                        format!("graph `{}` is missing from the plan", parent.graph_id),
                    );
                    fail_execution(state, error, &mut events);
                    return StepResult {
                        status: StepStatus::Failed,
                        events,
                    };
                };
                let Some(_node) = parent_compiled.node(&node_id).cloned() else {
                    state.frames.pop();
                    let error = ExecutionError::new(
                        "PLAN_MISSING_NODE",
                        format!("loop node `{node_id}` is missing"),
                    );
                    fail_execution(state, error, &mut events);
                    return StepResult {
                        status: StepStatus::Failed,
                        events,
                    };
                };
                let Some(body) = plan.graph(&loop_frame.body_graph_id).cloned() else {
                    state.frames.pop();
                    let error = ExecutionError::new(
                        "PLAN_MISSING_GRAPH",
                        format!("loop body graph `{}` is missing", loop_frame.body_graph_id),
                    );
                    let status =
                        finish_container_failure(state, plan, &node_id, error, &mut events);
                    return StepResult { status, events };
                };

                let next_item = if loop_frame.mode == "while" {
                    let loop_context = LoopContext {
                        item: Value::Null,
                        index: loop_frame.index,
                    };
                    let context = context_with_loop(state, &parent, Some(&loop_context));
                    match &loop_frame.condition {
                        Some(source) => match expression::evaluate_bool(source, &context) {
                            Ok(true) => {
                                if loop_frame.index >= loop_frame.max_iterations {
                                    let error = ExecutionError::new(
                                        "LIMIT_ITERATIONS",
                                        format!(
                                            "while loop exceeded maxIterations {}",
                                            loop_frame.max_iterations
                                        ),
                                    );
                                    state.frames.pop();
                                    let status = finish_container_failure(
                                        state,
                                        plan,
                                        &node_id,
                                        error,
                                        &mut events,
                                    );
                                    return StepResult { status, events };
                                }
                                Some(Value::Null)
                            }
                            Ok(false) => None,
                            Err(error) => {
                                state.frames.pop();
                                let status = finish_container_failure(
                                    state,
                                    plan,
                                    &node_id,
                                    error.into(),
                                    &mut events,
                                );
                                return StepResult { status, events };
                            }
                        },
                        None => {
                            state.frames.pop();
                            let error = ExecutionError::new(
                                "LOOP_CONDITION",
                                "while loop has no condition",
                            );
                            let status =
                                finish_container_failure(state, plan, &node_id, error, &mut events);
                            return StepResult { status, events };
                        }
                    }
                } else if loop_frame.index < loop_frame.items.len() {
                    Some(loop_frame.items[loop_frame.index].clone())
                } else {
                    None
                };

                match next_item {
                    Some(item) => {
                        loop_frame.active_index = loop_frame.index;
                        let mut vars = parent.vars.clone();
                        vars.insert(loop_frame.item_variable.clone(), item.clone());
                        vars.insert(loop_frame.index_variable.clone(), json!(loop_frame.index));
                        let mut child = GraphFrame::new(
                            &body,
                            GraphKind::LoopBody,
                            parent.input.clone(),
                            vars,
                            Some(LoopContext {
                                item,
                                index: loop_frame.index,
                            }),
                            parent.branch.clone(),
                        );
                        child.started_ms = now_ms();
                        replace_top(state, Frame::Loop(loop_frame));
                        state.frames.push(Frame::Graph(child));
                        return StepResult {
                            status: StepStatus::Continue,
                            events,
                        };
                    }
                    None => {
                        let count = loop_frame.results.len() + loop_frame.failed.len();
                        let output = json!({
                            "items": loop_frame.results,
                            "failed": loop_frame.failed,
                            "count": count,
                            "mode": loop_frame.mode,
                        });
                        let port = if loop_frame.index == 0 {
                            "empty"
                        } else {
                            "out"
                        };
                        state.frames.pop();
                        let status = finish_container_success(
                            state,
                            plan,
                            &node_id,
                            output,
                            &[port],
                            &mut events,
                        );
                        return StepResult { status, events };
                    }
                }
            }
            Frame::Parallel(mut parallel_frame) => {
                if parallel_frame.concurrent {
                    return step_concurrent_parallel(
                        workflow,
                        plan,
                        state,
                        parallel_frame,
                        &mut events,
                    )
                    .await;
                }
                if let Some(result) = parallel_frame.pending_result.take() {
                    if let Some(index) = parallel_frame.pending_branch.take()
                        && let Some(branch) = parallel_frame.branches.get(index).cloned()
                    {
                        if result.success {
                            parallel_frame
                                .outputs
                                .insert(branch.id.clone(), result.output);
                        } else if let Some(error) = result.error {
                            parallel_frame.errors.push(error);
                        }
                    }
                    if !parallel_frame.errors.is_empty() && parallel_frame.fail_fast {
                        parallel_frame.stopped = true;
                    }
                    let successes = parallel_frame.outputs.len();
                    match parallel_frame.join.as_str() {
                        "any" if successes > 0 => parallel_frame.stopped = true,
                        "n_of" if successes >= parallel_frame.n_of => {
                            parallel_frame.stopped = true;
                        }
                        _ => {}
                    }
                }

                let node_id = parallel_frame.node_id.clone();
                let pending_started = parallel_frame.pending_branch.is_some();
                if !pending_started
                    && !parallel_frame.stopped
                    && parallel_frame.current < parallel_frame.branches.len()
                {
                    let branch = parallel_frame
                        .branches
                        .get(parallel_frame.current)
                        .cloned()
                        .unwrap_or(ParallelBranchState {
                            id: String::new(),
                            graph_id: String::new(),
                        });
                    let Some(body) = plan.graph(&branch.graph_id).cloned() else {
                        state.frames.pop();
                        let error = ExecutionError::new(
                            "PLAN_MISSING_GRAPH",
                            format!("parallel branch graph `{}` is missing", branch.graph_id),
                        );
                        let status =
                            finish_container_failure(state, plan, &node_id, error, &mut events);
                        return StepResult { status, events };
                    };
                    let Some(parent) = parent_graph_frame(state) else {
                        let error = ExecutionError::new(
                            "STATE_INVALID",
                            "parallel frame has no parent graph frame",
                        );
                        fail_execution(state, error, &mut events);
                        return StepResult {
                            status: StepStatus::Failed,
                            events,
                        };
                    };
                    let child = GraphFrame::new(
                        &body,
                        GraphKind::ParallelBranch,
                        parent.input.clone(),
                        parent.vars.clone(),
                        parent.loop_context.clone(),
                        Some(branch.id.clone()),
                    );
                    parallel_frame.pending_branch = Some(parallel_frame.current);
                    parallel_frame.current = parallel_frame.current.saturating_add(1);
                    replace_top(state, Frame::Parallel(parallel_frame));
                    state.frames.push(Frame::Graph(child));
                    return StepResult {
                        status: StepStatus::Continue,
                        events,
                    };
                }

                let successes = parallel_frame.outputs.len();
                let has_errors = !parallel_frame.errors.is_empty();
                let join = parallel_frame.join.clone();
                let success = match join.as_str() {
                    "any" => successes > 0,
                    "n_of" => successes >= parallel_frame.n_of,
                    _ => !has_errors && successes == parallel_frame.branches.len(),
                };
                let first_error = parallel_frame.errors.first().cloned();
                let output = json!({
                    "branches": parallel_frame.outputs,
                    "errors": parallel_frame.errors,
                    "join": parallel_frame.join,
                });
                state.frames.pop();
                if success {
                    let port = if has_errors { "partial" } else { "out" };
                    let status = finish_container_success(
                        state,
                        plan,
                        &node_id,
                        output,
                        &[port],
                        &mut events,
                    );
                    return StepResult { status, events };
                }
                let error = first_error.unwrap_or_else(|| {
                    ExecutionError::new("PARALLEL_JOIN", format!("parallel join `{join}` failed"))
                });
                let status = finish_container_failure(state, plan, &node_id, error, &mut events);
                return StepResult { status, events };
            }
            Frame::Subflow(mut subflow_frame) => {
                let node_id = subflow_frame.node_id.clone();
                let Some(result) = subflow_frame.pending_result.take() else {
                    let error =
                        ExecutionError::new("STATE_INVALID", "subflow frame has no pending result");
                    state.frames.pop();
                    fail_execution(state, error, &mut events);
                    return StepResult {
                        status: StepStatus::Failed,
                        events,
                    };
                };
                state.frames.pop();
                if result.success {
                    let status = finish_container_success(
                        state,
                        plan,
                        &node_id,
                        result.output,
                        &["out"],
                        &mut events,
                    );
                    return StepResult { status, events };
                }
                let error = result.error.unwrap_or_else(|| {
                    ExecutionError::new("SUBFLOW_FAILED", "subflow execution failed")
                });
                let status = finish_container_failure(state, plan, &node_id, error, &mut events);
                return StepResult { status, events };
            }
        }
    }
}

enum ContainerAction {
    Started,
    Completed,
}

struct LeafOutcome {
    output: Value,
    ports: Vec<String>,
}

async fn execute_leaf(
    node: &Node,
    context: &Value,
    frame: &mut GraphFrame,
) -> Result<LeafOutcome, ExecutionError> {
    let kind = node.kind().ok_or_else(|| {
        ExecutionError::new(
            "NODE_TYPE",
            format!("unsupported node type `{}`", node.node_type),
        )
    })?;
    match kind {
        NodeKind::Start => Ok(LeafOutcome {
            output: json!({}),
            ports: vec!["out".to_string()],
        }),
        NodeKind::End => {
            let config: EndConfig = parse_config(node)?;
            let output = match &config.output {
                Some(value) => eval_value(value, context)?,
                None => json!({}),
            };
            Ok(LeafOutcome {
                output,
                ports: Vec::new(),
            })
        }
        NodeKind::Transform => {
            let config: TransformConfig = parse_config(node)?;
            let output = eval_value(&config.mapping, context)?;
            Ok(LeafOutcome {
                output,
                ports: vec!["out".to_string()],
            })
        }
        NodeKind::Condition => {
            let config: ConditionConfig = parse_config(node)?;
            let source = expression_source(&config.condition, "condition")?;
            let result =
                expression::evaluate_bool(&source, context).map_err(ExecutionError::from)?;
            let port = if result { "true" } else { "false" };
            Ok(LeafOutcome {
                output: json!({"result": result}),
                ports: vec![port.to_string()],
            })
        }
        NodeKind::Switch => {
            let config: SwitchConfig = parse_config(node)?;
            let mut selected = config
                .default_port
                .clone()
                .unwrap_or_else(|| "default".to_string());
            for case in &config.cases {
                let source = expression_source(&case.condition, "switch case")?;
                if expression::evaluate_bool(&source, context).map_err(ExecutionError::from)? {
                    selected = case.port.clone();
                    break;
                }
            }
            Ok(LeafOutcome {
                output: json!({"port": selected}),
                ports: vec![selected],
            })
        }
        NodeKind::Script => {
            let config: ScriptConfig = parse_config(node)?;
            let language = config.language.as_deref().unwrap_or("jq");
            if language != "jq" {
                return Err(ExecutionError::new(
                    "SCRIPT_LANGUAGE",
                    format!("M0 supports only jq scripts, got `{language}`"),
                ));
            }
            let source = config
                .source
                .ok_or_else(|| ExecutionError::new("SCRIPT_SOURCE", "script source is missing"))?;
            let output = expression::evaluate(&source, context).map_err(ExecutionError::from)?;
            Ok(LeafOutcome {
                output,
                ports: vec!["out".to_string()],
            })
        }
        NodeKind::HttpRequest => {
            let config: HttpConfig = parse_config(node)?;
            let url_value = config
                .url
                .ok_or_else(|| ExecutionError::new("HTTP_URL", "http.request requires a url"))?;
            let url_value = eval_value(&url_value, context)?;
            let mut url = url_value
                .as_str()
                .ok_or_else(|| ExecutionError::new("HTTP_URL", "url must evaluate to a string"))?
                .to_string();
            if let Some(query) = &config.query {
                let query = eval_value(query, context)?;
                if let Value::Object(map) = query {
                    let encoded = http::encode_query(&map);
                    if !encoded.is_empty() {
                        url.push(if url.contains('?') { '&' } else { '?' });
                        url.push_str(&encoded);
                    }
                } else {
                    return Err(ExecutionError::new(
                        "HTTP_QUERY",
                        "query must evaluate to an object",
                    ));
                }
            }
            let mut headers = BTreeMap::new();
            if let Some(value) = &config.headers {
                let value = eval_value(value, context)?;
                let map = value.as_object().ok_or_else(|| {
                    ExecutionError::new("HTTP_HEADERS", "headers must evaluate to an object")
                })?;
                for (key, value) in map {
                    let text = match value {
                        Value::String(text) => text.clone(),
                        other => other.to_string(),
                    };
                    headers.insert(key.clone(), text);
                }
            }
            let body = match &config.body {
                Some(value) => Some(eval_value(value, context)?),
                None => None,
            };
            let timeout_ms = config
                .timeout_ms
                .or(node.timeout_ms)
                .unwrap_or(DEFAULT_NODE_TIMEOUT_MS)
                .max(1);
            let method = config.method.unwrap_or_else(|| "GET".to_string());
            let policy = registry::NetworkPolicy::from_permissions(node.permissions.as_ref())
                .map_err(|message| ExecutionError::new("PERMISSION_NETWORK", message))?;
            let expected_status = config.expected_status;
            let max_response_bytes = config.max_response_bytes.unwrap_or(8 * 1024 * 1024);
            let response = http::request(http::HttpRequestOptions {
                method: &method,
                url: &url,
                headers: &headers,
                body: body.as_ref(),
                timeout: Duration::from_millis(timeout_ms),
                policy: &policy,
                expected_status: &expected_status,
                max_response_bytes,
            })
            .await?;
            Ok(LeafOutcome {
                output: response,
                ports: vec!["out".to_string()],
            })
        }
        NodeKind::SetVariable => {
            let config: SetVariableConfig = parse_config(node)?;
            let name = config.name.ok_or_else(|| {
                ExecutionError::new("SET_VARIABLE_NAME", "setVariable requires a name")
            })?;
            let value = match &config.value {
                Some(value) => eval_value(value, context)?,
                None => Value::Null,
            };
            frame.vars.insert(name.clone(), value.clone());
            Ok(LeafOutcome {
                output: json!({"name": name, "value": value}),
                ports: vec!["out".to_string()],
            })
        }
        NodeKind::Join => {
            let _: JoinConfig = parse_config(node)?;
            Ok(LeafOutcome {
                output: json!({}),
                ports: vec!["out".to_string()],
            })
        }
        NodeKind::Loop | NodeKind::Parallel | NodeKind::Subflow | NodeKind::HumanApproval => {
            Err(ExecutionError::new(
                "NODE_DISPATCH",
                format!(
                    "node type `{}` must be handled by a container",
                    node.node_type
                ),
            ))
        }
    }
}

fn start_loop(
    state: &mut ExecutionState,
    plan: &ExecutionPlan,
    frame: &mut GraphFrame,
    node: &Node,
    context: &Value,
    events: &mut Vec<ExecutionEvent>,
) -> Result<ContainerAction, ExecutionError> {
    let config: LoopConfig = parse_config(node)?;
    let mode = config.mode.clone().unwrap_or_default();
    let body_graph_id = crate::compiler::loop_graph_id(&node.id);
    if plan.graph(&body_graph_id).is_none() {
        return Err(ExecutionError::new(
            "PLAN_MISSING_GRAPH",
            format!("loop body graph `{body_graph_id}` is missing"),
        ));
    }
    let max_iterations = config.max_iterations() as usize;
    let (items, condition) = match mode.as_str() {
        "forEach" => {
            let items_value = config
                .items
                .clone()
                .ok_or_else(|| ExecutionError::new("LOOP_ITEMS", "forEach loop requires items"))?;
            let items_value = eval_value(&items_value, context)?;
            let items = items_value.as_array().ok_or_else(|| {
                ExecutionError::new("LOOP_ITEMS", "loop items must evaluate to an array")
            })?;
            if items.len() > max_iterations {
                return Err(ExecutionError::new(
                    "LIMIT_ITERATIONS",
                    format!(
                        "loop has {} items but maxIterations is {max_iterations}",
                        items.len()
                    ),
                ));
            }
            (items.clone(), None)
        }
        "while" => {
            let source = config
                .condition
                .as_ref()
                .ok_or_else(|| {
                    ExecutionError::new("LOOP_CONDITION", "while loop requires a condition")
                })
                .and_then(|value| expression_source(value, "while loop"))?;
            (Vec::new(), Some(source))
        }
        other => {
            return Err(ExecutionError::new(
                "LOOP_MODE",
                format!("unsupported loop mode `{other}`"),
            ));
        }
    };

    frame.statuses.insert(node.id.clone(), NodeStatus::Running);
    emit(
        state,
        events,
        "node.started",
        Some(&node.id),
        Some(&frame.graph_id),
        None,
        json!({"type": "loop"}),
        None,
    );
    if mode == "forEach" && items.is_empty() {
        let output = json!({"items": [], "failed": [], "count": 0, "mode": mode});
        complete_node(
            state,
            &plan.graph(&frame.graph_id).cloned().ok_or_else(|| {
                ExecutionError::new("PLAN_MISSING_GRAPH", "loop parent graph is missing")
            })?,
            frame,
            &node.id,
            output.clone(),
            &["empty".to_string()],
            NodeStatus::Succeeded,
        );
        emit(
            state,
            events,
            "node.succeeded",
            Some(&node.id),
            Some(&frame.graph_id),
            None,
            json!({"output": output, "ports": ["empty"]}),
            None,
        );
        return Ok(ContainerAction::Completed);
    }

    let loop_frame = LoopFrame {
        node_id: node.id.clone(),
        body_graph_id,
        mode,
        condition,
        items,
        index: 0,
        active_index: 0,
        max_iterations,
        item_variable: config.item_variable().to_string(),
        index_variable: config.index_variable().to_string(),
        results: Vec::new(),
        failed: Vec::new(),
        continue_on_error: config.continue_on_error.unwrap_or(false),
        pending_result: None,
        started_ms: now_ms(),
    };
    state.frames.push(Frame::Loop(loop_frame));
    Ok(ContainerAction::Started)
}

fn start_parallel(
    state: &mut ExecutionState,
    plan: &ExecutionPlan,
    frame: &mut GraphFrame,
    node: &Node,
    _context: &Value,
    events: &mut Vec<ExecutionEvent>,
) -> Result<(), ExecutionError> {
    let config: ParallelConfig = parse_config(node)?;
    let branches_config = node.branches.clone().unwrap_or_default();
    let branches: Vec<ParallelBranchState> = if branches_config.is_empty() {
        if node.body.is_some() {
            vec![ParallelBranchState {
                id: "main".to_string(),
                graph_id: crate::compiler::parallel_graph_id(&node.id, "main"),
            }]
        } else {
            return Err(ExecutionError::new(
                "PARALLEL_BRANCHES",
                "parallel node requires branches",
            ));
        }
    } else {
        branches_config
            .iter()
            .map(|branch| ParallelBranchState {
                id: branch.id.clone(),
                graph_id: crate::compiler::parallel_graph_id(&node.id, &branch.id),
            })
            .collect()
    };
    for branch in &branches {
        if plan.graph(&branch.graph_id).is_none() {
            return Err(ExecutionError::new(
                "PLAN_MISSING_GRAPH",
                format!("parallel branch graph `{}` is missing", branch.graph_id),
            ));
        }
    }
    let join = config.join().to_string();
    let n_of = config.n_of.unwrap_or(1) as usize;
    let concurrent = branches
        .iter()
        .filter_map(|branch| plan.graph(&branch.graph_id))
        .all(|graph| !compiled_has_approval(plan, graph, &mut BTreeSet::new()));
    frame.statuses.insert(node.id.clone(), NodeStatus::Running);
    let parallel_frame = ParallelFrame {
        node_id: node.id.clone(),
        branches,
        current: 0,
        pending_branch: None,
        join,
        n_of,
        fail_fast: config.fail_fast(),
        concurrent,
        max_concurrency: config.max_concurrency() as usize,
        concurrent_states: Vec::new(),
        outputs: BTreeMap::new(),
        errors: Vec::new(),
        pending_result: None,
        stopped: false,
        started_ms: now_ms(),
    };
    state.frames.push(Frame::Parallel(parallel_frame));
    emit(
        state,
        events,
        "node.started",
        Some(&node.id),
        Some(&frame.graph_id),
        None,
        json!({"type": "parallel"}),
        None,
    );
    Ok(())
}

fn start_subflow(
    state: &mut ExecutionState,
    plan: &ExecutionPlan,
    frame: &mut GraphFrame,
    node: &Node,
    context: &Value,
    events: &mut Vec<ExecutionEvent>,
) -> Result<(), ExecutionError> {
    let config: SubflowConfig = parse_config(node)?;
    let subflow_id = config
        .subflow_id
        .ok_or_else(|| ExecutionError::new("SUBFLOW_MISSING", "subflow node requires subflowId"))?;
    let graph = plan.subflow_graph(&subflow_id).ok_or_else(|| {
        ExecutionError::new(
            "SUBFLOW_MISSING",
            format!("subflow `{subflow_id}` has no compiled graph"),
        )
    })?;
    let input = match &config.input {
        Some(value) => eval_value(value, context)?,
        None => frame.input.clone(),
    };
    frame.statuses.insert(node.id.clone(), NodeStatus::Running);
    let child = GraphFrame::new(
        graph,
        GraphKind::Subflow,
        input,
        frame.vars.clone(),
        None,
        None,
    );
    state.frames.push(Frame::Subflow(SubflowFrame {
        node_id: node.id.clone(),
        subflow_id,
        pending_result: None,
    }));
    state.frames.push(Frame::Graph(child));
    emit(
        state,
        events,
        "node.started",
        Some(&node.id),
        Some(&frame.graph_id),
        None,
        json!({"type": "subflow"}),
        None,
    );
    Ok(())
}

fn start_approval(
    state: &mut ExecutionState,
    frame: &mut GraphFrame,
    node: &Node,
    events: &mut Vec<ExecutionEvent>,
) -> Result<(), ExecutionError> {
    let config: ApprovalConfig = parse_config(node)?;
    frame.statuses.insert(node.id.clone(), NodeStatus::Waiting);
    let on_timeout = config.on_timeout().to_string();
    let title = config
        .title
        .or_else(|| node.name.clone())
        .unwrap_or_else(|| format!("Approve {}", node.id));
    let request = ApprovalRequest {
        id: Uuid::now_v7().to_string(),
        node_id: node.id.clone(),
        graph_id: frame.graph_id.clone(),
        title,
        form_schema: config.form_schema,
        assignees: config.assignees,
        sla_ms: config.sla_ms,
        on_timeout,
        requested_at: now_rfc3339(),
    };
    state.waiting = Some(request.clone());
    state.status = ExecutionStatus::WaitingApproval;
    state.updated_at = now_rfc3339();
    emit(
        state,
        events,
        "approval.requested",
        Some(&node.id),
        Some(&frame.graph_id),
        None,
        json!({"approval": request}),
        None,
    );
    emit(
        state,
        events,
        "node.waiting",
        Some(&node.id),
        Some(&frame.graph_id),
        None,
        json!({"reason": "approval"}),
        None,
    );
    Ok(())
}

pub fn resume_approval(
    state: &mut ExecutionState,
    plan: &ExecutionPlan,
    node_id: &str,
    decision: &str,
    form: Value,
) -> Result<Vec<ExecutionEvent>, ExecutionError> {
    resume_approval_inner(state, plan, node_id, decision, form, false)
}

pub fn expire_approval(
    state: &mut ExecutionState,
    plan: &ExecutionPlan,
    node_id: &str,
    form: Value,
) -> Result<Vec<ExecutionEvent>, ExecutionError> {
    if state.status != ExecutionStatus::WaitingApproval {
        return Err(ExecutionError::new(
            "APPROVAL_STATE",
            "execution is not waiting for approval",
        ));
    }
    let waiting = state
        .waiting
        .clone()
        .ok_or_else(|| ExecutionError::new("APPROVAL_STATE", "approval request is missing"))?;
    let decision = match waiting.on_timeout.as_str() {
        "approve" => "approved",
        _ => "rejected",
    };
    resume_approval_inner(state, plan, node_id, decision, form, true)
}

fn resume_approval_inner(
    state: &mut ExecutionState,
    plan: &ExecutionPlan,
    node_id: &str,
    decision: &str,
    form: Value,
    timed_out: bool,
) -> Result<Vec<ExecutionEvent>, ExecutionError> {
    if state.status != ExecutionStatus::WaitingApproval {
        return Err(ExecutionError::new(
            "APPROVAL_STATE",
            "execution is not waiting for approval",
        ));
    }
    let waiting = state
        .waiting
        .clone()
        .ok_or_else(|| ExecutionError::new("APPROVAL_STATE", "approval request is missing"))?;
    if waiting.node_id != node_id {
        return Err(ExecutionError::new(
            "APPROVAL_NODE",
            format!(
                "execution is waiting for `{}`, not `{node_id}`",
                waiting.node_id
            ),
        ));
    }
    let port = match decision {
        "approved" | "rejected" => decision,
        "timeout" if !timed_out => "timeout",
        _ => {
            return Err(ExecutionError::new(
                "APPROVAL_DECISION",
                "decision must be approved, rejected or timeout",
            ));
        }
    };
    let index = state
        .frames
        .iter()
        .rposition(|frame| {
            matches!(
                frame,
                Frame::Graph(graph)
                    if graph.graph_id == waiting.graph_id
                        && graph.statuses.get(node_id) == Some(&NodeStatus::Waiting)
            )
        })
        .ok_or_else(|| {
            ExecutionError::new(
                "APPROVAL_STATE",
                "waiting node is not present in execution state",
            )
        })?;
    let Some(Frame::Graph(mut graph_frame)) = state.frames.get(index).cloned() else {
        return Err(ExecutionError::new(
            "APPROVAL_STATE",
            "waiting node is not in a graph frame",
        ));
    };
    let compiled = plan
        .graph(&graph_frame.graph_id)
        .cloned()
        .ok_or_else(|| ExecutionError::new("PLAN_MISSING_GRAPH", "approval graph is missing"))?;
    let mut output = match form {
        Value::Object(map) => Value::Object(map),
        Value::Null => json!({}),
        other => json!({"form": other}),
    };
    if let Value::Object(map) = &mut output {
        map.insert("decision".to_string(), json!(decision));
        if timed_out {
            map.insert("timedOut".to_string(), json!(true));
        }
    }
    complete_node(
        state,
        &compiled,
        &mut graph_frame,
        node_id,
        output.clone(),
        &[port.to_string()],
        NodeStatus::Succeeded,
    );
    if let Some(slot) = state.frames.get_mut(index) {
        *slot = Frame::Graph(graph_frame);
    }
    state.waiting = None;
    state.status = ExecutionStatus::Running;
    state.updated_at = now_rfc3339();
    let mut events = Vec::new();
    emit(
        state,
        &mut events,
        match decision {
            _ if timed_out => "approval.timeout",
            "approved" => "approval.approved",
            "rejected" => "approval.rejected",
            _ => "approval.timeout",
        },
        Some(node_id),
        Some(&waiting.graph_id),
        None,
        json!({"decision": decision, "timedOut": timed_out, "output": output}),
        None,
    );
    emit(
        state,
        &mut events,
        "node.succeeded",
        Some(node_id),
        Some(&waiting.graph_id),
        None,
        json!({"output": output, "decision": decision}),
        None,
    );
    Ok(events)
}

pub fn cancel_execution(state: &mut ExecutionState) -> Result<Vec<ExecutionEvent>, ExecutionError> {
    if state.status.is_terminal() {
        return Err(ExecutionError::new(
            "CANCEL_STATE",
            "execution is already terminal",
        ));
    }
    state.status = ExecutionStatus::Cancelled;
    state.waiting = None;
    state.updated_at = now_rfc3339();
    let mut events = Vec::new();
    emit(
        state,
        &mut events,
        "execution.cancelled",
        None,
        None,
        None,
        json!({}),
        None,
    );
    Ok(events)
}

fn complete_node(
    state: &mut ExecutionState,
    compiled: &CompiledGraph,
    frame: &mut GraphFrame,
    node_id: &str,
    output: Value,
    ports: &[String],
    status: NodeStatus,
) {
    frame.statuses.insert(node_id.to_string(), status);
    if status == NodeStatus::Succeeded {
        state.node_outputs.insert(node_id.to_string(), output);
    }
    if let Some(edges) = compiled.outgoing.get(node_id) {
        for edge in edges {
            let next = if status == NodeStatus::Succeeded && ports.contains(&edge.source_port) {
                EdgeStatus::Activated
            } else {
                EdgeStatus::Deactivated
            };
            frame.edges.insert(edge.id.clone(), next);
        }
    }
}

fn apply_error_policy(
    state: &mut ExecutionState,
    compiled: &CompiledGraph,
    frame: &mut GraphFrame,
    node: &Node,
    error: ExecutionError,
) -> StepStatus {
    let policy = node.error_policy.clone();
    let mode = policy
        .as_ref()
        .map(|policy| policy.mode)
        .unwrap_or(ErrorMode::Fail);
    let fallback_port = policy
        .as_ref()
        .and_then(|policy| policy.fallback_port.clone())
        .unwrap_or_else(|| "error".to_string());
    match mode {
        ErrorMode::Fallback => {
            complete_node(
                state,
                compiled,
                frame,
                &node.id,
                json!({"error": error}),
                &[fallback_port],
                NodeStatus::Succeeded,
            );
            StepStatus::Continue
        }
        ErrorMode::Continue => {
            complete_node(
                state,
                compiled,
                frame,
                &node.id,
                json!({"error": error}),
                &["out".to_string()],
                NodeStatus::Succeeded,
            );
            StepStatus::Continue
        }
        ErrorMode::Skip => {
            complete_node(
                state,
                compiled,
                frame,
                &node.id,
                Value::Null,
                &[],
                NodeStatus::Skipped,
            );
            StepStatus::Continue
        }
        ErrorMode::Fail | ErrorMode::Compensate => {
            frame.error = Some(error);
            complete_node(
                state,
                compiled,
                frame,
                &node.id,
                Value::Null,
                &[],
                NodeStatus::Failed,
            );
            fail_remaining(compiled, frame);
            StepStatus::Continue
        }
    }
}

fn handle_failure(
    state: &mut ExecutionState,
    compiled: &CompiledGraph,
    frame: &mut GraphFrame,
    node: &Node,
    error: ExecutionError,
    events: &mut Vec<ExecutionEvent>,
) -> StepStatus {
    emit(
        state,
        events,
        "node.failed",
        Some(&node.id),
        Some(&compiled.id),
        frame.attempts.get(&node.id).copied(),
        json!({"error": error}),
        None,
    );
    apply_error_policy(state, compiled, frame, node, error)
}

fn finish_container_success(
    state: &mut ExecutionState,
    plan: &ExecutionPlan,
    node_id: &str,
    output: Value,
    ports: &[&str],
    events: &mut Vec<ExecutionEvent>,
) -> StepStatus {
    let Some(parent) = state.frames.last().cloned() else {
        let error = ExecutionError::new("STATE_INVALID", "container has no parent graph");
        fail_execution(state, error, events);
        return StepStatus::Failed;
    };
    let Frame::Graph(mut graph_frame) = parent else {
        let error = ExecutionError::new("STATE_INVALID", "container parent is not a graph");
        fail_execution(state, error, events);
        return StepStatus::Failed;
    };
    let Some(compiled) = plan.graph(&graph_frame.graph_id).cloned() else {
        let error = ExecutionError::new("PLAN_MISSING_GRAPH", "container parent graph is missing");
        fail_execution(state, error, events);
        return StepStatus::Failed;
    };
    let ports: Vec<String> = ports.iter().map(|port| (*port).to_string()).collect();
    complete_node(
        state,
        &compiled,
        &mut graph_frame,
        node_id,
        output.clone(),
        &ports,
        NodeStatus::Succeeded,
    );
    let graph_id = compiled.id.clone();
    replace_top(state, Frame::Graph(graph_frame));
    emit(
        state,
        events,
        "node.succeeded",
        Some(node_id),
        Some(&graph_id),
        None,
        json!({"output": output, "ports": ports}),
        None,
    );
    StepStatus::Continue
}

fn finish_container_failure(
    state: &mut ExecutionState,
    plan: &ExecutionPlan,
    node_id: &str,
    error: ExecutionError,
    events: &mut Vec<ExecutionEvent>,
) -> StepStatus {
    let Some(parent) = state.frames.last().cloned() else {
        let error = ExecutionError::new("STATE_INVALID", "container has no parent graph");
        fail_execution(state, error, events);
        return StepStatus::Failed;
    };
    let Frame::Graph(mut graph_frame) = parent else {
        let error = ExecutionError::new("STATE_INVALID", "container parent is not a graph");
        fail_execution(state, error, events);
        return StepStatus::Failed;
    };
    let Some(compiled) = plan.graph(&graph_frame.graph_id).cloned() else {
        let error = ExecutionError::new("PLAN_MISSING_GRAPH", "container parent graph is missing");
        fail_execution(state, error, events);
        return StepStatus::Failed;
    };
    let Some(node) = compiled.node(node_id).cloned() else {
        let error = ExecutionError::new(
            "PLAN_MISSING_NODE",
            format!("container node `{node_id}` is missing"),
        );
        fail_execution(state, error, events);
        return StepStatus::Failed;
    };
    emit(
        state,
        events,
        "node.failed",
        Some(node_id),
        Some(&compiled.id),
        None,
        json!({"error": error}),
        None,
    );
    let status = apply_error_policy(state, &compiled, &mut graph_frame, &node, error);
    replace_top(state, Frame::Graph(graph_frame));
    status
}

fn attach_child_result(state: &mut ExecutionState, result: GraphResult) -> bool {
    match state.frames.last_mut() {
        Some(Frame::Loop(frame)) => {
            frame.pending_result = Some(result);
            true
        }
        Some(Frame::Parallel(frame)) => {
            frame.pending_result = Some(result);
            true
        }
        Some(Frame::Subflow(frame)) => {
            frame.pending_result = Some(result);
            true
        }
        _ => false,
    }
}

fn graph_complete(compiled: &CompiledGraph, frame: &GraphFrame) -> bool {
    compiled.order.iter().all(|id| {
        matches!(
            frame.statuses.get(id),
            Some(NodeStatus::Succeeded | NodeStatus::Skipped | NodeStatus::Failed)
        )
    })
}

fn graph_result(
    state: &ExecutionState,
    compiled: &CompiledGraph,
    frame: &GraphFrame,
) -> GraphResult {
    let success = frame.error.is_none()
        && compiled
            .order
            .iter()
            .all(|id| frame.statuses.get(id) != Some(&NodeStatus::Failed));
    let mut exits = BTreeMap::new();
    for node_id in &compiled.exits {
        if frame.statuses.get(node_id) == Some(&NodeStatus::Succeeded)
            && let Some(output) = state.node_outputs.get(node_id)
        {
            exits.insert(node_id.clone(), output.clone());
        }
    }
    let output = if exits.len() == 1 {
        exits.values().next().cloned().unwrap_or(Value::Null)
    } else if exits.is_empty() {
        Value::Null
    } else {
        Value::Object(
            exits
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        )
    };
    GraphResult {
        success,
        output,
        exits,
        error: frame.error.clone(),
    }
}

fn finish_execution(
    state: &mut ExecutionState,
    result: GraphResult,
    events: &mut Vec<ExecutionEvent>,
) {
    state.updated_at = now_rfc3339();
    if result.success {
        state.status = ExecutionStatus::Succeeded;
        state.output = Some(result.output.clone());
        state.error = None;
        emit(
            state,
            events,
            "execution.succeeded",
            None,
            None,
            None,
            json!({"output": result.output}),
            None,
        );
    } else {
        let error = result.error.unwrap_or_else(|| {
            ExecutionError::new("EXECUTION_FAILED", "workflow execution failed")
        });
        state.status = ExecutionStatus::Failed;
        state.error = Some(error.clone());
        emit(
            state,
            events,
            "execution.failed",
            None,
            None,
            None,
            json!({"error": error}),
            None,
        );
    }
}

fn fail_execution(
    state: &mut ExecutionState,
    error: ExecutionError,
    events: &mut Vec<ExecutionEvent>,
) {
    state.status = ExecutionStatus::Failed;
    state.error = Some(error.clone());
    state.updated_at = now_rfc3339();
    emit(
        state,
        events,
        "execution.failed",
        None,
        None,
        None,
        json!({"error": error}),
        None,
    );
}

fn terminal_status(status: ExecutionStatus) -> StepStatus {
    match status {
        ExecutionStatus::Succeeded => StepStatus::Succeeded,
        ExecutionStatus::Failed | ExecutionStatus::Cancelled => StepStatus::Failed,
        ExecutionStatus::WaitingApproval => StepStatus::WaitingApproval,
        ExecutionStatus::Pending | ExecutionStatus::Running => StepStatus::Continue,
    }
}

fn next_ready(compiled: &CompiledGraph, frame: &GraphFrame) -> Option<String> {
    for node_id in &compiled.order {
        if frame.statuses.get(node_id) != Some(&NodeStatus::Pending) {
            continue;
        }
        let incoming = compiled
            .incoming
            .get(node_id)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        if incoming.is_empty() {
            return Some(node_id.clone());
        }
        let mut all_terminal = true;
        let mut any_activated = false;
        for edge in incoming {
            match frame
                .edges
                .get(&edge.id)
                .copied()
                .unwrap_or(EdgeStatus::Pending)
            {
                EdgeStatus::Activated => any_activated = true,
                EdgeStatus::Deactivated => {}
                EdgeStatus::Pending => all_terminal = false,
            }
        }
        if all_terminal && any_activated {
            return Some(node_id.clone());
        }
    }
    None
}

fn resolve_skips(compiled: &CompiledGraph, frame: &mut GraphFrame) -> Vec<String> {
    let mut skipped = Vec::new();
    loop {
        let mut changed = false;
        for node_id in &compiled.order {
            if frame.statuses.get(node_id) != Some(&NodeStatus::Pending) {
                continue;
            }
            let incoming = compiled
                .incoming
                .get(node_id)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            if incoming.is_empty() {
                continue;
            }
            let all_deactivated = incoming.iter().all(|edge| {
                frame
                    .edges
                    .get(&edge.id)
                    .copied()
                    .unwrap_or(EdgeStatus::Pending)
                    == EdgeStatus::Deactivated
            });
            if !all_deactivated {
                continue;
            }
            frame.statuses.insert(node_id.clone(), NodeStatus::Skipped);
            if let Some(edges) = compiled.outgoing.get(node_id) {
                for edge in edges {
                    frame.edges.insert(edge.id.clone(), EdgeStatus::Deactivated);
                }
            }
            skipped.push(node_id.clone());
            changed = true;
        }
        if !changed {
            break;
        }
    }
    skipped
}

fn reset_stale_running(frame: &mut GraphFrame) -> bool {
    let mut changed = false;
    for status in frame.statuses.values_mut() {
        if *status == NodeStatus::Running {
            *status = NodeStatus::Pending;
            changed = true;
        }
    }
    changed
}

fn fail_remaining(compiled: &CompiledGraph, frame: &mut GraphFrame) {
    for node_id in &compiled.order {
        if matches!(
            frame.statuses.get(node_id),
            Some(NodeStatus::Pending | NodeStatus::Running | NodeStatus::Waiting)
        ) {
            frame.statuses.insert(node_id.clone(), NodeStatus::Skipped);
        }
    }
    for edge in &compiled.edges {
        frame.edges.insert(edge.id.clone(), EdgeStatus::Deactivated);
    }
}

fn replace_top(state: &mut ExecutionState, frame: Frame) {
    if let Some(slot) = state.frames.last_mut() {
        *slot = frame;
    }
}

fn replace_frame_at_depth(state: &mut ExecutionState, frame: Frame, depth: usize) {
    if let Some(index) = state.frames.len().checked_sub(depth + 1)
        && let Some(slot) = state.frames.get_mut(index)
    {
        *slot = frame;
    }
}

fn parent_graph_frame(state: &ExecutionState) -> Option<GraphFrame> {
    state.frames.iter().rev().find_map(|frame| match frame {
        Frame::Graph(graph) => Some(graph.clone()),
        _ => None,
    })
}

fn context_for(state: &ExecutionState, frame: &GraphFrame) -> Value {
    context_with_loop(state, frame, frame.loop_context.as_ref())
}

fn context_with_loop(
    state: &ExecutionState,
    frame: &GraphFrame,
    loop_context: Option<&LoopContext>,
) -> Value {
    let mut nodes = Map::new();
    for (id, output) in &state.node_outputs {
        nodes.insert(id.clone(), json!({"output": output}));
    }
    let mut vars = Map::new();
    for (key, value) in &frame.vars {
        vars.insert(key.clone(), value.clone());
    }
    let mut env = Map::new();
    for (key, value) in &state.env {
        env.insert(key.clone(), Value::String(value.clone()));
    }
    json!({
        "input": frame.input,
        "vars": Value::Object(vars),
        "nodes": Value::Object(nodes),
        "env": Value::Object(env),
        "execution": {
            "id": state.id,
            "now": state.now,
            "tenantId": state.tenant_id,
        },
        "error": state.pending_error,
        "loop": loop_context.map(|context| json!({
            "item": context.item,
            "index": context.index,
        })).unwrap_or(Value::Null),
        "parallel": frame.branch.as_ref().map(|branch| json!({"branch": branch})).unwrap_or(Value::Null),
    })
}

pub fn eval_value(value: &Value, context: &Value) -> Result<Value, ExecutionError> {
    match value {
        Value::Object(map) => {
            let has_expr = map.contains_key("$expr");
            let has_template = map.contains_key("$template");
            if has_expr || has_template {
                if map.len() != 1 || (has_expr && has_template) {
                    return Err(ExecutionError::new(
                        "EXPR_SHAPE",
                        "expression object must contain exactly one of $expr or $template",
                    ));
                }
                if let Some(source) = map.get("$expr") {
                    let source = source.as_str().ok_or_else(|| {
                        ExecutionError::new("EXPR_SHAPE", "$expr must be a string")
                    })?;
                    return expression::evaluate(source, context).map_err(ExecutionError::from);
                }
                if let Some(template) = map.get("$template") {
                    let template = template.as_str().ok_or_else(|| {
                        ExecutionError::new("EXPR_SHAPE", "$template must be a string")
                    })?;
                    return expression::render_template(template, context)
                        .map(Value::String)
                        .map_err(ExecutionError::from);
                }
            }
            let mut out = Map::new();
            for (key, child) in map {
                out.insert(key.clone(), eval_value(child, context)?);
            }
            Ok(Value::Object(out))
        }
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(eval_value(item, context)?);
            }
            Ok(Value::Array(out))
        }
        other => Ok(other.clone()),
    }
}

fn expression_source(value: &Value, purpose: &str) -> Result<String, ExecutionError> {
    let map = value.as_object().ok_or_else(|| {
        ExecutionError::new(
            "EXPR_SHAPE",
            format!("{purpose} requires an object with $expr"),
        )
    })?;
    if map.contains_key("$template") {
        return Err(ExecutionError::new(
            "EXPR_SHAPE",
            format!("{purpose} requires $expr, not $template"),
        ));
    }
    match map.get("$expr") {
        Some(Value::String(source)) if map.len() == 1 => Ok(source.clone()),
        _ => Err(ExecutionError::new(
            "EXPR_SHAPE",
            format!("{purpose} requires a $expr string"),
        )),
    }
}

fn parse_config<T: serde::de::DeserializeOwned>(node: &Node) -> Result<T, ExecutionError> {
    node.config_as::<T>().map_err(|error| {
        ExecutionError::new(
            "NODE_CONFIG",
            format!("invalid config for `{}`: {error}", node.node_type),
        )
    })
}

fn should_retry(node: &Node, error: &ExecutionError, attempt: u32) -> bool {
    let Some(retry) = &node.retry else {
        return false;
    };
    if attempt >= retry.attempts() {
        return false;
    }
    if retry.non_retryable.iter().any(|code| code == &error.code) {
        return false;
    }
    if !retry.retry_on.is_empty() {
        return retry
            .retry_on
            .iter()
            .any(|code| code == "*" || code == &error.code);
    }
    error.retryable
}

fn retry_delay(node: &Node, attempt: u32) -> u64 {
    let Some(retry) = &node.retry else {
        return 0;
    };
    let Some(backoff) = &retry.backoff else {
        return 0;
    };
    let base = backoff.base_ms();
    let max = backoff.max_ms();
    let raw = match backoff.kind() {
        crate::ir::BackoffKind::Constant => base,
        crate::ir::BackoffKind::Linear => base.saturating_mul(attempt as u64),
        crate::ir::BackoffKind::Exponential => {
            let factor = 1u64 << (attempt.saturating_sub(1)).min(20);
            base.saturating_mul(factor)
        }
    };
    let raw = raw.min(max);
    if backoff.jitter() {
        let spread = (raw / 5).max(1);
        raw.saturating_add(now_ms() % spread)
    } else {
        raw
    }
}

#[allow(clippy::too_many_arguments)]
fn emit(
    state: &mut ExecutionState,
    events: &mut Vec<ExecutionEvent>,
    kind: &str,
    node_id: Option<&str>,
    graph_id: Option<&str>,
    attempt: Option<u32>,
    data: Value,
    message: Option<String>,
) {
    state.sequence = state.sequence.saturating_add(1);
    events.push(ExecutionEvent {
        id: Uuid::now_v7().to_string(),
        execution_id: state.id.clone(),
        sequence: state.sequence,
        kind: kind.to_string(),
        at: now_rfc3339(),
        node_id: node_id.map(str::to_string),
        graph_id: graph_id.map(str::to_string),
        attempt,
        data,
        message,
    });
}

fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn default_registry_version() -> String {
    "legacy".to_string()
}

pub fn execution_output(state: &ExecutionState) -> Option<Value> {
    state.output.clone()
}

pub fn start_event(state: &mut ExecutionState) -> ExecutionEvent {
    state.sequence = state.sequence.saturating_add(1);
    ExecutionEvent {
        id: Uuid::now_v7().to_string(),
        execution_id: state.id.clone(),
        sequence: state.sequence,
        kind: "execution.started".to_string(),
        at: now_rfc3339(),
        node_id: None,
        graph_id: None,
        attempt: None,
        data: json!({"workflowId": state.workflow_id, "workflowVersion": state.workflow_version}),
        message: None,
    }
}

pub fn pending_node_ids(state: &ExecutionState) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for frame in &state.frames {
        if let Frame::Graph(graph) = frame {
            for (id, status) in &graph.statuses {
                if matches!(status, NodeStatus::Waiting | NodeStatus::Running) {
                    ids.insert(id.clone());
                }
            }
        }
    }
    ids
}

fn graph_has_approval(
    plan: &ExecutionPlan,
    graph: &crate::ir::Graph,
    seen: &mut BTreeSet<String>,
) -> bool {
    graph
        .nodes
        .iter()
        .any(|node| node_has_approval(plan, node, seen))
}

fn node_has_approval(plan: &ExecutionPlan, node: &Node, seen: &mut BTreeSet<String>) -> bool {
    if node.kind() == Some(NodeKind::HumanApproval) {
        return true;
    }
    if let Some(body) = &node.body
        && graph_has_approval(plan, body, seen)
    {
        return true;
    }
    if let Some(branches) = &node.branches
        && branches
            .iter()
            .any(|branch| graph_has_approval(plan, &branch.graph, seen))
    {
        return true;
    }
    if node.node_type == "subflow"
        && let Ok(config) = node.config_as::<SubflowConfig>()
        && let Some(id) = config.subflow_id
        && seen.insert(id.clone())
        && let Some(graph) = plan.subflow_graph(&id)
        && graph
            .nodes
            .values()
            .any(|node| node_has_approval(plan, node, seen))
    {
        return true;
    }
    false
}

fn compiled_has_approval(
    plan: &ExecutionPlan,
    graph: &CompiledGraph,
    seen: &mut BTreeSet<String>,
) -> bool {
    graph
        .nodes
        .values()
        .any(|node| node_has_approval(plan, node, seen))
}

async fn step_concurrent_parallel(
    workflow: &Workflow,
    plan: &ExecutionPlan,
    state: &mut ExecutionState,
    mut frame: ParallelFrame,
    events: &mut Vec<ExecutionEvent>,
) -> StepResult {
    if frame.concurrent_states.is_empty() {
        let Some(parent) = parent_graph_frame(state) else {
            let error =
                ExecutionError::new("STATE_INVALID", "parallel frame has no parent graph frame");
            fail_execution(state, error, events);
            return StepResult {
                status: StepStatus::Failed,
                events: std::mem::take(events),
            };
        };
        for branch in &frame.branches {
            let branch_state = match new_branch_state(workflow, plan, state, &parent, branch) {
                Ok(branch_state) => branch_state,
                Err(error) => {
                    state.frames.pop();
                    let status =
                        finish_container_failure(state, plan, &frame.node_id, error, events);
                    return StepResult {
                        status,
                        events: std::mem::take(events),
                    };
                }
            };
            frame.concurrent_states.push(ConcurrentBranchState {
                id: branch.id.clone(),
                state: branch_state,
                done: false,
                cancelled: false,
                sleep_until_ms: 0,
            });
        }
    }

    let now = now_ms();
    let semaphore = Arc::new(Semaphore::new(frame.max_concurrency.max(1)));
    let futures = frame
        .concurrent_states
        .iter_mut()
        .filter(|item| {
            !item.done
                && !item.cancelled
                && item.sleep_until_ms <= now
                && !item.state.status.is_terminal()
        })
        .map(|item| {
            let semaphore = Arc::clone(&semaphore);
            async move {
                let permit = match semaphore.acquire().await {
                    Ok(permit) => permit,
                    Err(_) => {
                        return (
                            item,
                            Err(ExecutionError::new(
                                "PARALLEL_SEMAPHORE",
                                "parallel semaphore closed",
                            )),
                        );
                    }
                };
                let result = step(workflow, plan, &mut item.state).await;
                drop(permit);
                (item, Ok(result))
            }
        })
        .collect::<Vec<_>>();

    for (item, outcome) in join_all(futures).await {
        let result = match outcome {
            Ok(result) => result,
            Err(error) => {
                item.done = true;
                item.state.status = ExecutionStatus::Failed;
                item.state.error = Some(error);
                continue;
            }
        };
        let StepResult {
            status,
            events: branch_events,
        } = result;
        merge_branch_events(state, branch_events, events);
        match status {
            StepStatus::Continue => item.sleep_until_ms = 0,
            StepStatus::Retry { delay_ms } => {
                item.sleep_until_ms = now_ms().saturating_add(delay_ms);
            }
            StepStatus::WaitingApproval | StepStatus::Succeeded | StepStatus::Failed => {
                item.done = true;
                item.sleep_until_ms = 0;
            }
        }
        if item.state.status.is_terminal() {
            item.done = true;
        }
    }

    if frame.fail_fast
        && frame.concurrent_states.iter().any(|item| {
            item.done && !item.cancelled && item.state.status != ExecutionStatus::Succeeded
        })
    {
        for item in &mut frame.concurrent_states {
            if !item.done {
                item.done = true;
                item.cancelled = true;
                item.sleep_until_ms = 0;
            }
        }
    }

    let remaining = frame
        .concurrent_states
        .iter()
        .any(|item| !item.done && !item.cancelled);
    if remaining {
        let now = now_ms();
        let wake = frame
            .concurrent_states
            .iter()
            .filter(|item| !item.done && !item.cancelled && item.sleep_until_ms > now)
            .map(|item| item.sleep_until_ms)
            .min();
        let runnable = frame
            .concurrent_states
            .iter()
            .any(|item| !item.done && !item.cancelled && item.sleep_until_ms <= now);
        replace_top(state, Frame::Parallel(frame));
        return StepResult {
            status: if runnable {
                StepStatus::Continue
            } else {
                StepStatus::Retry {
                    delay_ms: wake.map(|at| at.saturating_sub(now)).unwrap_or(1).max(1),
                }
            },
            events: std::mem::take(events),
        };
    }

    let mut outputs = BTreeMap::new();
    let mut errors = Vec::new();
    let mut first_error = None;
    for item in &frame.concurrent_states {
        if item.cancelled {
            continue;
        }
        if item.state.status == ExecutionStatus::Succeeded {
            outputs.insert(
                item.id.clone(),
                item.state.output.clone().unwrap_or(Value::Null),
            );
        } else {
            let error = branch_error(&item.state, &item.id);
            errors.push(json!({"branchId": item.id, "error": error}));
            if first_error.is_none() {
                first_error = Some(error);
            }
        }
    }
    for item in std::mem::take(&mut frame.concurrent_states) {
        if item.cancelled {
            continue;
        }
        state.node_outputs.extend(item.state.node_outputs);
    }
    let join = frame.join.clone();
    let successes = outputs.len();
    let success = match join.as_str() {
        "any" => successes > 0,
        "n_of" => successes >= frame.n_of,
        _ => errors.is_empty() && successes == frame.branches.len(),
    };
    let has_errors = !errors.is_empty();
    let output = json!({"branches": outputs, "errors": errors, "join": join});
    state.frames.pop();
    if success {
        let port = if has_errors { "partial" } else { "out" };
        let status = finish_container_success(state, plan, &frame.node_id, output, &[port], events);
        return StepResult {
            status,
            events: std::mem::take(events),
        };
    }
    let error = first_error.unwrap_or_else(|| {
        ExecutionError::new("PARALLEL_JOIN", format!("parallel join `{join}` failed"))
    });
    let status = finish_container_failure(state, plan, &frame.node_id, error, events);
    StepResult {
        status,
        events: std::mem::take(events),
    }
}

fn branch_error(state: &ExecutionState, branch_id: &str) -> ExecutionError {
    if state.status == ExecutionStatus::WaitingApproval {
        return ExecutionError::new(
            "PARALLEL_APPROVAL",
            format!("parallel branch `{branch_id}` is waiting for approval"),
        );
    }
    state.error.clone().unwrap_or_else(|| {
        ExecutionError::new(
            "PARALLEL_BRANCH",
            format!("parallel branch `{branch_id}` did not succeed"),
        )
    })
}

fn new_branch_state(
    workflow: &Workflow,
    plan: &ExecutionPlan,
    parent_state: &ExecutionState,
    parent: &GraphFrame,
    branch: &ParallelBranchState,
) -> Result<ExecutionState, ExecutionError> {
    let graph = plan.graph(&branch.graph_id).ok_or_else(|| {
        ExecutionError::new(
            "PLAN_MISSING_GRAPH",
            format!("parallel branch graph `{}` is missing", branch.graph_id),
        )
    })?;
    let frame = GraphFrame::new(
        graph,
        GraphKind::ParallelBranch,
        parent.input.clone(),
        parent.vars.clone(),
        parent.loop_context.clone(),
        Some(branch.id.clone()),
    );
    Ok(ExecutionState {
        id: format!("{}:{}", parent_state.id, branch.id),
        workflow_id: workflow.id.clone(),
        workflow_version: workflow.workflow_version.clone(),
        registry_version: plan.registry_version.clone(),
        status: ExecutionStatus::Running,
        input: parent.input.clone(),
        variables: parent.vars.clone(),
        node_outputs: BTreeMap::new(),
        frames: vec![Frame::Graph(frame)],
        waiting: None,
        output: None,
        error: None,
        pending_error: None,
        error_graph_ran: false,
        started_at: parent_state.now.clone(),
        updated_at: parent_state.now.clone(),
        now: parent_state.now.clone(),
        started_ms: now_ms(),
        sequence: 0,
        step_count: 0,
        tenant_id: parent_state.tenant_id.clone(),
        env: parent_state.env.clone(),
    })
}

fn merge_branch_events(
    parent: &mut ExecutionState,
    branch_events: Vec<ExecutionEvent>,
    out: &mut Vec<ExecutionEvent>,
) {
    for mut event in branch_events {
        if event.kind.starts_with("execution.") {
            continue;
        }
        parent.sequence = parent.sequence.saturating_add(1);
        event.sequence = parent.sequence;
        event.execution_id = parent.id.clone();
        out.push(event);
    }
}
