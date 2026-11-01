use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use hpa_core::compiler::compile_workflow;
use hpa_core::ir::Workflow;
use hpa_core::runtime::{new_state, start_event};
use hpa_skill::SkillManifest;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::connector::ConnectorRuntime;
use crate::db::{
    AgentApprovalRow, AgentEventRow, AgentSessionRow, AgentSummaryRow, AgentVersionRow, Db,
    DbError, ExecutionRow,
};
use crate::driver::RuntimeManager;
use crate::mcp_client::{self, ExternalServer};
use crate::skills::{SkillService, SkillServiceError};

pub const AGENT_SCHEMA: &str = "hpa.agent/v1";
pub const AGENT_SESSION_SCHEMA: &str = "hpa.agent.session/v1";
pub const APPROVAL_TTL_SECONDS: i64 = 600;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentDefinition {
    pub schema_version: String,
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    pub planner: PlannerConfig,
    #[serde(default)]
    pub limits: AgentLimits,
    #[serde(default)]
    pub approval: Vec<ApprovalRule>,
    #[serde(default)]
    pub sub_agents: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum PlannerConfig {
    Deterministic {
        #[serde(default)]
        plan: Vec<PlanStep>,
    },
    #[serde(rename_all = "camelCase")]
    OpenAi {
        endpoint: String,
        model: String,
        #[serde(default)]
        token_env: Option<String>,
        #[serde(default)]
        temperature: Option<f64>,
        #[serde(default)]
        input_cost_per_1k: Option<f64>,
        #[serde(default)]
        output_cost_per_1k: Option<f64>,
        #[serde(default)]
        network: Vec<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum PlanStep {
    Tool {
        tool: String,
        #[serde(default)]
        arguments: Value,
    },
    Respond {
        #[serde(default)]
        output: Option<Value>,
    },
    #[serde(rename_all = "camelCase")]
    Handoff {
        agent_id: String,
        #[serde(default)]
        input: Option<Value>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentLimits {
    #[serde(default = "default_max_steps")]
    pub max_steps: u32,
    #[serde(default = "default_max_tool_calls")]
    pub max_tool_calls: u32,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u64,
    #[serde(default = "default_max_cost")]
    pub max_cost_usd: f64,
    #[serde(default = "default_max_handoffs")]
    pub max_handoffs: u32,
    #[serde(default = "default_max_depth")]
    pub max_depth: u32,
}

impl Default for AgentLimits {
    fn default() -> Self {
        Self {
            max_steps: default_max_steps(),
            max_tool_calls: default_max_tool_calls(),
            timeout_ms: default_timeout_ms(),
            max_tokens: default_max_tokens(),
            max_cost_usd: default_max_cost(),
            max_handoffs: default_max_handoffs(),
            max_depth: default_max_depth(),
        }
    }
}

fn default_max_steps() -> u32 {
    12
}
fn default_max_tool_calls() -> u32 {
    6
}
fn default_timeout_ms() -> u64 {
    60_000
}
fn default_max_tokens() -> u64 {
    8_000
}
fn default_max_cost() -> f64 {
    0.25
}
fn default_max_handoffs() -> u32 {
    4
}
fn default_max_depth() -> u32 {
    2
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalRule {
    pub tool: String,
    pub mode: ApprovalMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalMode {
    Always,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Running,
    Interrupted,
    WaitingApproval,
    Succeeded,
    Failed,
    BudgetExhausted,
    Timeout,
    Cancelled,
}

impl AgentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Interrupted => "interrupted",
            Self::WaitingApproval => "waiting_approval",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::BudgetExhausted => "budget_exhausted",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded
                | Self::Failed
                | Self::BudgetExhausted
                | Self::Timeout
                | Self::Cancelled
        )
    }

    pub fn is_resumable(self) -> bool {
        matches!(self, Self::Running | Self::Interrupted)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionState {
    pub schema_version: String,
    pub agent_id: String,
    pub agent_version: String,
    pub status: AgentStatus,
    pub input: Value,
    pub plan_index: usize,
    pub plan_history: Vec<String>,
    pub steps: u32,
    pub tool_calls: u32,
    pub handoffs: u32,
    pub depth: u32,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost_usd: f64,
    pub messages: Vec<Value>,
    pub last_output: Value,
    pub output: Option<Value>,
    pub pending_action: Option<PlanStep>,
    pub pending_approval: Option<String>,
    pub pending_tool_call_id: Option<String>,
    pub child_session_id: Option<String>,
    pub stop_reason: Option<String>,
    pub error: Option<String>,
    pub started_ms: u64,
    pub updated_at: String,
    pub event_sequence: i64,
}

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("{0}")]
    Db(#[from] DbError),
    #[error("{0}")]
    Skill(#[from] SkillServiceError),
    #[error("{0}")]
    Connector(#[from] crate::connector::ConnectorError),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Tool(String),
    #[error("{0}")]
    Model(String),
    #[error("{0}")]
    Approval(String),
    #[error("{0}")]
    Budget(String),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Clone)]
pub struct AgentRuntime {
    pub db: Db,
    pub skills: SkillService,
    pub workflow_runtime: RuntimeManager,
    pub connectors: ConnectorRuntime,
    http: reqwest::Client,
}

struct ToolOutcome {
    value: Value,
    is_error: bool,
}

pub struct AgentStart {
    pub agent_id: String,
    pub version: Option<String>,
    pub input: Value,
    pub parent_session_id: Option<String>,
    pub depth: u32,
    pub limit_override: Option<AgentLimits>,
}

impl AgentRuntime {
    pub fn new(
        skills: SkillService,
        workflow_runtime: RuntimeManager,
        connectors: ConnectorRuntime,
    ) -> Self {
        Self {
            db: skills.db.clone(),
            skills,
            workflow_runtime,
            connectors,
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .timeout(Duration::from_secs(60))
                .build()
                .unwrap_or_default(),
        }
    }

    pub async fn latest_published(&self, agent_id: &str) -> Result<AgentVersionRow, AgentError> {
        self.db
            .latest_published_agent_version(agent_id)
            .await?
            .ok_or_else(|| {
                AgentError::NotFound(format!("agent `{agent_id}` has no published version"))
            })
    }

    pub async fn start(&self, start: AgentStart) -> Result<AgentSessionRow, AgentError> {
        let version_row = match start.version.as_deref() {
            Some(version) => self
                .db
                .get_agent_version(&start.agent_id, version)
                .await?
                .filter(|row| row.status == "published")
                .ok_or_else(|| {
                    AgentError::NotFound(format!(
                        "agent `{}@{}` is not published",
                        start.agent_id, version
                    ))
                })?,
            None => self.latest_published(&start.agent_id).await?,
        };
        let mut definition = parse_definition(&version_row)?;
        if let Some(limits) = start.limit_override {
            definition.limits = inherited_limits(&definition.limits, &limits);
        }
        if start.depth > definition.limits.max_depth {
            return Err(AgentError::Budget(
                "agent depth exceeds the inherited maxDepth".to_string(),
            ));
        }
        let session_id = format!("agentsess_{}", Uuid::now_v7());
        let now = now_rfc3339();
        let mut state = AgentSessionState {
            schema_version: AGENT_SESSION_SCHEMA.to_string(),
            agent_id: definition.id.clone(),
            agent_version: definition.version.clone(),
            status: AgentStatus::Running,
            input: start.input.clone(),
            plan_index: 0,
            plan_history: Vec::new(),
            steps: 0,
            tool_calls: 0,
            handoffs: 0,
            depth: start.depth,
            tokens_in: 0,
            tokens_out: 0,
            cost_usd: 0.0,
            messages: Vec::new(),
            last_output: Value::Null,
            output: None,
            pending_action: None,
            pending_approval: None,
            pending_tool_call_id: None,
            child_session_id: None,
            stop_reason: None,
            error: None,
            started_ms: now_ms(),
            updated_at: now.clone(),
            event_sequence: 0,
        };
        prepare_messages(&definition, &mut state);
        let row = AgentSessionRow {
            id: session_id.clone(),
            agent_id: definition.id.clone(),
            agent_version: definition.version.clone(),
            status: AgentStatus::Running.as_str().to_string(),
            input_json: serde_json::to_string(&state.input)?,
            output_json: None,
            state_json: serde_json::to_string(&state)?,
            error_json: None,
            parent_session_id: start.parent_session_id.clone(),
            depth: start.depth as i64,
            created_at: now.clone(),
            updated_at: now,
            finished_at: None,
        };
        self.db.insert_agent_session(&row).await?;
        self.emit(
            &mut state,
            &session_id,
            "agent.started",
            json!({"agentId": definition.id, "version": definition.version, "depth": start.depth}),
        )
        .await?;
        self.persist(&session_id, &state).await?;
        self.drive_with_definition(&session_id, &definition, &version_row)
            .await
    }

    pub async fn drive(&self, session_id: &str) -> Result<AgentSessionRow, AgentError> {
        let row = self
            .db
            .get_agent_session(session_id)
            .await?
            .ok_or_else(|| {
                AgentError::NotFound(format!("agent session `{session_id}` not found"))
            })?;
        let mut state: AgentSessionState = serde_json::from_str(&row.state_json)?;
        if state.status.is_terminal() {
            return Ok(row);
        }
        if state.status == AgentStatus::WaitingApproval {
            return Ok(row);
        }
        if state.status == AgentStatus::Interrupted {
            state.status = AgentStatus::Running;
            state.stop_reason = None;
            state.error = None;
            self.emit(
                &mut state,
                session_id,
                "agent.resumed",
                json!({"from": "interrupted"}),
            )
            .await?;
            self.persist(session_id, &state).await?;
        }
        let version = self
            .db
            .get_agent_version(&row.agent_id, &row.agent_version)
            .await?
            .ok_or_else(|| {
                AgentError::NotFound(format!(
                    "agent version `{}@{}` not found",
                    row.agent_id, row.agent_version
                ))
            })?;
        let definition = parse_definition(&version)?;
        self.drive_with_definition(session_id, &definition, &version)
            .await
    }

    async fn drive_with_definition(
        &self,
        session_id: &str,
        definition: &AgentDefinition,
        version: &AgentVersionRow,
    ) -> Result<AgentSessionRow, AgentError> {
        let mut row = self
            .db
            .get_agent_session(session_id)
            .await?
            .ok_or_else(|| {
                AgentError::NotFound(format!("agent session `{session_id}` not found"))
            })?;
        let mut state: AgentSessionState = serde_json::from_str(&row.state_json)?;
        loop {
            if state.status.is_terminal() || state.status == AgentStatus::WaitingApproval {
                break;
            }
            if now_ms().saturating_sub(state.started_ms) > definition.limits.timeout_ms {
                self.finish_with_error(
                    &mut state,
                    session_id,
                    AgentStatus::Timeout,
                    "AGENT_TIMEOUT",
                    format!("agent exceeded {} ms", definition.limits.timeout_ms),
                )
                .await?;
                break;
            }
            if state.steps >= definition.limits.max_steps {
                self.finish_with_error(
                    &mut state,
                    session_id,
                    AgentStatus::BudgetExhausted,
                    "AGENT_STEPS",
                    "agent step budget exhausted".to_string(),
                )
                .await?;
                break;
            }
            let action = match self
                .next_action(definition, version, session_id, &mut state)
                .await
            {
                Ok(action) => action,
                Err(error) => {
                    self.finish_with_error(
                        &mut state,
                        session_id,
                        AgentStatus::Failed,
                        "AGENT_PLAN",
                        error.to_string(),
                    )
                    .await?;
                    break;
                }
            };
            let action = resolve_action(action, &state.input, &state.last_output);
            let action_key = serde_json::to_string(&action)?;
            state.plan_history.push(action_key);
            if matches!(definition.planner, PlannerConfig::OpenAi { .. })
                && repeated_tail(&state.plan_history, 3)
            {
                self.finish_with_error(
                    &mut state,
                    session_id,
                    AgentStatus::Failed,
                    "AGENT_LOOP",
                    "repeated model plan detected".to_string(),
                )
                .await?;
                break;
            }
            if matches!(definition.planner, PlannerConfig::OpenAi { .. }) {
                let step_number = state.steps + 1;
                self.emit(
                    &mut state,
                    session_id,
                    "agent.plan",
                    json!({"step": step_number, "action": action}),
                )
                .await?;
            }
            state.steps = state.steps.saturating_add(1);
            match action {
                PlanStep::Respond { output } => {
                    let output = output.unwrap_or_else(|| state.last_output.clone());
                    state.output = Some(output.clone());
                    state.status = AgentStatus::Succeeded;
                    state.stop_reason = Some("completed".to_string());
                    state.pending_action = None;
                    self.emit(
                        &mut state,
                        session_id,
                        "agent.finished",
                        json!({"status": "succeeded", "output": output}),
                    )
                    .await?;
                    self.persist(session_id, &state).await?;
                    break;
                }
                PlanStep::Tool { tool, arguments } => {
                    if !definition.tools.iter().any(|item| item == &tool) {
                        self.finish_with_error(
                            &mut state,
                            session_id,
                            AgentStatus::Failed,
                            "AGENT_TOOL",
                            format!("tool `{tool}` is not declared by the agent"),
                        )
                        .await?;
                        break;
                    }
                    if state.tool_calls >= definition.limits.max_tool_calls {
                        self.finish_with_error(
                            &mut state,
                            session_id,
                            AgentStatus::BudgetExhausted,
                            "AGENT_TOOL_BUDGET",
                            "agent tool-call budget exhausted".to_string(),
                        )
                        .await?;
                        break;
                    }
                    if model_budget_exhausted(definition, &state) {
                        self.finish_with_error(
                            &mut state,
                            session_id,
                            AgentStatus::BudgetExhausted,
                            "AGENT_BUDGET",
                            "agent token or cost budget exhausted".to_string(),
                        )
                        .await?;
                        break;
                    }
                    let args = arguments;
                    let risk = tool_risk(&tool);
                    if requires_approval(definition, &tool, risk) {
                        let approval = self.create_approval(session_id, &tool, &args, risk).await?;
                        state.pending_action = Some(PlanStep::Tool {
                            tool: tool.clone(),
                            arguments: args,
                        });
                        state.pending_approval = Some(approval.id.clone());
                        state.status = AgentStatus::WaitingApproval;
                        state.stop_reason = Some("approval".to_string());
                        self.emit(
                            &mut state,
                            session_id,
                            "agent.approval_requested",
                            json!({"approvalId": approval.id, "tool": tool, "risk": risk}),
                        )
                        .await?;
                        self.persist(session_id, &state).await?;
                        break;
                    }
                    let outcome = self.execute_tool(&tool, &args).await?;
                    self.apply_tool_outcome(&mut state, session_id, &tool, outcome)
                        .await?;
                    state.pending_action = None;
                    state.pending_tool_call_id = None;
                    self.persist(session_id, &state).await?;
                    if self.budget_exhausted(definition, &state) {
                        self.finish_with_error(
                            &mut state,
                            session_id,
                            AgentStatus::BudgetExhausted,
                            "AGENT_BUDGET",
                            "agent budget exhausted".to_string(),
                        )
                        .await?;
                        break;
                    }
                }
                PlanStep::Handoff { agent_id, input } => {
                    if !definition.sub_agents.iter().any(|item| item == &agent_id) {
                        self.finish_with_error(
                            &mut state,
                            session_id,
                            AgentStatus::Failed,
                            "AGENT_HANDOFF",
                            format!("agent `{agent_id}` is not declared as a sub-agent"),
                        )
                        .await?;
                        break;
                    }
                    if state.handoffs >= definition.limits.max_handoffs
                        || state.depth >= definition.limits.max_depth
                    {
                        self.finish_with_error(
                            &mut state,
                            session_id,
                            AgentStatus::BudgetExhausted,
                            "AGENT_HANDOFF_BUDGET",
                            "handoff budget or depth exhausted".to_string(),
                        )
                        .await?;
                        break;
                    }
                    let remaining = remaining_limits(definition, &state);
                    if remaining.max_steps == 0
                        || remaining.max_tool_calls == 0
                        || remaining.max_tokens == 0
                        || remaining.max_cost_usd <= 0.0
                    {
                        self.finish_with_error(
                            &mut state,
                            session_id,
                            AgentStatus::BudgetExhausted,
                            "AGENT_HANDOFF_BUDGET",
                            "parent budget exhausted before handoff".to_string(),
                        )
                        .await?;
                        break;
                    }
                    let child_input = input.unwrap_or_else(|| state.input.clone());
                    let child = Box::pin(self.start(AgentStart {
                        agent_id: agent_id.clone(),
                        version: None,
                        input: child_input,
                        parent_session_id: Some(session_id.to_string()),
                        depth: state.depth.saturating_add(1),
                        limit_override: Some(remaining),
                    }))
                    .await?;
                    state.child_session_id = Some(child.id.clone());
                    if child.status == AgentStatus::WaitingApproval.as_str() {
                        self.finish_with_error(
                            &mut state,
                            session_id,
                            AgentStatus::Failed,
                            "AGENT_CHILD_APPROVAL",
                            "handoff target is waiting for approval; the parent cannot approve it"
                                .to_string(),
                        )
                        .await?;
                        break;
                    }
                    if child.status != AgentStatus::Succeeded.as_str() {
                        self.finish_with_error(
                            &mut state,
                            session_id,
                            AgentStatus::Failed,
                            "AGENT_HANDOFF_CHILD_FAILED",
                            format!("handoff target finished with status `{}`", child.status),
                        )
                        .await?;
                        break;
                    }
                    let child_state: AgentSessionState = serde_json::from_str(&child.state_json)?;
                    state.handoffs = state.handoffs.saturating_add(1);
                    state.last_output = child_state.output.clone().unwrap_or(Value::Null);
                    state.tool_calls = state.tool_calls.saturating_add(child_state.tool_calls);
                    state.tokens_in = state.tokens_in.saturating_add(child_state.tokens_in);
                    state.tokens_out = state.tokens_out.saturating_add(child_state.tokens_out);
                    state.cost_usd += child_state.cost_usd;
                    if let Some(call_id) = state.pending_tool_call_id.take() {
                        state.messages.push(json!({
                            "role": "tool",
                            "tool_call_id": call_id,
                            "content": serde_json::to_string(&state.last_output).unwrap_or_default(),
                        }));
                    }
                    let handoff_output = state.last_output.clone();
                    self.emit(
                        &mut state,
                        session_id,
                        "agent.handoff",
                        json!({
                            "agentId": agent_id,
                            "childSessionId": child.id,
                            "status": child.status,
                            "output": handoff_output,
                        }),
                    )
                    .await?;
                    self.persist(session_id, &state).await?;
                    if self.budget_exhausted(definition, &state) {
                        self.finish_with_error(
                            &mut state,
                            session_id,
                            AgentStatus::BudgetExhausted,
                            "AGENT_BUDGET",
                            "agent budget exhausted after handoff".to_string(),
                        )
                        .await?;
                        break;
                    }
                }
            }
        }
        row = self
            .db
            .get_agent_session(session_id)
            .await?
            .ok_or_else(|| AgentError::NotFound(format!("agent session `{session_id}` missing")))?;
        Ok(row)
    }

    async fn next_action(
        &self,
        definition: &AgentDefinition,
        version: &AgentVersionRow,
        session_id: &str,
        state: &mut AgentSessionState,
    ) -> Result<PlanStep, AgentError> {
        match &definition.planner {
            PlannerConfig::Deterministic { plan } => {
                if state.plan_index < plan.len() {
                    let action = plan[state.plan_index].clone();
                    state.plan_index += 1;
                    Ok(action)
                } else {
                    Ok(PlanStep::Respond {
                        output: Some(state.last_output.clone()),
                    })
                }
            }
            PlannerConfig::OpenAi { .. } => {
                self.next_model_action(definition, version, session_id, state)
                    .await
            }
        }
    }

    async fn execute_tool(&self, tool: &str, arguments: &Value) -> Result<ToolOutcome, AgentError> {
        if let Some(rest) = tool.strip_prefix("connector.") {
            let Some((connection_id, operation)) = rest.split_once('.') else {
                return Err(AgentError::Tool(format!("invalid connector tool `{tool}`")));
            };
            let value = self
                .connectors
                .invoke(connection_id, operation, arguments.clone())
                .await
                .map_err(|error| AgentError::Tool(error.to_string()))?;
            return Ok(ToolOutcome {
                value,
                is_error: false,
            });
        }
        if let Some(rest) = tool.strip_prefix("skill.") {
            let Some((skill_name, script_id)) = rest.rsplit_once('.') else {
                return Err(AgentError::Tool(format!("invalid skill tool `{tool}`")));
            };
            let skill = self
                .db
                .list_skills()
                .await?
                .into_iter()
                .find(|skill| skill.name == skill_name)
                .ok_or_else(|| AgentError::NotFound(format!("skill `{skill_name}` not found")))?;
            let outcome = self
                .skills
                .run(&skill.id, None, Some(script_id), arguments.clone())
                .await?;
            let value = outcome
                .output
                .clone()
                .unwrap_or_else(|| json!({"stdout": outcome.stdout}));
            return Ok(ToolOutcome {
                value,
                is_error: !outcome.succeeded(),
            });
        }
        match tool {
            "workflow.execute" => {
                let workflow_id = arguments
                    .get("workflowId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| AgentError::Tool("workflowId is required".to_string()))?;
                let version = arguments
                    .get("version")
                    .and_then(Value::as_str)
                    .ok_or_else(|| AgentError::Tool("version is required".to_string()))?;
                let value = self
                    .start_workflow(workflow_id, version, arguments.get("input").cloned())
                    .await?;
                Ok(ToolOutcome {
                    value,
                    is_error: false,
                })
            }
            "workflow.list" => {
                let workflows = self
                    .db
                    .list_workflows()
                    .await?
                    .into_iter()
                    .filter(|workflow| workflow.published != 0)
                    .map(|workflow| {
                        json!({"workflowId": workflow.id, "name": workflow.name, "version": workflow.version})
                    })
                    .collect::<Vec<_>>();
                Ok(ToolOutcome {
                    value: json!({"workflows": workflows}),
                    is_error: false,
                })
            }
            "execution.get" => {
                let execution_id = arguments
                    .get("executionId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| AgentError::Tool("executionId is required".to_string()))?;
                let value = self.execution_value(execution_id).await?;
                Ok(ToolOutcome {
                    value,
                    is_error: false,
                })
            }
            "execution.wait" => {
                let execution_id = arguments
                    .get("executionId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| AgentError::Tool("executionId is required".to_string()))?;
                let timeout = arguments
                    .get("timeoutMs")
                    .and_then(Value::as_u64)
                    .unwrap_or(10_000)
                    .clamp(100, 30_000);
                let deadline = std::time::Instant::now() + Duration::from_millis(timeout);
                loop {
                    let value = self.execution_value(execution_id).await?;
                    let status = value
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    if matches!(
                        status.as_str(),
                        "succeeded" | "failed" | "cancelled" | "waiting_approval"
                    ) || std::time::Instant::now() >= deadline
                    {
                        return Ok(ToolOutcome {
                            value,
                            is_error: status == "failed",
                        });
                    }
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
            other if other.starts_with("mcp.") => {
                let rest = &other[4..];
                let Some((server_id, tool_name)) = rest.split_once('.') else {
                    return Err(AgentError::Tool(format!("invalid MCP tool `{tool}`")));
                };
                let row = self.db.get_mcp_server(server_id).await?.ok_or_else(|| {
                    AgentError::NotFound(format!("MCP server `{server_id}` not found"))
                })?;
                let server = ExternalServer::from_row(&row).map_err(AgentError::Tool)?;
                let result = mcp_client::call_tool(&server, tool_name, arguments.clone())
                    .await
                    .map_err(AgentError::Tool)?;
                let is_error = result.get("isError") == Some(&json!(true));
                let value = result.get("structuredContent").cloned().unwrap_or(result);
                Ok(ToolOutcome { value, is_error })
            }
            _ => Err(AgentError::Tool(format!("unknown tool `{tool}`"))),
        }
    }

    async fn apply_tool_outcome(
        &self,
        state: &mut AgentSessionState,
        session_id: &str,
        tool: &str,
        outcome: ToolOutcome,
    ) -> Result<(), AgentError> {
        state.tool_calls = state.tool_calls.saturating_add(1);
        state.last_output = outcome.value.clone();
        if let Some(call_id) = state.pending_tool_call_id.take() {
            state.messages.push(json!({
                "role": "tool",
                "tool_call_id": call_id,
                "content": serde_json::to_string(&outcome.value).unwrap_or_default(),
            }));
        }
        self.emit(
            state,
            session_id,
            if outcome.is_error {
                "agent.tool_failed"
            } else {
                "agent.tool_completed"
            },
            json!({"tool": tool, "output": outcome.value}),
        )
        .await?;
        Ok(())
    }

    async fn execution_value(&self, execution_id: &str) -> Result<Value, AgentError> {
        let row =
            self.db.get_execution(execution_id).await?.ok_or_else(|| {
                AgentError::NotFound(format!("execution `{execution_id}` not found"))
            })?;
        let state = crate::db::parse_state(&row)?;
        Ok(json!({
            "executionId": state.id,
            "workflowId": state.workflow_id,
            "workflowVersion": state.workflow_version,
            "status": state.status,
            "waiting": state.waiting,
            "output": state.output,
            "error": state.error,
        }))
    }

    async fn start_workflow(
        &self,
        workflow_id: &str,
        version: &str,
        input: Option<Value>,
    ) -> Result<Value, AgentError> {
        let version_row = self
            .db
            .get_version(workflow_id, version)
            .await?
            .ok_or_else(|| {
                AgentError::NotFound(format!(
                    "workflow `{workflow_id}@{version}` is not published"
                ))
            })?;
        let workflow: Workflow = serde_json::from_str(&version_row.ir_json)?;
        let compilation = compile_workflow(&workflow).map_err(|issues| {
            AgentError::Tool(serde_json::to_string(&issues).unwrap_or_default())
        })?;
        let execution_id = format!("exec_{}", Uuid::now_v7());
        let mut execution = new_state(
            &workflow,
            &compilation.plan,
            &execution_id,
            input.unwrap_or_else(|| json!({})),
            BTreeMap::new(),
            "local",
        )
        .map_err(|error| AgentError::Tool(error.to_string()))?;
        let event = start_event(&mut execution);
        let row = ExecutionRow {
            id: execution.id.clone(),
            workflow_id: execution.workflow_id.clone(),
            workflow_version: execution.workflow_version.clone(),
            status: "running".to_string(),
            state_json: serde_json::to_string(&execution)?,
            workflow_json: version_row.ir_json,
            output_json: None,
            error_json: None,
            started_at: execution.started_at.clone(),
            updated_at: execution.updated_at.clone(),
        };
        self.db.insert_execution(&row, &event).await?;
        self.workflow_runtime.trigger(execution_id.clone());
        Ok(json!({"executionId": execution_id, "status": "running"}))
    }

    async fn create_approval(
        &self,
        session_id: &str,
        tool: &str,
        arguments: &Value,
        risk: &str,
    ) -> Result<AgentApprovalRow, AgentError> {
        let now = now_rfc3339();
        let expires = (OffsetDateTime::now_utc() + time::Duration::seconds(APPROVAL_TTL_SECONDS))
            .format(&Rfc3339)
            .unwrap_or_default();
        Ok(self
            .db
            .create_agent_approval(
                session_id,
                tool,
                &serde_json::to_string(arguments)?,
                risk,
                &now,
                &expires,
            )
            .await?)
    }

    pub async fn resume_approval(
        &self,
        session_id: &str,
        decision: &str,
    ) -> Result<AgentSessionRow, AgentError> {
        if !matches!(decision, "approved" | "rejected") {
            return Err(AgentError::Approval(
                "decision must be approved or rejected".to_string(),
            ));
        }
        let _ = self.db.expire_agent_approvals(&now_rfc3339()).await;
        let row = self
            .db
            .get_agent_session(session_id)
            .await?
            .ok_or_else(|| {
                AgentError::NotFound(format!("agent session `{session_id}` not found"))
            })?;
        let mut state: AgentSessionState = serde_json::from_str(&row.state_json)?;
        if state.status != AgentStatus::WaitingApproval {
            return Err(AgentError::Approval(
                "agent session is not waiting for approval".to_string(),
            ));
        }
        let approval_id = state
            .pending_approval
            .clone()
            .ok_or_else(|| AgentError::Approval("approval reference is missing".to_string()))?;
        let approval = self
            .db
            .get_agent_approval(&approval_id)
            .await?
            .ok_or_else(|| AgentError::Approval(format!("approval `{approval_id}` not found")))?;
        if approval.status == "rejected" {
            self.finish_with_error(
                &mut state,
                session_id,
                AgentStatus::Failed,
                "APPROVAL_REJECTED",
                "approval was rejected".to_string(),
            )
            .await?;
            return self
                .db
                .get_agent_session(session_id)
                .await?
                .ok_or_else(|| AgentError::NotFound(session_id.to_string()));
        }
        if approval.status != "approved" {
            return Err(AgentError::Approval(format!(
                "approval is `{}`",
                approval.status
            )));
        }
        if !self
            .db
            .consume_agent_approval(&approval_id, &now_rfc3339())
            .await?
        {
            return Err(AgentError::Approval(
                "approval was already consumed".to_string(),
            ));
        }
        let version = self
            .db
            .get_agent_version(&row.agent_id, &row.agent_version)
            .await?
            .ok_or_else(|| {
                AgentError::NotFound(format!(
                    "agent version `{}@{}` missing",
                    row.agent_id, row.agent_version
                ))
            })?;
        let definition = parse_definition(&version)?;
        let action = state
            .pending_action
            .clone()
            .ok_or_else(|| AgentError::Approval("pending tool action is missing".to_string()))?;
        let PlanStep::Tool { tool, arguments } = action else {
            return Err(AgentError::Approval(
                "pending action is not a tool call".to_string(),
            ));
        };
        let outcome = self.execute_tool(&tool, &arguments).await?;
        self.apply_tool_outcome(&mut state, session_id, &tool, outcome)
            .await?;
        state.pending_action = None;
        state.pending_approval = None;
        state.pending_tool_call_id = None;
        state.status = AgentStatus::Running;
        state.stop_reason = None;
        self.emit(
            &mut state,
            session_id,
            "agent.approval_resumed",
            json!({"approvalId": approval_id, "decision": decision, "tool": tool}),
        )
        .await?;
        self.persist(session_id, &state).await?;
        self.drive_with_definition(session_id, &definition, &version)
            .await
    }

    pub async fn cancel(&self, session_id: &str) -> Result<AgentSessionRow, AgentError> {
        let row = self
            .db
            .get_agent_session(session_id)
            .await?
            .ok_or_else(|| {
                AgentError::NotFound(format!("agent session `{session_id}` not found"))
            })?;
        let mut state: AgentSessionState = serde_json::from_str(&row.state_json)?;
        if state.status.is_terminal() {
            return Ok(row);
        }
        if let Some(approval_id) = state.pending_approval.take() {
            let _ = self
                .db
                .decide_agent_approval(&approval_id, "cancelled", &now_rfc3339())
                .await;
        }
        state.status = AgentStatus::Cancelled;
        state.stop_reason = Some("cancelled".to_string());
        self.emit(&mut state, session_id, "agent.cancelled", json!({}))
            .await?;
        self.persist(session_id, &state).await?;
        self.db
            .get_agent_session(session_id)
            .await?
            .ok_or_else(|| AgentError::NotFound(session_id.to_string()))
    }

    async fn persist(&self, session_id: &str, state: &AgentSessionState) -> Result<(), AgentError> {
        let row = self
            .db
            .get_agent_session(session_id)
            .await?
            .ok_or_else(|| AgentError::NotFound(session_id.to_string()))?;
        let now = now_rfc3339();
        let mut snapshot = state.clone();
        snapshot.updated_at = now.clone();
        let updated = AgentSessionRow {
            id: row.id,
            agent_id: row.agent_id,
            agent_version: row.agent_version,
            status: state.status.as_str().to_string(),
            input_json: serde_json::to_string(&state.input)?,
            output_json: state
                .output
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?,
            state_json: serde_json::to_string(&snapshot)?,
            error_json: state
                .error
                .as_ref()
                .map(|error| serde_json::to_string(&json!({"message": error})))
                .transpose()?,
            parent_session_id: row.parent_session_id,
            depth: row.depth,
            created_at: row.created_at,
            updated_at: now.clone(),
            finished_at: state.status.is_terminal().then_some(now),
        };
        self.db.update_agent_session(&updated).await?;
        Ok(())
    }

    async fn emit(
        &self,
        state: &mut AgentSessionState,
        session_id: &str,
        kind: &str,
        data: Value,
    ) -> Result<(), AgentError> {
        state.event_sequence += 1;
        self.db
            .append_agent_events(&[AgentEventRow {
                session_id: session_id.to_string(),
                sequence: state.event_sequence,
                at: now_rfc3339(),
                kind: kind.to_string(),
                data_json: serde_json::to_string(&data)?,
            }])
            .await?;
        Ok(())
    }

    async fn finish_with_error(
        &self,
        state: &mut AgentSessionState,
        session_id: &str,
        status: AgentStatus,
        code: &str,
        message: String,
    ) -> Result<(), AgentError> {
        state.status = status;
        state.error = Some(format!("{code}: {message}"));
        state.stop_reason = Some(code.to_string());
        state.pending_action = None;
        state.pending_approval = None;
        state.pending_tool_call_id = None;
        self.emit(
            state,
            session_id,
            "agent.finished",
            json!({"status": status.as_str(), "code": code, "message": message}),
        )
        .await?;
        self.persist(session_id, state).await?;
        Ok(())
    }

    fn budget_exhausted(&self, definition: &AgentDefinition, state: &AgentSessionState) -> bool {
        state.tool_calls > definition.limits.max_tool_calls
            || model_budget_exhausted(definition, state)
    }

    pub async fn recover(&self) -> Result<usize, AgentError> {
        let sessions = self.db.running_agent_sessions().await?;
        let mut recovered = 0;
        for row in sessions {
            let mut state: AgentSessionState = serde_json::from_str(&row.state_json)?;
            if !state.status.is_resumable() {
                continue;
            }
            state.status = AgentStatus::Interrupted;
            state.stop_reason = Some("runtime_restart".to_string());
            state.error = Some(
                "AGENT_INTERRUPTED: runtime restarted; resume to continue from the last checkpoint"
                    .to_string(),
            );
            self.emit(
                &mut state,
                &row.id,
                "agent.interrupted",
                json!({"reason": "runtime_restart"}),
            )
            .await?;
            self.persist(&row.id, &state).await?;
            recovered += 1;
        }
        Ok(recovered)
    }

    pub fn start_approval_monitor(&self) {
        let runtime = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let _ = runtime.expire_waiting_approvals().await;
            }
        });
    }

    async fn expire_waiting_approvals(&self) -> Result<(), AgentError> {
        let now = now_rfc3339();
        let expired = self.db.expire_agent_approvals(&now).await?;
        if expired == 0 {
            return Ok(());
        }
        let sessions = self
            .db
            .list_agent_sessions(None, 200)
            .await?
            .into_iter()
            .filter(|row| row.status == AgentStatus::WaitingApproval.as_str())
            .collect::<Vec<_>>();
        for row in sessions {
            let state = session_state(&row)?;
            let Some(approval_id) = state.pending_approval.clone() else {
                continue;
            };
            let Some(approval) = self.db.get_agent_approval(&approval_id).await? else {
                continue;
            };
            if approval.status != "expired" {
                continue;
            }
            let mut state = state;
            self.finish_with_error(
                &mut state,
                &row.id,
                AgentStatus::Failed,
                "APPROVAL_EXPIRED",
                format!("approval `{approval_id}` expired"),
            )
            .await?;
        }
        Ok(())
    }

    pub async fn replay(&self, session_id: &str) -> Result<Value, AgentError> {
        let row = self
            .db
            .get_agent_session(session_id)
            .await?
            .ok_or_else(|| {
                AgentError::NotFound(format!("agent session `{session_id}` not found"))
            })?;
        let original = session_state(&row)?;
        let version = self
            .db
            .get_agent_version(&row.agent_id, &row.agent_version)
            .await?
            .ok_or_else(|| {
                AgentError::NotFound(format!(
                    "agent version `{}@{}` missing",
                    row.agent_id, row.agent_version
                ))
            })?;
        let definition = parse_definition(&version)?;
        let PlannerConfig::Deterministic { plan } = &definition.planner else {
            return Ok(json!({
                "deterministic": false,
                "replayMode": "unsupported",
                "matches": false,
                "originalSessionId": row.id,
                "originalStatus": original.status,
                "reason": "model planner sessions are recorded for audit but cannot be replayed deterministically",
            }));
        };
        if original.status != AgentStatus::Succeeded {
            return Ok(json!({
                "deterministic": true,
                "replayMode": "trace",
                "matches": false,
                "originalSessionId": row.id,
                "originalStatus": original.status,
                "reason": "only succeeded sessions can be replayed from trace",
            }));
        }
        let events = self.db.list_agent_events(session_id, 1_000).await?;
        let mut outputs = Vec::new();
        for event in &events {
            if matches!(
                event.kind.as_str(),
                "agent.tool_completed" | "agent.tool_failed" | "agent.handoff"
            ) {
                let data: Value = serde_json::from_str(&event.data_json)?;
                outputs.push(data.get("output").cloned().unwrap_or(Value::Null));
            }
        }
        let mut diffs = Vec::new();
        let mut last_output = Value::Null;
        let mut replay_output = None;
        let mut output_index = 0;
        let mut plan_index = 0;
        let mut simulated = 0;
        for recorded_text in &original.plan_history {
            let step = if plan_index < plan.len() {
                let step = plan[plan_index].clone();
                plan_index += 1;
                step
            } else {
                PlanStep::Respond {
                    output: Some(last_output.clone()),
                }
            };
            let resolved = resolve_action(step, &original.input, &last_output);
            simulated += 1;
            let resolved_text = serde_json::to_string(&resolved)?;
            if &resolved_text != recorded_text {
                diffs.push(json!({
                    "step": simulated,
                    "expected": recorded_text,
                    "replayed": resolved_text,
                }));
            }
            match resolved {
                PlanStep::Tool { .. } | PlanStep::Handoff { .. } => {
                    last_output = outputs.get(output_index).cloned().unwrap_or(Value::Null);
                    output_index += 1;
                }
                PlanStep::Respond { output } => {
                    replay_output = Some(output.unwrap_or(Value::Null));
                    break;
                }
            }
        }
        let matches = diffs.is_empty() && replay_output == original.output;
        Ok(json!({
            "deterministic": true,
            "replayMode": "trace",
            "matches": matches,
            "originalSessionId": row.id,
            "originalStatus": original.status,
            "replayStatus": AgentStatus::Succeeded,
            "originalOutput": original.output,
            "replayOutput": replay_output,
            "actions": simulated,
            "toolOutputs": output_index,
            "diffs": diffs,
        }))
    }

    async fn next_model_action(
        &self,
        definition: &AgentDefinition,
        version: &AgentVersionRow,
        session_id: &str,
        state: &mut AgentSessionState,
    ) -> Result<PlanStep, AgentError> {
        let PlannerConfig::OpenAi {
            endpoint,
            model,
            token_env,
            temperature,
            input_cost_per_1k,
            output_cost_per_1k,
            network,
        } = &definition.planner
        else {
            return Err(AgentError::Invalid(
                "agent planner is not OpenAI".to_string(),
            ));
        };
        validate_model_endpoint(endpoint, network).await?;
        let tools = self.tool_schemas(definition, version).await?;
        let mut body = json!({
            "model": model,
            "messages": state.messages,
            "temperature": temperature.unwrap_or(0.0),
        });
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools);
            body["tool_choice"] = json!("auto");
        }
        let mut request = self
            .http
            .post(endpoint)
            .header("content-type", "application/json")
            .json(&body);
        if let Some(name) = token_env
            && let Ok(token) = std::env::var(name)
        {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .await
            .map_err(|error| AgentError::Model(format!("model request failed: {error}")))?;
        let status = response.status();
        let body = response
            .json::<Value>()
            .await
            .map_err(|error| AgentError::Model(format!("invalid model response: {error}")))?;
        if !status.is_success() {
            return Err(AgentError::Model(format!(
                "model endpoint returned HTTP {status}: {}",
                body.get("error").cloned().unwrap_or(Value::Null)
            )));
        }
        let usage = body.get("usage").cloned().unwrap_or_else(|| json!({}));
        let input_tokens = usage
            .get("prompt_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let output_tokens = usage
            .get("completion_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        state.tokens_in = state.tokens_in.saturating_add(input_tokens);
        state.tokens_out = state.tokens_out.saturating_add(output_tokens);
        state.cost_usd += input_tokens as f64 * input_cost_per_1k.unwrap_or(0.0) / 1000.0
            + output_tokens as f64 * output_cost_per_1k.unwrap_or(0.0) / 1000.0;
        self.emit(
            state,
            session_id,
            "agent.model_usage",
            json!({
                "model": model,
                "inputTokens": input_tokens,
                "outputTokens": output_tokens,
                "costUsd": state.cost_usd,
            }),
        )
        .await?;
        let Some(message) = body.pointer("/choices/0/message") else {
            return Err(AgentError::Model(
                "model response has no message".to_string(),
            ));
        };
        if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array)
            && tool_calls.len() > 1
        {
            return Err(AgentError::Model(
                "only one tool call per model turn is supported".to_string(),
            ));
        }
        if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array)
            && let Some(call) = tool_calls.first()
        {
            let name = call
                .pointer("/function/name")
                .and_then(Value::as_str)
                .ok_or_else(|| AgentError::Model("tool call has no name".to_string()))?;
            let raw_arguments = call
                .pointer("/function/arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let arguments = match raw_arguments {
                Value::String(text) => serde_json::from_str::<Value>(&text).map_err(|error| {
                    AgentError::Model(format!("invalid tool arguments: {error}"))
                })?,
                other => other,
            };
            let call_id = call
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("call_{}", Uuid::now_v7()));
            state.messages.push(message.clone());
            if name == "agent.handoff" {
                let agent_id = arguments
                    .get("agentId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| AgentError::Model("handoff requires agentId".to_string()))?;
                let input = arguments.get("input").cloned();
                state.pending_tool_call_id = Some(call_id);
                return Ok(PlanStep::Handoff {
                    agent_id: agent_id.to_string(),
                    input,
                });
            }
            state.pending_tool_call_id = Some(call_id);
            return Ok(PlanStep::Tool {
                tool: name.to_string(),
                arguments,
            });
        }
        let content = message
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        state
            .messages
            .push(json!({"role": "assistant", "content": content}));
        let output = serde_json::from_str::<Value>(&content).unwrap_or(Value::String(content));
        Ok(PlanStep::Respond {
            output: Some(output),
        })
    }

    pub async fn tool_catalog(&self) -> Result<Value, AgentError> {
        let mut skills = Vec::new();
        for skill in self.db.list_skills().await? {
            if skill.status != "published" {
                continue;
            }
            let Some(version) = self
                .db
                .get_skill_version(&skill.id, &skill.latest_version)
                .await?
            else {
                continue;
            };
            let manifest: Option<SkillManifest> = serde_json::from_str(&version.manifest_json).ok();
            for script in manifest.iter().flat_map(|manifest| manifest.scripts.iter()) {
                let tool = format!("skill.{}.{}", skill.name, script.id);
                skills.push(json!({
                    "tool": tool,
                    "risk": tool_risk(&tool),
                    "kind": "skill",
                    "label": format!("{} / {}", skill.name, script.id),
                    "skillId": skill.id,
                    "skillVersion": skill.latest_version,
                    "runtime": script.runtime,
                    "inputSchema": script.input_schema,
                }));
            }
        }
        let workflows = self
            .db
            .list_workflows()
            .await?
            .into_iter()
            .filter(|workflow| workflow.published != 0)
            .map(|workflow| {
                json!({
                    "tool": "workflow.execute",
                    "risk": "high",
                    "kind": "workflow",
                    "label": workflow.name,
                    "workflowId": workflow.id,
                    "workflowVersion": workflow.version,
                })
            })
            .collect::<Vec<_>>();
        let mcp_servers = self
            .db
            .list_mcp_servers()
            .await?
            .into_iter()
            .map(|server| {
                let allowed: Value =
                    serde_json::from_str(&server.allowed_tools_json).unwrap_or_else(|_| json!([]));
                json!({
                    "tool": format!("mcp.{}.{{tool}}", server.id),
                    "risk": "high",
                    "kind": "mcp",
                    "label": server.name,
                    "serverId": server.id,
                    "transport": server.transport,
                    "allowedTools": allowed,
                })
            })
            .collect::<Vec<_>>();
        let connector_tools = self.connectors.tool_catalog().await?;
        Ok(json!({
            "builtIn": [
                {"tool": "workflow.list", "risk": "low", "kind": "builtin"},
                {"tool": "workflow.execute", "risk": "high", "kind": "builtin"},
                {"tool": "execution.get", "risk": "low", "kind": "builtin"},
                {"tool": "execution.wait", "risk": "low", "kind": "builtin"}
            ],
            "skills": skills,
            "workflows": workflows,
            "mcpServers": mcp_servers,
            "connectors": connector_tools,
        }))
    }

    async fn tool_schemas(
        &self,
        definition: &AgentDefinition,
        _version: &AgentVersionRow,
    ) -> Result<Vec<Value>, AgentError> {
        let mut tools = Vec::new();
        for tool in &definition.tools {
            let schema = if let Some(rest) = tool.strip_prefix("skill.") {
                let Some((skill_name, script_id)) = rest.rsplit_once('.') else {
                    continue;
                };
                let skill = self
                    .db
                    .list_skills()
                    .await?
                    .into_iter()
                    .find(|skill| skill.name == skill_name);
                let input = if let Some(skill) = skill
                    && let Some(version) = self
                        .db
                        .get_skill_version(&skill.id, &skill.latest_version)
                        .await?
                {
                    serde_json::from_str::<SkillManifest>(&version.manifest_json)
                        .ok()
                        .and_then(|manifest| {
                            manifest
                                .scripts
                                .iter()
                                .find(|script| script.id == script_id)
                                .and_then(|script| script.input_schema.clone())
                        })
                } else {
                    None
                };
                input.unwrap_or_else(|| json!({"type": "object", "additionalProperties": true}))
            } else {
                match tool.as_str() {
                    "workflow.execute" => json!({
                        "type": "object",
                        "properties": {
                            "workflowId": {"type": "string"},
                            "version": {"type": "string"},
                            "input": {"type": "object"}
                        },
                        "required": ["workflowId", "version"]
                    }),
                    "workflow.list" => json!({"type": "object", "properties": {}}),
                    "execution.get" => json!({
                        "type": "object",
                        "properties": {"executionId": {"type": "string"}},
                        "required": ["executionId"]
                    }),
                    "execution.wait" => json!({
                        "type": "object",
                        "properties": {
                            "executionId": {"type": "string"},
                            "timeoutMs": {"type": "integer", "minimum": 100, "maximum": 30000}
                        },
                        "required": ["executionId"]
                    }),
                    other if other.starts_with("mcp.") => {
                        json!({"type": "object", "additionalProperties": true})
                    }
                    other if other.starts_with("connector.") => {
                        let schema = match other
                            .strip_prefix("connector.")
                            .and_then(|rest| rest.split_once('.'))
                        {
                            Some((connection_id, operation)) => self
                                .connectors
                                .operation_schema(connection_id, operation)
                                .await
                                .ok()
                                .flatten(),
                            None => None,
                        };
                        schema.unwrap_or_else(
                            || json!({"type": "object", "additionalProperties": true}),
                        )
                    }
                    _ => json!({"type": "object", "additionalProperties": true}),
                }
            };
            tools.push(json!({
                "type": "function",
                "function": {
                    "name": tool,
                    "description": format!("Agent tool `{tool}`"),
                    "parameters": schema,
                }
            }));
        }
        if !definition.sub_agents.is_empty() {
            tools.push(json!({
                "type": "function",
                "function": {
                    "name": "agent.handoff",
                    "description": "Hand off the task to a declared sub-agent.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "agentId": {"type": "string", "enum": definition.sub_agents},
                            "input": {"type": "object"}
                        },
                        "required": ["agentId"]
                    }
                }
            }));
        }
        Ok(tools)
    }
}

fn prepare_messages(definition: &AgentDefinition, state: &mut AgentSessionState) {
    if matches!(definition.planner, PlannerConfig::OpenAi { .. }) {
        let instructions = definition
            .instructions
            .clone()
            .unwrap_or_else(|| format!("You are {}.", definition.name));
        state.messages = vec![
            json!({"role": "system", "content": instructions}),
            json!({"role": "user", "content": serde_json::to_string(&state.input).unwrap_or_default()}),
        ];
    }
}

fn parse_definition(row: &AgentVersionRow) -> Result<AgentDefinition, AgentError> {
    let definition: AgentDefinition = serde_json::from_str(&row.config_json)?;
    validate_definition(&definition).map_err(|issues| AgentError::Invalid(issues.join("; ")))?;
    Ok(definition)
}

pub fn validate_definition(definition: &AgentDefinition) -> Result<(), Vec<String>> {
    let mut issues = Vec::new();
    if definition.schema_version != AGENT_SCHEMA {
        issues.push(format!("schemaVersion must be `{AGENT_SCHEMA}`"));
    }
    if definition.id.trim().is_empty() {
        issues.push("id is required".to_string());
    }
    if definition.name.trim().is_empty() {
        issues.push("name is required".to_string());
    }
    if !valid_semver(&definition.version) {
        issues.push("version must be semver".to_string());
    }
    let tools = definition.tools.iter().collect::<BTreeSet<_>>();
    if tools.len() != definition.tools.len() {
        issues.push("tools must be unique".to_string());
    }
    for tool in &definition.tools {
        if !tool.starts_with("skill.")
            && !tool.starts_with("mcp.")
            && !tool.starts_with("connector.")
            && !matches!(
                tool.as_str(),
                "workflow.execute" | "workflow.list" | "execution.get" | "execution.wait"
            )
        {
            issues.push(format!("unknown tool `{tool}`"));
        }
    }
    let mut plan_len = 0;
    match &definition.planner {
        PlannerConfig::Deterministic { plan } => {
            plan_len = plan.len();
            for (index, step) in plan.iter().enumerate() {
                match step {
                    PlanStep::Tool { tool, arguments } => {
                        if !arguments.is_object() {
                            issues.push(format!("plan[{index}].arguments must be an object"));
                        }
                        if !definition.tools.iter().any(|item| item == tool) {
                            issues
                                .push(format!("plan[{index}] references undeclared tool `{tool}`"));
                        }
                    }
                    PlanStep::Respond { .. } => {}
                    PlanStep::Handoff { agent_id, .. } => {
                        if !definition.sub_agents.iter().any(|item| item == agent_id) {
                            issues.push(format!(
                                "plan[{index}] references undeclared sub-agent `{agent_id}`"
                            ));
                        }
                    }
                }
            }
        }
        PlannerConfig::OpenAi {
            endpoint, model, ..
        } => {
            if !(endpoint.starts_with("http://") || endpoint.starts_with("https://")) {
                issues.push("planner.endpoint must be http(s)".to_string());
            }
            if model.trim().is_empty() {
                issues.push("planner.model is required".to_string());
            }
        }
    }
    if plan_len == 0 && matches!(definition.planner, PlannerConfig::Deterministic { .. }) {
        issues.push("deterministic planner requires a non-empty plan".to_string());
    }
    if definition.limits.max_steps == 0
        || definition.limits.max_tool_calls == 0
        || definition.limits.timeout_ms == 0
        || definition.limits.max_depth == 0
    {
        issues.push("limits must be positive".to_string());
    }
    for rule in &definition.approval {
        if !definition.tools.iter().any(|tool| tool == &rule.tool) {
            issues.push(format!(
                "approval rule references undeclared tool `{}`",
                rule.tool
            ));
        }
        if rule.mode == ApprovalMode::Never && tool_risk(&rule.tool) == "high" {
            issues.push(format!(
                "high-risk tool `{}` cannot be configured with approval never",
                rule.tool
            ));
        }
    }
    if !definition.sub_agents.is_empty() && definition.limits.max_depth == 0 {
        issues.push("sub-agents require maxDepth >= 1".to_string());
    }
    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues)
    }
}

fn model_budget_exhausted(definition: &AgentDefinition, state: &AgentSessionState) -> bool {
    state.tokens_in + state.tokens_out > definition.limits.max_tokens
        || state.cost_usd > definition.limits.max_cost_usd
}

fn remaining_limits(definition: &AgentDefinition, state: &AgentSessionState) -> AgentLimits {
    AgentLimits {
        max_steps: definition.limits.max_steps.saturating_sub(state.steps),
        max_tool_calls: definition
            .limits
            .max_tool_calls
            .saturating_sub(state.tool_calls),
        timeout_ms: definition
            .limits
            .timeout_ms
            .saturating_sub(now_ms().saturating_sub(state.started_ms)),
        max_tokens: definition
            .limits
            .max_tokens
            .saturating_sub(state.tokens_in + state.tokens_out),
        max_cost_usd: (definition.limits.max_cost_usd - state.cost_usd).max(0.0),
        max_handoffs: definition
            .limits
            .max_handoffs
            .saturating_sub(state.handoffs),
        max_depth: definition.limits.max_depth,
    }
}

fn inherited_limits(child: &AgentLimits, inherited: &AgentLimits) -> AgentLimits {
    AgentLimits {
        max_steps: child.max_steps.min(inherited.max_steps),
        max_tool_calls: child.max_tool_calls.min(inherited.max_tool_calls),
        timeout_ms: child.timeout_ms.min(inherited.timeout_ms),
        max_tokens: child.max_tokens.min(inherited.max_tokens),
        max_cost_usd: child.max_cost_usd.min(inherited.max_cost_usd),
        max_handoffs: child.max_handoffs.min(inherited.max_handoffs),
        max_depth: child.max_depth.min(inherited.max_depth),
    }
}

fn requires_approval(definition: &AgentDefinition, tool: &str, risk: &str) -> bool {
    if risk == "high" {
        return true;
    }
    definition
        .approval
        .iter()
        .find(|rule| rule.tool == tool)
        .map(|rule| rule.mode == ApprovalMode::Always)
        .unwrap_or(false)
}

fn tool_risk(tool: &str) -> &'static str {
    if tool == "workflow.execute" || tool.starts_with("mcp.") || tool.starts_with("connector.") {
        "high"
    } else if matches!(tool, "workflow.list" | "execution.get" | "execution.wait") {
        "low"
    } else {
        "medium"
    }
}

fn resolve_action(action: PlanStep, input: &Value, last_output: &Value) -> PlanStep {
    match action {
        PlanStep::Tool { tool, arguments } => PlanStep::Tool {
            tool,
            arguments: resolve_value(&arguments, input, last_output),
        },
        PlanStep::Respond { output } => PlanStep::Respond {
            output: Some(
                output
                    .map(|value| resolve_value(&value, input, last_output))
                    .unwrap_or_else(|| last_output.clone()),
            ),
        },
        PlanStep::Handoff {
            agent_id,
            input: value,
        } => PlanStep::Handoff {
            agent_id,
            input: value.map(|value| resolve_value(&value, input, last_output)),
        },
    }
}

fn resolve_value(value: &Value, input: &Value, last_output: &Value) -> Value {
    match value {
        Value::Object(map) => {
            if map.len() == 1 && map.get("$input") == Some(&Value::Bool(true)) {
                return input.clone();
            }
            if map.len() == 1
                && let Some(Value::String(path)) = map.get("$input")
            {
                return lookup_path(input, path).cloned().unwrap_or(Value::Null);
            }
            if map.len() == 1 && map.get("$last") == Some(&Value::Bool(true)) {
                return last_output.clone();
            }
            if map.len() == 1
                && let Some(Value::String(path)) = map.get("$last")
            {
                return lookup_path(last_output, path)
                    .cloned()
                    .unwrap_or(Value::Null);
            }
            Value::Object(
                map.iter()
                    .map(|(key, value)| (key.clone(), resolve_value(value, input, last_output)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| resolve_value(item, input, last_output))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn lookup_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for segment in path.split('.').filter(|segment| !segment.is_empty()) {
        current = current.get(segment)?;
    }
    Some(current)
}

fn repeated_tail(history: &[String], count: usize) -> bool {
    if history.len() < count {
        return false;
    }
    let tail = &history[history.len() - count..];
    tail.iter().all(|item| item == &tail[0])
}

async fn validate_model_endpoint(endpoint: &str, network: &[String]) -> Result<(), AgentError> {
    let url = reqwest::Url::parse(endpoint)
        .map_err(|error| AgentError::Model(format!("invalid model endpoint: {error}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(AgentError::Model(
            "model endpoint must be http(s)".to_string(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(AgentError::Model(
            "model endpoint must not contain userinfo".to_string(),
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| AgentError::Model("model endpoint has no host".to_string()))?;
    let literal = host.parse::<std::net::IpAddr>().ok();
    if host == "localhost" || literal.is_some_and(|address| address.is_loopback()) {
        return Ok(());
    }
    let port = url.port_or_known_default().unwrap_or(443);
    let allowed = network.iter().any(|rule| {
        rule == "*"
            || rule == host
            || rule == &format!("{host}:{port}")
            || rule
                .strip_prefix("*.")
                .is_some_and(|suffix| host.ends_with(suffix) && host.len() > suffix.len())
    });
    if !allowed {
        return Err(AgentError::Model(format!(
            "model host `{host}:{port}` is not in the configured network allowlist"
        )));
    }
    if url.scheme() != "https" {
        return Err(AgentError::Model(
            "non-loopback model endpoints must use https".to_string(),
        ));
    }
    if let Some(address) = literal {
        if is_private_address(address) {
            return Err(AgentError::Model(
                "model endpoint resolves to a private address".to_string(),
            ));
        }
        return Ok(());
    }
    let addresses = tokio::net::lookup_host((host, port))
        .await
        .map_err(|error| AgentError::Model(format!("cannot resolve model host: {error}")))?;
    let mut resolved = false;
    for address in addresses {
        resolved = true;
        if is_private_address(address.ip()) {
            return Err(AgentError::Model(
                "model endpoint resolves to a private address".to_string(),
            ));
        }
    }
    if !resolved {
        return Err(AgentError::Model(
            "model endpoint did not resolve to an address".to_string(),
        ));
    }
    Ok(())
}

fn is_private_address(address: std::net::IpAddr) -> bool {
    match address {
        std::net::IpAddr::V4(address) => {
            address.is_private()
                || address.is_loopback()
                || address.is_link_local()
                || address.is_broadcast()
                || address.is_unspecified()
                || address.is_documentation()
        }
        std::net::IpAddr::V6(address) => {
            address.is_loopback()
                || address.is_unspecified()
                || address.is_unique_local()
                || address.is_unicast_link_local()
                || address.is_multicast()
        }
    }
}

pub fn canonical_agent_value(value: &Value) -> Result<(AgentDefinition, String), Vec<String>> {
    let definition: AgentDefinition = serde_json::from_value(value.clone())
        .map_err(|error| vec![format!("agent config is invalid: {error}")])?;
    validate_definition(&definition)?;
    let canonical = serde_json::to_string(&definition).map_err(|error| vec![error.to_string()])?;
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    Ok((definition, format!("{:x}", hasher.finalize())))
}

fn valid_semver(version: &str) -> bool {
    let core = version
        .split_once(['-', '+'])
        .map_or(version, |(core, _)| core);
    let parts = core.split('.').collect::<Vec<_>>();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

pub fn agent_summary_value(row: &AgentSummaryRow) -> Value {
    json!({
        "id": row.id,
        "name": row.name,
        "latestVersion": row.latest_version,
        "status": row.status,
        "createdAt": row.created_at,
        "updatedAt": row.updated_at,
    })
}

pub fn agent_version_value(row: &AgentVersionRow) -> Value {
    json!({
        "agentId": row.agent_id,
        "version": row.version,
        "name": row.name,
        "description": row.description,
        "contentHash": row.content_hash,
        "status": row.status,
        "createdAt": row.created_at,
        "publishedAt": row.published_at,
    })
}

fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

pub fn session_state(row: &AgentSessionRow) -> Result<AgentSessionState, AgentError> {
    Ok(serde_json::from_str(&row.state_json)?)
}

pub fn agent_session_value(row: &AgentSessionRow) -> Result<Value, AgentError> {
    let state = session_state(row)?;
    Ok(json!({
        "id": row.id,
        "agentId": row.agent_id,
        "agentVersion": row.agent_version,
        "status": row.status,
        "depth": row.depth,
        "parentSessionId": row.parent_session_id,
        "input": state.input,
        "output": state.output,
        "error": state.error,
        "stopReason": state.stop_reason,
        "steps": state.steps,
        "toolCalls": state.tool_calls,
        "handoffs": state.handoffs,
        "tokensIn": state.tokens_in,
        "tokensOut": state.tokens_out,
        "costUsd": state.cost_usd,
        "pendingApproval": state.pending_approval,
        "childSessionId": state.child_session_id,
        "planIndex": state.plan_index,
        "lastOutput": state.last_output,
        "createdAt": row.created_at,
        "updatedAt": row.updated_at,
        "finishedAt": row.finished_at,
    }))
}

pub fn agent_session_summary(row: &AgentSessionRow) -> Result<Value, AgentError> {
    let state = session_state(row)?;
    Ok(json!({
        "id": row.id,
        "agentId": row.agent_id,
        "agentVersion": row.agent_version,
        "status": row.status,
        "depth": row.depth,
        "parentSessionId": row.parent_session_id,
        "steps": state.steps,
        "toolCalls": state.tool_calls,
        "handoffs": state.handoffs,
        "costUsd": state.cost_usd,
        "createdAt": row.created_at,
        "updatedAt": row.updated_at,
        "finishedAt": row.finished_at,
    }))
}

pub fn approval_value(row: &AgentApprovalRow) -> Value {
    json!({
        "id": row.id,
        "sessionId": row.session_id,
        "toolName": row.tool_name,
        "arguments": serde_json::from_str::<Value>(&row.args_json).unwrap_or(Value::Null),
        "risk": row.risk,
        "status": row.status,
        "createdAt": row.created_at,
        "expiresAt": row.expires_at,
        "decidedAt": row.decided_at,
        "usedAt": row.used_at,
    })
}

pub fn event_value(row: &AgentEventRow) -> Value {
    json!({
        "sessionId": row.session_id,
        "sequence": row.sequence,
        "at": row.at,
        "kind": row.kind,
        "data": serde_json::from_str::<Value>(&row.data_json).unwrap_or(Value::Null),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::AgentSessionRow;

    #[tokio::test]
    async fn interrupted_session_recovers_from_checkpoint() {
        let data_dir = std::env::temp_dir().join(format!("hpa-agent-recover-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&data_dir).expect("data dir");
        let db = Db::open(&data_dir.join("hpa.db")).await.expect("db");
        let config = json!({
            "schemaVersion": AGENT_SCHEMA,
            "id": "agent_recover",
            "name": "Recover",
            "version": "1.0.0",
            "tools": [],
            "planner": {
                "kind": "deterministic",
                "plan": [{"kind": "respond", "output": {"recovered": true}}]
            }
        });
        let (definition, hash) = canonical_agent_value(&config).expect("config");
        let now = now_rfc3339();
        db.upsert_agent_version(&AgentVersionRow {
            agent_id: definition.id.clone(),
            version: definition.version.clone(),
            name: definition.name.clone(),
            description: None,
            config_json: serde_json::to_string(&definition).expect("config json"),
            content_hash: hash,
            status: "imported".to_string(),
            created_at: now.clone(),
            published_at: None,
        })
        .await
        .expect("version");
        db.publish_agent_version(&definition.id, &definition.version, &now)
            .await
            .expect("publish");
        let state = AgentSessionState {
            schema_version: AGENT_SESSION_SCHEMA.to_string(),
            agent_id: definition.id.clone(),
            agent_version: definition.version.clone(),
            status: AgentStatus::Running,
            input: json!({}),
            plan_index: 0,
            plan_history: Vec::new(),
            steps: 0,
            tool_calls: 0,
            handoffs: 0,
            depth: 0,
            tokens_in: 0,
            tokens_out: 0,
            cost_usd: 0.0,
            messages: Vec::new(),
            last_output: Value::Null,
            output: None,
            pending_action: None,
            pending_approval: None,
            pending_tool_call_id: None,
            child_session_id: None,
            stop_reason: None,
            error: None,
            started_ms: now_ms(),
            updated_at: now.clone(),
            event_sequence: 0,
        };
        let session_id = "agentsess_recover".to_string();
        db.insert_agent_session(&AgentSessionRow {
            id: session_id.clone(),
            agent_id: definition.id.clone(),
            agent_version: definition.version.clone(),
            status: "running".to_string(),
            input_json: "{}".to_string(),
            output_json: None,
            state_json: serde_json::to_string(&state).expect("state"),
            error_json: None,
            parent_session_id: None,
            depth: 0,
            created_at: now.clone(),
            updated_at: now,
            finished_at: None,
        })
        .await
        .expect("session");
        let runtime = AgentRuntime::new(
            SkillService::new(db.clone(), data_dir.clone()),
            RuntimeManager::new(db.clone()),
            crate::connector::ConnectorRuntime::new(db.clone()),
        );
        assert_eq!(runtime.recover().await.expect("recover"), 1);
        let row = db
            .get_agent_session(&session_id)
            .await
            .expect("load")
            .expect("session");
        assert_eq!(
            session_state(&row).expect("state").status,
            AgentStatus::Interrupted
        );
        let resumed = runtime.drive(&session_id).await.expect("resume");
        assert_eq!(resumed.status, AgentStatus::Succeeded.as_str());
        let resumed_state = session_state(&resumed).expect("state");
        assert_eq!(resumed_state.output, Some(json!({"recovered": true})));
        assert!(resumed_state.event_sequence > 0);
    }
}
