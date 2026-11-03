use std::collections::BTreeMap;
use std::convert::Infallible;
use std::path::{Path as FsPath, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use futures_core::Stream;
use hpa_core::compiler::compile_workflow;
use hpa_core::error::ValidationReport;
use hpa_core::ir::Workflow;
use hpa_core::runtime::{
    ApprovalRequest, ExecutionError, ExecutionEvent, ExecutionState, ExecutionStatus,
    cancel_execution, new_state, resume_approval, start_event,
};
use hpa_core::{registry, validator};
use hpa_skill::{SkillError, SkillManifest, runtime_versions};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::sync::mpsc;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

use crate::agent::{self, AgentError, AgentLimits, AgentRuntime, AgentStart};
use crate::connector::{self, ConnectionInput, ConnectorError, ConnectorRuntime};
use crate::db::{
    AgentApprovalRow, AgentEventRow, AgentSessionRow, AgentVersionRow, ArtifactRow, Db, DbError,
    ExecutionRow, MarketplaceEventRow, McpServerRow, SkillRunRow, SkillSummaryRow, SkillVersionRow,
    TriggerStateRow, WorkflowRow, WorkflowVersionRow, parse_state,
};
use crate::driver::RuntimeManager;
use crate::mcp::{self, McpService};
use crate::mcp_client::{self, ExternalServer};
use crate::plugin::{BundleOptions, PluginError, PluginService};
use crate::skills::{LoadedSkill, SkillService, SkillServiceError, TestResult, sandbox_label};

const BRAND_JSON: &str = include_str!("../../../branding/brand.json");

#[derive(Clone)]
pub struct ApiState {
    pub tenant_id: String,
    pub db: Db,
    pub runtime: RuntimeManager,
    pub agents: AgentRuntime,
    pub connectors: ConnectorRuntime,
    pub plugins: PluginService,
    pub data_dir: PathBuf,
    pub mcp_base_url: String,
    pub worker_queue: Option<crate::worker::WorkerService>,
    pub event_bus: Option<crate::event_bus::EventBusPublisher>,
}

impl ApiState {
    pub(crate) fn trigger_host(&self) -> crate::scheduler::TriggerHost {
        crate::scheduler::TriggerHost::new(
            self.db.clone(),
            self.runtime.clone(),
            self.tenant_id.clone(),
        )
    }
}

fn worker_api_error(error: crate::worker::WorkerError) -> ApiError {
    use crate::worker::WorkerErrorKind;
    match error.kind {
        WorkerErrorKind::Invalid => ApiError::bad_request("WORKER_INVALID", error.message),
        WorkerErrorKind::NotFound => ApiError::not_found(error.message),
        WorkerErrorKind::Conflict => ApiError::conflict("WORKER_CONFLICT", error.message),
        WorkerErrorKind::Forbidden => {
            ApiError::new(StatusCode::FORBIDDEN, "WORKER_FORBIDDEN", error.message)
        }
        WorkerErrorKind::Internal => ApiError::internal(error.message),
    }
}

pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/openapi.json", get(openapi_contract))
        .route("/nodes", get(list_nodes))
        .route("/stats", get(stats))
        .route("/workflows", get(list_workflows).post(create_workflow))
        .route("/workflows/validate", post(validate_workflow))
        .route("/workflows/{id}", get(get_workflow).put(update_workflow))
        .route("/workflows/{id}/publish", post(publish_workflow))
        .route("/workflows/{id}/versions", get(list_versions))
        .route("/workflows/{id}/executions", post(start_execution))
        .route("/executions", get(list_executions))
        .route("/executions/{id}", get(get_execution))
        .route("/executions/{id}/events", get(list_events))
        .route("/executions/{id}/events/stream", get(stream_events))
        .route("/executions/{id}/approvals", post(decide_approval))
        .route("/executions/{id}/cancel", post(cancel))
        .route("/approvals", get(list_approvals))
        .route("/artifacts", get(list_artifacts))
        .route("/artifacts/{id}", get(get_artifact))
        .route("/agents", get(list_agents).post(create_agent))
        .route("/agents/runtime", get(agent_runtime_info))
        .route("/agents/tools", get(agent_tools))
        .route("/internal/agent-tool-calls", post(agent_tool_call))
        .route("/agents/approvals", get(list_agent_approvals))
        .route("/agents/approvals/{id}/decide", post(decide_agent_approval))
        .route("/agents/sessions", get(list_agent_sessions))
        .route("/agents/sessions/{id}", get(get_agent_session))
        .route(
            "/agents/sessions/{id}/events",
            get(list_agent_session_events),
        )
        .route(
            "/agents/sessions/{id}/continue",
            post(continue_agent_session),
        )
        .route("/agents/sessions/{id}/cancel", post(cancel_agent_session))
        .route("/agents/sessions/{id}/replay", post(replay_agent_session))
        .route("/agents/{id}", get(get_agent))
        .route("/agents/{id}/versions/{version}", get(get_agent_version))
        .route("/agents/{id}/publish", post(publish_agent))
        .route("/agents/{id}/run", post(run_agent))
        .route("/plugins", get(list_plugins))
        .route("/plugins/key", get(plugin_key))
        .route(
            "/plugins/keys",
            get(list_plugin_keys).post(trust_plugin_key),
        )
        .route("/plugins/inspect", post(inspect_plugin))
        .route("/plugins/install", post(install_plugin))
        .route("/plugins/sample", post(create_sample_plugin))
        .route("/plugins/{id}", get(get_plugin))
        .route("/plugins/{id}/versions/{version}", get(get_plugin_version))
        .route("/plugins/{id}/enable", post(enable_plugin))
        .route("/plugins/{id}/disable", post(disable_plugin))
        .route("/plugins/{id}/rollback", post(rollback_plugin))
        .route("/plugins/{id}/uninstall", post(uninstall_plugin))
        .route("/marketplace", get(marketplace_list))
        .route("/marketplace/publish", post(publish_marketplace))
        .route("/marketplace/remote", get(remote_marketplace_packages))
        .route(
            "/marketplace/vulnerabilities/{id}/{version}",
            get(marketplace_vulnerabilities),
        )
        .route("/marketplace/events/stream", get(stream_marketplace_events))
        .route("/marketplace/events/latest", get(marketplace_event_cursor))
        .route(
            "/marketplace/publishers",
            get(list_marketplace_publishers).post(add_marketplace_publisher),
        )
        .route(
            "/marketplace/publishers/{id}/keys",
            post(add_marketplace_publisher_key).delete(revoke_marketplace_publisher_key),
        )
        .route(
            "/marketplace/sources",
            get(list_marketplace_sources).post(add_marketplace_source),
        )
        .route(
            "/marketplace/sources/{id}/rotate-key",
            post(rotate_marketplace_source_key),
        )
        .route(
            "/marketplace/sources/{id}/refresh",
            post(refresh_marketplace_source),
        )
        .route(
            "/marketplace/remote/{source}/{id}/versions/{version}/install",
            post(install_remote_marketplace_package),
        )
        .route(
            "/marketplace/{id}/versions/{version}",
            get(marketplace_detail),
        )
        .route(
            "/marketplace/{id}/versions/{version}/review",
            post(review_marketplace),
        )
        .route(
            "/marketplace/{id}/versions/{version}/withdraw",
            post(withdraw_marketplace),
        )
        .route(
            "/marketplace/{id}/versions/{version}/rate",
            post(rate_marketplace),
        )
        .route(
            "/marketplace/{id}/versions/{version}/install",
            post(install_marketplace),
        )
        .route("/connectors", get(list_connectors))
        .route("/connectors/sample", post(create_sample_connection))
        .route("/connectors/{id}", get(get_connector))
        .route("/connections", get(list_connections).post(save_connection))
        .route("/connections/{id}", delete(delete_connection))
        .route("/connections/{id}/test", post(test_connection))
        .route("/connections/{id}/invoke", post(invoke_connection))
        .route("/connector-audit", get(connector_audit))
        .route("/skills", get(list_skills))
        .route("/skills/runtimes", get(skill_runtimes))
        .route("/skills/import", post(import_skill))
        .route("/skills/{id}", get(get_skill))
        .route("/skills/{id}/runs", get(list_skill_runs))
        .route("/skills/{id}/versions/{version}", get(get_skill_version))
        .route(
            "/skills/{id}/versions/{version}/source",
            get(get_skill_source),
        )
        .route(
            "/skills/{id}/versions/{version}/draft",
            post(save_skill_draft),
        )
        .route("/skills/{id}/versions/{version}/sign", post(sign_skill))
        .route(
            "/skills/{id}/versions/{version}/publish",
            post(publish_skill),
        )
        .route("/skills/{id}/versions/{version}/export", post(export_skill))
        .route("/skills/{id}/versions/{version}/run", post(run_skill))
        .route("/skills/{id}/versions/{version}/tests", post(test_skill))
        .route("/skill-runs/{id}", get(get_skill_run))
        .route("/skill-runs/{id}/artifacts", get(list_skill_run_artifacts))
        .route(
            "/skill-runs/{id}/artifacts/{artifact_id}",
            get(get_skill_artifact),
        )
        .route("/triggers", get(list_triggers_api))
        .route(
            "/triggers/{workflow_id}/{trigger_id}/enable",
            post(enable_trigger_api),
        )
        .route(
            "/triggers/{workflow_id}/{trigger_id}/disable",
            post(disable_trigger_api),
        )
        .route(
            "/triggers/{workflow_id}/{trigger_id}/fire",
            post(fire_trigger_api),
        )
        .route("/trigger-fires", get(list_trigger_fires_api))
        .route("/events/{topic}", post(publish_event_api))
        .route("/mcp/overview", get(mcp_overview))
        .route("/mcp/catalog", get(mcp_catalog))
        .route("/mcp/call", post(mcp_call))
        .route("/mcp/resource", post(mcp_read_resource))
        .route("/mcp/prompt", post(mcp_get_prompt))
        .route("/mcp/tokens", get(mcp_tokens).post(mcp_token_create))
        .route("/mcp/tokens/{id}", delete(mcp_token_revoke))
        .route("/mcp/approvals", get(mcp_approvals))
        .route("/mcp/approvals/{id}/decide", post(mcp_approval_decide))
        .route("/mcp/audit", get(mcp_audit))
        .route("/mcp/servers", get(mcp_servers).post(mcp_server_upsert))
        .route("/mcp/servers/{id}", delete(mcp_server_delete))
        .route("/mcp/servers/{id}/probe", post(mcp_server_probe))
        .route("/mcp/servers/{id}/call", post(mcp_server_call))
        .fallback(api_not_found)
        .with_state(state)
        .route_layer(axum::middleware::from_fn(crate::metrics::track_http))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
}

async fn openapi_contract() -> Json<Value> {
    const CONTRACT: &str = include_str!("../../../schema/openapi/local-api.json");
    match serde_json::from_str(CONTRACT) {
        Ok(value) => Json(value),
        Err(error) => Json(json!({
            "error": {
                "code": "OPENAPI_INVALID",
                "message": format!("embedded OpenAPI contract is invalid: {error}")
            }
        })),
    }
}

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: String,
    pub message: String,
    pub details: Option<Value>,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &str, message: impl Into<String>) -> Self {
        Self {
            status,
            code: code.to_string(),
            message: message.into(),
            details: None,
        }
    }

    pub fn bad_request(code: &str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "NOT_FOUND", message)
    }

    pub fn conflict(code: &str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, code, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL", message)
    }

    pub fn validation(report: ValidationReport) -> Self {
        let details = serde_json::to_value(&report).ok();
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "VALIDATION_FAILED".to_string(),
            message: format!(
                "workflow validation failed with {} error(s)",
                report.error_count()
            ),
            details,
        }
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = json!({
            "error": {
                "code": self.code,
                "message": self.message,
                "details": self.details,
            }
        });
        (self.status, Json(body)).into_response()
    }
}

impl From<DbError> for ApiError {
    fn from(error: DbError) -> Self {
        match error {
            DbError::AgentVersionFrozen { agent_id, version } => Self::conflict(
                "AGENT_VERSION_FROZEN",
                format!(
                    "agent `{agent_id}` version `{version}` is already stored with different content; bump the version"
                ),
            ),
            other => Self::internal(other.to_string()),
        }
    }
}

impl From<AgentError> for ApiError {
    fn from(error: AgentError) -> Self {
        match error {
            AgentError::Db(error) => error.into(),
            AgentError::Skill(error) => error.into(),
            AgentError::Connector(error) => error.into(),
            AgentError::Invalid(message) => {
                Self::new(StatusCode::UNPROCESSABLE_ENTITY, "AGENT_INVALID", message)
            }
            AgentError::NotFound(message) => {
                Self::new(StatusCode::NOT_FOUND, "AGENT_NOT_FOUND", message)
            }
            AgentError::Tool(message) => {
                Self::new(StatusCode::UNPROCESSABLE_ENTITY, "AGENT_TOOL", message)
            }
            AgentError::Model(message) => {
                Self::new(StatusCode::BAD_GATEWAY, "AGENT_MODEL", message)
            }
            AgentError::Approval(message) => Self::conflict("AGENT_APPROVAL", message),
            AgentError::Forbidden(message) => {
                Self::new(StatusCode::FORBIDDEN, "AGENT_FORBIDDEN", message)
            }
            AgentError::Budget(message) => {
                Self::new(StatusCode::UNPROCESSABLE_ENTITY, "AGENT_BUDGET", message)
            }
            AgentError::Lease(message) => Self::conflict("AGENT_LEASE", message),
            AgentError::Json(error) => Self::internal(format!("JSON error: {error}")),
        }
    }
}

impl From<ConnectorError> for ApiError {
    fn from(error: ConnectorError) -> Self {
        match error {
            ConnectorError::Db(error) => error.into(),
            ConnectorError::Sqlx(error) => Self::internal(error.to_string()),
            ConnectorError::Json(error) => Self::internal(format!("JSON error: {error}")),
            ConnectorError::Io(error) => Self::internal(error.to_string()),
            ConnectorError::Invalid(message) => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "CONNECTOR_INVALID",
                message,
            ),
            ConnectorError::NotFound(message) => {
                Self::new(StatusCode::NOT_FOUND, "CONNECTOR_NOT_FOUND", message)
            }
            ConnectorError::Forbidden(message) => {
                Self::new(StatusCode::FORBIDDEN, "CONNECTOR_FORBIDDEN", message)
            }
            ConnectorError::RateLimited(message) => Self::new(
                StatusCode::TOO_MANY_REQUESTS,
                "CONNECTOR_RATE_LIMIT",
                message,
            ),
            ConnectorError::Upstream(message) => {
                Self::new(StatusCode::BAD_GATEWAY, "CONNECTOR_UPSTREAM", message)
            }
        }
    }
}

impl From<PluginError> for ApiError {
    fn from(error: PluginError) -> Self {
        match error {
            PluginError::Db(error) => error.into(),
            PluginError::Sqlx(error) => Self::internal(error.to_string()),
            PluginError::Json(error) => Self::internal(format!("JSON error: {error}")),
            PluginError::Io(error) => Self::internal(error.to_string()),
            PluginError::Skill(error) => error.into(),
            PluginError::Connector(error) => error.into(),
            PluginError::Invalid(message) => {
                Self::new(StatusCode::UNPROCESSABLE_ENTITY, "PLUGIN_INVALID", message)
            }
            PluginError::NotFound(message) => {
                Self::new(StatusCode::NOT_FOUND, "PLUGIN_NOT_FOUND", message)
            }
            PluginError::Frozen(message) => Self::conflict("PLUGIN_VERSION_FROZEN", message),
            PluginError::Signature(message) => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "PLUGIN_SIGNATURE",
                message,
            ),
            PluginError::License(message) => {
                Self::new(StatusCode::UNPROCESSABLE_ENTITY, "PLUGIN_LICENSE", message)
            }
            PluginError::Dependency(message) => Self::conflict("PLUGIN_DEPENDENCY", message),
            PluginError::Conflict(message) => Self::conflict("PLUGIN_CONFLICT", message),
            PluginError::Forbidden(message) => {
                Self::new(StatusCode::FORBIDDEN, "PLUGIN_FORBIDDEN", message)
            }
        }
    }
}

impl From<SkillServiceError> for ApiError {
    fn from(error: SkillServiceError) -> Self {
        match error {
            SkillServiceError::Skill(error) => match error {
                SkillError::NotFound(message) => Self::not_found(message),
                SkillError::LicenseDenied(message) => {
                    Self::new(StatusCode::UNPROCESSABLE_ENTITY, "SKILL_LICENSE", message)
                }
                SkillError::Permission(message) => {
                    Self::new(StatusCode::FORBIDDEN, "SKILL_PERMISSION", message)
                }
                SkillError::RuntimeMissing(message) => Self::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "SKILL_RUNTIME_MISSING",
                    format!("runtime `{message}` is not available"),
                ),
                SkillError::Invalid { code, message } => {
                    Self::new(StatusCode::UNPROCESSABLE_ENTITY, &code, message)
                }
                other => Self::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    other.code(),
                    other.to_string(),
                ),
            },
            SkillServiceError::Db(DbError::SkillVersionFrozen { skill_id, version }) => {
                Self::conflict(
                    "SKILL_VERSION_FROZEN",
                    format!(
                        "skill `{skill_id}` version `{version}` is already imported with different content; bump version"
                    ),
                )
            }
            SkillServiceError::Db(error) => Self::internal(error.to_string()),
            SkillServiceError::Io(error) => Self::internal(error.to_string()),
            SkillServiceError::Json(error) => Self::internal(error.to_string()),
            SkillServiceError::NotFound(message) => Self::not_found(message),
            SkillServiceError::Invalid {
                code,
                message,
                details,
            } => {
                let mut error = Self::new(StatusCode::UNPROCESSABLE_ENTITY, &code, message);
                if let Some(details) = details {
                    error = error.with_details(details);
                }
                error
            }
        }
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(error: serde_json::Error) -> Self {
        Self::internal(format!("JSON error: {error}"))
    }
}

async fn health() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "internalId": "hpa",
        "version": env!("CARGO_PKG_VERSION"),
        "brand": brand(),
        "runtime": "rust",
    }))
}

fn brand() -> Value {
    serde_json::from_str(BRAND_JSON).unwrap_or_else(|_| json!({"internalId": "hpa"}))
}

async fn stats(State(state): State<ApiState>) -> Result<Json<Value>, ApiError> {
    let stats = state.db.stats().await?;
    Ok(Json(serde_json::to_value(stats)?))
}

async fn list_nodes() -> Json<Value> {
    Json(json!({
        "registryVersion": registry::REGISTRY_VERSION,
        "nodes": registry::definitions(),
    }))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowSummary {
    id: String,
    name: String,
    version: String,
    description: Option<String>,
    published: bool,
    created_at: String,
    updated_at: String,
}

impl From<&WorkflowRow> for WorkflowSummary {
    fn from(row: &WorkflowRow) -> Self {
        Self {
            id: row.id.clone(),
            name: row.name.clone(),
            version: row.version.clone(),
            description: row.description.clone(),
            published: row.published != 0,
            created_at: row.created_at.clone(),
            updated_at: row.updated_at.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowDetail {
    summary: WorkflowSummary,
    ir: Value,
    yaml: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct VersionSummary {
    version: String,
    registry_version: String,
    created_at: String,
}

impl From<&WorkflowVersionRow> for VersionSummary {
    fn from(row: &WorkflowVersionRow) -> Self {
        Self {
            version: row.version.clone(),
            registry_version: row.registry_version.clone(),
            created_at: row.created_at.clone(),
        }
    }
}

async fn list_workflows(
    State(state): State<ApiState>,
) -> Result<Json<Vec<WorkflowSummary>>, ApiError> {
    let rows = state.db.list_workflows().await?;
    Ok(Json(rows.iter().map(WorkflowSummary::from).collect()))
}

async fn get_workflow(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<WorkflowDetail>, ApiError> {
    let row = state
        .db
        .get_workflow(&id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("workflow `{id}` not found")))?;
    let ir: Value = serde_json::from_str(&row.ir_json)?;
    Ok(Json(WorkflowDetail {
        summary: WorkflowSummary::from(&row),
        ir,
        yaml: row.yaml,
    }))
}

#[derive(Debug, Deserialize)]
struct SaveWorkflowRequest {
    ir: Value,
}

async fn create_workflow(
    State(state): State<ApiState>,
    Json(request): Json<SaveWorkflowRequest>,
) -> Result<Json<WorkflowSummary>, ApiError> {
    let (workflow, ir, yaml) = validate_ir(&request.ir)?;
    let row = workflow_row(&workflow, &ir, &yaml);
    state.db.upsert_workflow(&row).await?;
    notify_mcp_catalog(&state);
    Ok(Json(WorkflowSummary::from(&row)))
}

async fn update_workflow(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(request): Json<SaveWorkflowRequest>,
) -> Result<Json<WorkflowSummary>, ApiError> {
    let (workflow, ir, yaml) = validate_ir(&request.ir)?;
    if workflow.id != id {
        return Err(ApiError::bad_request(
            "WORKFLOW_ID_MISMATCH",
            format!("workflow id `{}` does not match path `{id}`", workflow.id),
        ));
    }
    let row = workflow_row(&workflow, &ir, &yaml);
    state.db.upsert_workflow(&row).await?;
    notify_mcp_catalog(&state);
    Ok(Json(WorkflowSummary::from(&row)))
}

fn validate_ir(ir: &Value) -> Result<(Workflow, Value, String), ApiError> {
    let compilation = validator::check_value(ir).map_err(ApiError::validation)?;
    let workflow = Workflow::from_value(ir)
        .map_err(|error| ApiError::internal(format!("stored workflow is invalid: {error}")))?;
    if !compilation.issues.is_empty() {
        tracing::warn!(warnings = ?compilation.issues, "workflow saved with validation warnings");
    }
    let canonical = workflow
        .canonical_value()
        .map_err(|error| ApiError::internal(format!("cannot canonicalize workflow: {error}")))?;
    let yaml = workflow
        .to_yaml_string()
        .map_err(|error| ApiError::internal(format!("cannot render workflow YAML: {error}")))?;
    Ok((workflow, canonical, yaml))
}

fn workflow_row(workflow: &Workflow, canonical: &Value, yaml: &str) -> WorkflowRow {
    let now = now_rfc3339();
    WorkflowRow {
        id: workflow.id.clone(),
        name: workflow.name.clone(),
        version: workflow.workflow_version.clone(),
        description: workflow.description.clone(),
        published: 0,
        ir_json: serde_json::to_string_pretty(canonical).unwrap_or_else(|_| "{}".to_string()),
        yaml: yaml.to_string(),
        created_at: now.clone(),
        updated_at: now,
    }
}

async fn publish_workflow(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<WorkflowSummary>, ApiError> {
    let mut row = state
        .db
        .get_workflow(&id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("workflow `{id}` not found")))?;
    let ir: Value = serde_json::from_str(&row.ir_json)?;
    let (workflow, _canonical, yaml) = validate_ir(&ir)?;
    row.yaml = yaml;
    row.published = 1;
    row.updated_at = now_rfc3339();
    state
        .db
        .publish_workflow(&row, registry::REGISTRY_VERSION)
        .await
        .map_err(|error| match error {
            crate::db::DbError::VersionFrozen {
                workflow_id,
                version,
            } => ApiError::conflict(
                "VERSION_FROZEN",
                format!(
                    "workflow `{workflow_id}` version `{version}` is already published with different content; bump workflowVersion to publish a new version"
                ),
            ),
            other => ApiError::internal(other.to_string()),
        })?;
    crate::scheduler::sync_published(&state.trigger_host(), &workflow, &row.version)
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?;
    notify_mcp_catalog(&state);
    Ok(Json(WorkflowSummary::from(&row)))
}

async fn list_versions(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<VersionSummary>>, ApiError> {
    let rows = state.db.list_versions(&id).await?;
    Ok(Json(rows.iter().map(VersionSummary::from).collect()))
}

#[derive(Debug, Deserialize)]
struct ValidateRequest {
    format: Option<String>,
    content: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ValidateResponse {
    valid: bool,
    report: ValidationReport,
    canonical: Option<Value>,
    yaml: Option<String>,
    source_map: Option<hpa_core::yaml::SourceMap>,
    node_count: usize,
    edge_count: usize,
}

async fn validate_workflow(
    Json(request): Json<ValidateRequest>,
) -> Result<Json<ValidateResponse>, ApiError> {
    match check_content(request.format.as_deref(), &request.content) {
        Ok((workflow, compilation)) => {
            let canonical = workflow.canonical_value().ok();
            let yaml = workflow.to_yaml_string().ok();
            let source_map = if request.format.as_deref() != Some("json")
                && !request.content.trim_start().starts_with('{')
            {
                hpa_core::yaml::yaml_to_json_with_source_map(&request.content)
                    .ok()
                    .map(|(_, map)| map)
            } else {
                None
            };
            Ok(Json(ValidateResponse {
                valid: true,
                report: ValidationReport {
                    issues: compilation.issues.clone(),
                },
                canonical,
                yaml,
                source_map,
                node_count: workflow.nodes.len(),
                edge_count: workflow.edges.len(),
            }))
        }
        Err(report) => Err(ApiError::validation(report)),
    }
}

fn check_content(
    format: Option<&str>,
    content: &str,
) -> Result<(Workflow, hpa_core::compiler::Compilation), ValidationReport> {
    match format {
        Some("yaml") => {
            let compilation = validator::check_yaml(content)?;
            let workflow = Workflow::from_yaml_str(content).map_err(report_from_error)?;
            Ok((workflow, compilation))
        }
        Some("json") => {
            let compilation = validator::check_json(content)?;
            let workflow = Workflow::from_json_str(content).map_err(report_from_error)?;
            Ok((workflow, compilation))
        }
        _ => match validator::check_json(content) {
            Ok(compilation) => {
                let workflow = Workflow::from_json_str(content).map_err(report_from_error)?;
                Ok((workflow, compilation))
            }
            Err(json_report) => {
                if content.trim_start().starts_with('{') {
                    return Err(json_report);
                }
                let compilation = validator::check_yaml(content)?;
                let workflow = Workflow::from_yaml_str(content).map_err(report_from_error)?;
                Ok((workflow, compilation))
            }
        },
    }
}

fn report_from_error(error: hpa_core::error::CoreError) -> ValidationReport {
    ValidationReport {
        issues: vec![hpa_core::error::Issue::error(
            "IR_INVALID",
            "$",
            error.to_string(),
        )],
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExecutionSummary {
    id: String,
    workflow_id: String,
    workflow_version: String,
    status: ExecutionStatus,
    started_at: String,
    updated_at: String,
    waiting: Option<ApprovalRequest>,
    output: Option<Value>,
    error: Option<ExecutionError>,
}

impl ExecutionSummary {
    fn from_state(state: &ExecutionState) -> Self {
        Self {
            id: state.id.clone(),
            workflow_id: state.workflow_id.clone(),
            workflow_version: state.workflow_version.clone(),
            status: state.status,
            started_at: state.started_at.clone(),
            updated_at: state.updated_at.clone(),
            waiting: state.waiting.clone(),
            output: state.output.clone(),
            error: state.error.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExecutionDetail {
    summary: ExecutionSummary,
    state: ExecutionState,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecuteRequest {
    input: Option<Value>,
    env: Option<BTreeMap<String, String>>,
    tenant_id: Option<String>,
    version: Option<String>,
}

async fn start_execution(
    State(state): State<ApiState>,
    Path(workflow_id): Path<String>,
    Json(request): Json<ExecuteRequest>,
) -> Result<Json<ExecutionDetail>, ApiError> {
    let (workflow, frozen_registry) = if let Some(version) = request.version.as_deref() {
        let Some(row) = state.db.get_version(&workflow_id, version).await? else {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "VERSION_NOT_FOUND",
                format!("workflow `{workflow_id}` has no published version `{version}`"),
            ));
        };
        (
            serde_json::from_str::<Workflow>(&row.ir_json)?,
            Some(row.registry_version),
        )
    } else {
        let row =
            state.db.get_workflow(&workflow_id).await?.ok_or_else(|| {
                ApiError::not_found(format!("workflow `{workflow_id}` not found"))
            })?;
        (serde_json::from_str::<Workflow>(&row.ir_json)?, None)
    };
    let compilation = compile_workflow(&workflow).map_err(|issues| {
        ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_FAILED",
            "workflow does not compile",
        )
        .with_details(serde_json::to_value(issues).unwrap_or(Value::Null))
    })?;
    let execution_id = format!("exec_{}", Uuid::now_v7());
    let env = request.env.unwrap_or_default();
    if request
        .tenant_id
        .as_deref()
        .is_some_and(|tenant_id| tenant_id != state.tenant_id)
    {
        return Err(ApiError::bad_request(
            "TENANT_MISMATCH",
            "execution tenant must match the authenticated tenant",
        ));
    }
    let tenant_id = state.tenant_id.clone();
    let mut execution = new_state(
        &workflow,
        &compilation.plan,
        &execution_id,
        request.input.unwrap_or(json!({})),
        env,
        tenant_id,
    )
    .map_err(|error| ApiError::bad_request("INPUT_INVALID", error.to_string()))?;
    if let Some(registry_version) = frozen_registry
        && !registry_version.is_empty()
    {
        execution.registry_version = registry_version;
    }
    let event = start_event(&mut execution);
    let execution_row = ExecutionRow {
        id: execution.id.clone(),
        workflow_id: execution.workflow_id.clone(),
        workflow_version: execution.workflow_version.clone(),
        status: "running".to_string(),
        state_json: serde_json::to_string(&execution)?,
        workflow_json: workflow.to_json_string(false).map_err(|error| {
            ApiError::internal(format!("cannot serialize workflow snapshot: {error}"))
        })?,
        output_json: None,
        error_json: None,
        started_at: execution.started_at.clone(),
        updated_at: execution.updated_at.clone(),
    };
    state.db.insert_execution(&execution_row, &event).await?;
    state.runtime.trigger(execution_id);
    Ok(Json(ExecutionDetail {
        summary: ExecutionSummary::from_state(&execution),
        state: execution,
    }))
}

#[derive(Debug, Deserialize, Default)]
struct ListQuery {
    limit: Option<i64>,
    after: Option<i64>,
    offset: Option<i64>,
    status: Option<String>,
}

async fn list_executions(
    State(state): State<ApiState>,
    Query(query): Query<ListQuery>,
) -> Result<Response, ApiError> {
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let offset = query.offset.unwrap_or(0).max(0);
    let status = query
        .status
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let rows = state.db.list_executions_page(limit, offset, status).await?;
    let total = state.db.count_executions(status).await?;
    let mut summaries = Vec::with_capacity(rows.len());
    for row in rows {
        let execution = parse_state(&row)?;
        summaries.push(ExecutionSummary::from_state(&execution));
    }
    let mut response = Json(summaries).into_response();
    // The body stays a bare array so existing clients keep working; the total
    // travels in a header for the pager.
    response.headers_mut().insert(
        "x-total-count",
        HeaderValue::from_str(&total.to_string()).unwrap_or(HeaderValue::from_static("0")),
    );
    Ok(response)
}

async fn get_execution(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<ExecutionDetail>, ApiError> {
    let row = state
        .db
        .get_execution(&id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("execution `{id}` not found")))?;
    let execution = parse_state(&row)?;
    Ok(Json(ExecutionDetail {
        summary: ExecutionSummary::from_state(&execution),
        state: execution,
    }))
}

async fn list_events(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<ExecutionEvent>>, ApiError> {
    if state.db.get_execution(&id).await?.is_none() {
        return Err(ApiError::not_found(format!("execution `{id}` not found")));
    }
    let after = query.after.unwrap_or(0).max(0);
    let events = state.db.list_events(&id, after, 1_000).await?;
    Ok(Json(events))
}

struct SseStream {
    receiver: mpsc::UnboundedReceiver<ExecutionEvent>,
}

impl Stream for SseStream {
    type Item = Result<Event, Infallible>;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match this.receiver.poll_recv(context) {
            Poll::Ready(Some(event)) => Poll::Ready(Some(Ok(event_to_sse(event)))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

fn event_to_sse(event: ExecutionEvent) -> Event {
    let payload = serde_json::to_string(&event).unwrap_or_default();
    Event::default()
        .id(event.sequence.to_string())
        .event(event.kind)
        .data(payload)
}

async fn stream_events(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Sse<SseStream>, ApiError> {
    if state.db.get_execution(&id).await?.is_none() {
        return Err(ApiError::not_found(format!("execution `{id}` not found")));
    }
    let (sender, receiver) = mpsc::unbounded_channel();
    let db = state.db.clone();
    let execution_id = id.clone();
    tokio::spawn(async move {
        let mut last = 0i64;
        loop {
            match db.list_events(&execution_id, last, 200).await {
                Ok(events) if !events.is_empty() => {
                    for event in events {
                        last = event.sequence as i64;
                        if sender.send(event).is_err() {
                            return;
                        }
                    }
                }
                Ok(_) => {
                    let terminal = db
                        .get_execution(&execution_id)
                        .await
                        .ok()
                        .flatten()
                        .map(|row| {
                            matches!(row.status.as_str(), "succeeded" | "failed" | "cancelled")
                        })
                        .unwrap_or(true);
                    if terminal {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(300)).await;
                }
                Err(_) => return,
            }
        }
    });
    Ok(Sse::new(SseStream { receiver }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApprovalDecision {
    node_id: String,
    decision: String,
    form: Option<Value>,
}

async fn decide_approval(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(request): Json<ApprovalDecision>,
) -> Result<Json<ExecutionDetail>, ApiError> {
    let row = state
        .db
        .get_execution(&id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("execution `{id}` not found")))?;
    let mut execution = parse_state(&row)?;
    let workflow: Workflow = serde_json::from_str(&row.workflow_json)?;
    let compilation = compile_workflow(&workflow).map_err(|issues| {
        ApiError::internal(format!(
            "workflow snapshot does not compile: {}",
            serde_json::to_string(&issues).unwrap_or_default()
        ))
    })?;
    let events = resume_approval(
        &mut execution,
        &compilation.plan,
        &request.node_id,
        &request.decision,
        request.form.unwrap_or(json!({})),
    )
    .map_err(|error| ApiError::conflict("APPROVAL_FAILED", error.to_string()))?;
    state.db.save_execution(&execution, &events).await?;
    let remote = match &state.worker_queue {
        Some(queue) => queue
            .job_for_execution(&id)
            .await
            .map_err(worker_api_error)?,
        None => None,
    };
    if let Some(queue) = &state.worker_queue
        && remote.is_some()
    {
        let state_json = serde_json::to_string(&execution)?;
        queue
            .requeue_execution(&id, &state_json)
            .await
            .map_err(worker_api_error)?;
    } else {
        state.runtime.trigger(id.clone());
    }
    Ok(Json(ExecutionDetail {
        summary: ExecutionSummary::from_state(&execution),
        state: execution,
    }))
}

async fn cancel(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<ExecutionDetail>, ApiError> {
    let row = state
        .db
        .get_execution(&id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("execution `{id}` not found")))?;
    let mut execution = parse_state(&row)?;
    if execution.status == ExecutionStatus::Running {
        return Err(ApiError::conflict(
            "CANCEL_RUNNING",
            "cancel is only supported while waiting for approval in this milestone",
        ));
    }
    let events = cancel_execution(&mut execution)
        .map_err(|error| ApiError::conflict("CANCEL_FAILED", error.to_string()))?;
    state.db.save_execution(&execution, &events).await?;
    if let Some(queue) = &state.worker_queue {
        queue
            .cancel_execution_job(&id)
            .await
            .map_err(worker_api_error)?;
    }
    Ok(Json(ExecutionDetail {
        summary: ExecutionSummary::from_state(&execution),
        state: execution,
    }))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ApprovalSummary {
    execution_id: String,
    workflow_id: String,
    workflow_version: String,
    started_at: String,
    request: ApprovalRequest,
}

async fn list_approvals(
    State(state): State<ApiState>,
) -> Result<Json<Vec<ApprovalSummary>>, ApiError> {
    let rows = state.db.waiting_executions().await?;
    let mut approvals = Vec::new();
    for row in rows {
        let execution = parse_state(&row)?;
        if let Some(request) = execution.waiting.clone() {
            approvals.push(ApprovalSummary {
                execution_id: execution.id.clone(),
                workflow_id: execution.workflow_id.clone(),
                workflow_version: execution.workflow_version.clone(),
                started_at: execution.started_at.clone(),
                request,
            });
        }
    }
    Ok(Json(approvals))
}

#[derive(Debug, Deserialize, Default)]
struct ArtifactQuery {
    execution_id: Option<String>,
}

async fn list_artifacts(
    State(state): State<ApiState>,
    Query(query): Query<ArtifactQuery>,
) -> Result<Json<Vec<ArtifactRow>>, ApiError> {
    let artifacts = state
        .db
        .list_artifacts(query.execution_id.as_deref())
        .await?;
    Ok(Json(artifacts))
}

async fn get_artifact(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let artifact = state
        .db
        .get_artifact(&id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("artifact `{id}` not found")))?;
    let content: Value = serde_json::from_str(&artifact.content_json)?;
    Ok(Json(json!({"artifact": artifact, "content": content})))
}

async fn api_not_found() -> ApiError {
    ApiError::not_found("API route not found")
}

fn mcp_service(state: &ApiState) -> McpService {
    McpService::new(
        SkillService::new(state.db.clone(), state.data_dir.clone()),
        state.runtime.clone(),
    )
}

fn notify_mcp_catalog(state: &ApiState) {
    for kind in ["tools", "resources", "prompts"] {
        crate::mcp_http::notify_catalog_changed(&state.tenant_id, kind);
    }
}

fn rpc_result(response: Value) -> Result<Value, ApiError> {
    if let Some(error) = response.get("error") {
        let code = error
            .get("code")
            .and_then(Value::as_i64)
            .map(|code| code.to_string())
            .unwrap_or_else(|| "MCP_ERROR".to_string());
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("MCP request failed");
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            &code,
            message,
        ));
    }
    Ok(response.get("result").cloned().unwrap_or(Value::Null))
}

async fn mcp_overview(State(state): State<ApiState>) -> Result<Json<Value>, ApiError> {
    let service = mcp_service(&state);
    let (pending_approvals, published_skills, requests) = state.db.mcp_stats().await?;
    let tokens = state.db.list_mcp_tokens().await?;
    let servers = state.db.list_mcp_servers().await?;
    let tools = mcp::admin_request_quiet(&service, "tools/list", json!({})).await;
    let resources = mcp::admin_request_quiet(&service, "resources/list", json!({})).await;
    let prompts = mcp::admin_request_quiet(&service, "prompts/list", json!({})).await;
    Ok(Json(json!({
        "endpoint": format!("{}/mcp", state.mcp_base_url),
        "resourceMetadata": format!("{}/.well-known/oauth-protected-resource", state.mcp_base_url),
        "protocols": mcp::SUPPORTED_VERSIONS,
        "sandbox": sandbox_label(hpa_skill::detected_sandbox()),
        "runtimes": runtime_versions(),
        "stats": {
            "pendingApprovals": pending_approvals,
            "publishedSkills": published_skills,
            "requests": requests,
            "tokens": tokens.len(),
            "servers": servers.len(),
            "tools": tools.pointer("/result/tools").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
            "resources": resources.pointer("/result/resources").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
            "prompts": prompts.pointer("/result/prompts").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
        },
    })))
}

async fn mcp_catalog(State(state): State<ApiState>) -> Result<Json<Value>, ApiError> {
    let service = mcp_service(&state);
    let tools = rpc_result(mcp::admin_request_quiet(&service, "tools/list", json!({})).await)?;
    let resources =
        rpc_result(mcp::admin_request_quiet(&service, "resources/list", json!({})).await)?;
    let prompts = rpc_result(mcp::admin_request_quiet(&service, "prompts/list", json!({})).await)?;
    Ok(Json(json!({
        "tools": tools.get("tools").cloned().unwrap_or_else(|| json!([])),
        "resources": resources.get("resources").cloned().unwrap_or_else(|| json!([])),
        "prompts": prompts.get("prompts").cloned().unwrap_or_else(|| json!([])),
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpAdminCall {
    name: String,
    #[serde(default)]
    arguments: Value,
    #[serde(default)]
    request_state: Option<String>,
}

async fn mcp_call(
    State(state): State<ApiState>,
    Json(request): Json<McpAdminCall>,
) -> Result<Json<Value>, ApiError> {
    let service = mcp_service(&state);
    let params = json!({
        "name": request.name,
        "arguments": if request.arguments.is_null() { json!({}) } else { request.arguments },
        "requestState": request.request_state,
    });
    let result = rpc_result(mcp::admin_request(&service, "tools/call", params).await)?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
struct McpResourceRequest {
    uri: String,
}

async fn mcp_read_resource(
    State(state): State<ApiState>,
    Json(request): Json<McpResourceRequest>,
) -> Result<Json<Value>, ApiError> {
    let service = mcp_service(&state);
    let result = rpc_result(
        mcp::admin_request(&service, "resources/read", json!({"uri": request.uri})).await,
    )?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
struct McpPromptRequest {
    name: String,
    #[serde(default)]
    arguments: Value,
}

async fn mcp_get_prompt(
    State(state): State<ApiState>,
    Json(request): Json<McpPromptRequest>,
) -> Result<Json<Value>, ApiError> {
    let service = mcp_service(&state);
    let result = rpc_result(
        mcp::admin_request(
            &service,
            "prompts/get",
            json!({"name": request.name, "arguments": request.arguments}),
        )
        .await,
    )?;
    Ok(Json(result))
}

fn token_value(row: &crate::db::McpTokenRow) -> Value {
    json!({
        "id": row.id,
        "name": row.name,
        "scopes": mcp::token_scopes(row),
        "tenantId": row.tenant_id,
        "createdAt": row.created_at,
        "lastUsedAt": row.last_used_at,
        "revokedAt": row.revoked_at,
    })
}

async fn mcp_tokens(State(state): State<ApiState>) -> Result<Json<Value>, ApiError> {
    let rows = state.db.list_mcp_tokens().await?;
    Ok(Json(
        json!({"tokens": rows.iter().map(token_value).collect::<Vec<_>>()}),
    ))
}

#[derive(Debug, Deserialize)]
struct McpTokenRequest {
    name: String,
    #[serde(default)]
    scopes: Option<Vec<String>>,
}

async fn mcp_token_create(
    State(state): State<ApiState>,
    Json(request): Json<McpTokenRequest>,
) -> Result<Json<Value>, ApiError> {
    let scopes = request
        .scopes
        .unwrap_or_else(|| vec![mcp::SCOPE_READ.to_string(), mcp::SCOPE_EXECUTE.to_string()]);
    for scope in &scopes {
        if !matches!(scope.as_str(), mcp::SCOPE_READ | mcp::SCOPE_EXECUTE) {
            return Err(ApiError::bad_request(
                "MCP_SCOPE",
                format!("unknown scope `{scope}`"),
            ));
        }
    }
    let (token, row) = mcp::create_pat(
        &state.db,
        &request.name,
        &scopes,
        &state.tenant_id,
        &mcp::now_rfc3339(),
    )
    .await
    .map_err(|error| ApiError::internal(error.to_string()))?;
    Ok(Json(json!({
        "token": token,
        "record": token_value(&row),
    })))
}

async fn mcp_token_revoke(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let revoked = state.db.revoke_mcp_token(&id, &mcp::now_rfc3339()).await?;
    Ok(Json(json!({"revoked": revoked})))
}

fn approval_value(row: &crate::db::McpApprovalRow) -> Value {
    json!({
        "id": row.id,
        "tokenId": row.token_id,
        "toolName": row.tool_name,
        "arguments": serde_json::from_str::<Value>(&row.arguments_json).unwrap_or(Value::Null),
        "risk": row.risk,
        "status": row.status,
        "createdAt": row.created_at,
        "expiresAt": row.expires_at,
        "decidedAt": row.decided_at,
        "usedAt": row.used_at,
    })
}

#[derive(Debug, Deserialize)]
struct McpApprovalQuery {
    status: Option<String>,
    limit: Option<i64>,
}

async fn mcp_approvals(
    State(state): State<ApiState>,
    Query(query): Query<McpApprovalQuery>,
) -> Result<Json<Value>, ApiError> {
    let _ = state.db.expire_mcp_approvals(&mcp::now_rfc3339()).await;
    let rows = state
        .db
        .list_mcp_approvals(query.status.as_deref(), query.limit.unwrap_or(50))
        .await?;
    Ok(Json(json!({
        "approvals": rows.iter().map(approval_value).collect::<Vec<_>>(),
    })))
}

#[derive(Debug, Deserialize)]
struct McpApprovalDecision {
    decision: String,
}

async fn mcp_approval_decide(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(request): Json<McpApprovalDecision>,
) -> Result<Json<Value>, ApiError> {
    if !matches!(request.decision.as_str(), "approved" | "rejected") {
        return Err(ApiError::bad_request(
            "MCP_APPROVAL_DECISION",
            "decision must be approved or rejected",
        ));
    }
    let _ = state.db.expire_mcp_approvals(&mcp::now_rfc3339()).await;
    if state.db.get_mcp_approval(&id).await?.is_none() {
        return Err(ApiError::not_found(format!(
            "MCP approval `{id}` not found"
        )));
    }
    let decided = state
        .db
        .decide_mcp_approval(&id, &request.decision, &mcp::now_rfc3339())
        .await?;
    if !decided {
        return Err(ApiError::conflict(
            "MCP_APPROVAL_STATE",
            "approval is not pending",
        ));
    }
    Ok(Json(json!({"decided": true, "status": request.decision})))
}

#[derive(Debug, Deserialize)]
struct McpAuditQuery {
    limit: Option<i64>,
}

async fn mcp_audit(
    State(state): State<ApiState>,
    Query(query): Query<McpAuditQuery>,
) -> Result<Json<Value>, ApiError> {
    let rows = state.db.list_mcp_audit(query.limit.unwrap_or(50)).await?;
    let entries = rows
        .iter()
        .map(|row| {
            json!({
                "id": row.id,
                "at": row.at,
                "tokenId": row.token_id,
                "method": row.method,
                "toolName": row.tool_name,
                "status": row.status,
                "latencyMs": row.latency_ms,
                "detail": row.detail_json.as_ref().and_then(|value| serde_json::from_str::<Value>(value).ok()),
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({"audit": entries})))
}

fn server_value(row: &McpServerRow) -> Value {
    json!({
        "id": row.id,
        "name": row.name,
        "transport": row.transport,
        "command": serde_json::from_str::<Value>(&row.command_json).unwrap_or_else(|_| json!([])),
        "url": row.url,
        "allowedTools": serde_json::from_str::<Value>(&row.allowed_tools_json).unwrap_or_else(|_| json!([])),
        "network": serde_json::from_str::<Value>(&row.network_json).unwrap_or_else(|_| json!([])),
        "credentialEnv": row.credential_env,
        "createdAt": row.created_at,
        "updatedAt": row.updated_at,
    })
}

async fn mcp_servers(State(state): State<ApiState>) -> Result<Json<Value>, ApiError> {
    let rows = state.db.list_mcp_servers().await?;
    Ok(Json(
        json!({"servers": rows.iter().map(server_value).collect::<Vec<_>>()}),
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpServerRequest {
    id: Option<String>,
    name: String,
    transport: String,
    #[serde(default)]
    command: Vec<String>,
    url: Option<String>,
    #[serde(default)]
    allowed_tools: Vec<String>,
    #[serde(default)]
    network: Vec<String>,
    credential_env: Option<String>,
}

async fn mcp_server_upsert(
    State(state): State<ApiState>,
    Json(request): Json<McpServerRequest>,
) -> Result<Json<Value>, ApiError> {
    if !matches!(request.transport.as_str(), "stdio" | "http") {
        return Err(ApiError::bad_request(
            "MCP_TRANSPORT",
            "transport must be stdio or http",
        ));
    }
    if request.name.trim().is_empty() {
        return Err(ApiError::bad_request("MCP_SERVER_NAME", "name is required"));
    }
    if request.transport == "stdio" && request.command.is_empty() {
        return Err(ApiError::bad_request(
            "MCP_SERVER_COMMAND",
            "stdio servers require a command",
        ));
    }
    if request.transport == "http" {
        let Some(url) = request.url.as_deref() else {
            return Err(ApiError::bad_request(
                "MCP_SERVER_URL",
                "http servers require a url",
            ));
        };
        let parsed = reqwest::Url::parse(url)
            .map_err(|error| ApiError::bad_request("MCP_SERVER_URL", error.to_string()))?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            return Err(ApiError::bad_request(
                "MCP_SERVER_URL",
                "url must use http or https",
            ));
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(ApiError::bad_request(
                "MCP_SERVER_URL",
                "url must not contain userinfo",
            ));
        }
    }
    let now = now_rfc3339();
    let id = request
        .id
        .clone()
        .unwrap_or_else(|| format!("mcpsrv_{}", Uuid::now_v7()));
    let created_at = state
        .db
        .get_mcp_server(&id)
        .await?
        .map(|row| row.created_at)
        .unwrap_or_else(|| now.clone());
    let row = McpServerRow {
        id: id.clone(),
        name: request.name,
        transport: request.transport,
        command_json: serde_json::to_string(&request.command)
            .map_err(|error| ApiError::internal(error.to_string()))?,
        url: request.url,
        allowed_tools_json: serde_json::to_string(&request.allowed_tools)
            .map_err(|error| ApiError::internal(error.to_string()))?,
        network_json: serde_json::to_string(&request.network)
            .map_err(|error| ApiError::internal(error.to_string()))?,
        credential_env: request.credential_env,
        created_at,
        updated_at: now,
    };
    state.db.upsert_mcp_server(&row).await?;
    Ok(Json(json!({"server": server_value(&row)})))
}

async fn mcp_server_delete(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let deleted = state.db.delete_mcp_server(&id).await?;
    Ok(Json(json!({"deleted": deleted})))
}

async fn mcp_server_probe(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let row = state
        .db
        .get_mcp_server(&id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("MCP server `{id}` not found")))?;
    let server = ExternalServer::from_row(&row)
        .map_err(|error| ApiError::bad_request("MCP_SERVER_CONFIG", error))?;
    let result = mcp_client::probe(&server)
        .await
        .map_err(|error| ApiError::new(StatusCode::BAD_GATEWAY, "MCP_PROBE", error))?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
struct McpServerCall {
    tool: String,
    #[serde(default)]
    arguments: Value,
}

async fn mcp_server_call(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(request): Json<McpServerCall>,
) -> Result<Json<Value>, ApiError> {
    let row = state
        .db
        .get_mcp_server(&id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("MCP server `{id}` not found")))?;
    let server = ExternalServer::from_row(&row)
        .map_err(|error| ApiError::bad_request("MCP_SERVER_CONFIG", error))?;
    let result = mcp_client::call_tool(&server, &request.tool, request.arguments)
        .await
        .map_err(|error| ApiError::new(StatusCode::BAD_GATEWAY, "MCP_CALL", error))?;
    Ok(Json(result))
}

fn skill_service(state: &ApiState) -> SkillService {
    SkillService::new(state.db.clone(), state.data_dir.clone())
}

fn skill_version_value(row: &SkillVersionRow) -> Value {
    let manifest = serde_json::from_str::<SkillManifest>(&row.manifest_json).unwrap_or_default();
    json!({
        "version": row.version,
        "license": row.license,
        "status": row.status,
        "contentHash": row.content_hash,
        "importedAt": row.imported_at,
        "publishedAt": row.published_at,
        "scripts": manifest.scripts.len(),
        "tests": manifest.tests.len(),
        "entry": manifest.entry.kind(),
        "licenseAllowed": row.license.as_deref().is_some_and(hpa_skill::allowed_license),
    })
}

fn skill_detail_value(loaded: &LoadedSkill) -> Value {
    json!({
        "skill": {
            "id": loaded.snapshot.id,
            "name": loaded.snapshot.name,
            "version": loaded.snapshot.version,
            "license": loaded.snapshot.license,
            "status": loaded.version.status,
            "contentHash": loaded.snapshot.content_hash,
        },
        "summary": skill_version_value(&loaded.version),
        "manifest": loaded.snapshot.manifest,
        "skillMd": loaded.snapshot.skill_md,
        "report": loaded.report,
        "scripts": loaded.manifest.scripts,
        "tests": loaded.manifest.tests,
        "permissions": loaded.manifest.permissions,
        "entry": loaded.manifest.entry.kind(),
        "dependencies": loaded.manifest.dependencies,
        "files": loaded.snapshot.files.iter().map(|file| json!({
            "path": file.path,
            "size": file.size,
            "sha256": file.sha256,
        })).collect::<Vec<_>>(),
        "signature": loaded.signature,
        "sourcePath": loaded.version.source_path,
    })
}

fn skill_run_summary_value(row: &SkillRunRow) -> Value {
    json!({
        "id": row.id,
        "skillId": row.skill_id,
        "version": row.version,
        "scriptId": row.script_id,
        "status": row.status,
        "exitCode": row.exit_code,
        "sandbox": row.sandbox,
        "durationMs": row.duration_ms,
        "startedAt": row.started_at,
        "finishedAt": row.finished_at,
        "error": row.error_json.as_ref().and_then(|value| serde_json::from_str::<Value>(value).ok()),
    })
}

fn skill_run_detail_value(row: &SkillRunRow) -> Value {
    json!({
        "id": row.id,
        "skillId": row.skill_id,
        "version": row.version,
        "scriptId": row.script_id,
        "status": row.status,
        "exitCode": row.exit_code,
        "sandbox": row.sandbox,
        "durationMs": row.duration_ms,
        "startedAt": row.started_at,
        "finishedAt": row.finished_at,
        "input": serde_json::from_str::<Value>(&row.input_json).ok(),
        "output": row.output_json.as_ref().and_then(|value| serde_json::from_str::<Value>(value).ok()),
        "stdout": row.stdout,
        "stderr": row.stderr,
        "error": row.error_json.as_ref().and_then(|value| serde_json::from_str::<Value>(value).ok()),
    })
}

fn safe_header_name(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateAgentRequest {
    config: Value,
}

async fn list_agents(State(state): State<ApiState>) -> Result<Json<Value>, ApiError> {
    let rows = state.db.list_agents().await?;
    Ok(Json(json!({
        "agents": rows.iter().map(agent::agent_summary_value).collect::<Vec<_>>(),
    })))
}

async fn create_agent(
    State(state): State<ApiState>,
    Json(request): Json<CreateAgentRequest>,
) -> Result<Json<Value>, ApiError> {
    let (definition, content_hash) =
        agent::canonical_agent_value(&request.config).map_err(|issues| {
            ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "AGENT_INVALID",
                "agent configuration is invalid",
            )
            .with_details(json!(issues))
        })?;
    let config_json = serde_json::to_string(&definition)?;
    let created_at = now_rfc3339();
    let row = AgentVersionRow {
        agent_id: definition.id.clone(),
        version: definition.version.clone(),
        name: definition.name.clone(),
        description: definition.description.clone(),
        config_json,
        content_hash,
        status: "imported".to_string(),
        created_at,
        published_at: None,
    };
    let inserted = state.db.upsert_agent_version(&row).await?;
    let summary = state
        .db
        .get_agent(&row.agent_id)
        .await?
        .ok_or_else(|| ApiError::not_found("agent was not stored"))?;
    let stored = state
        .db
        .get_agent_version(&row.agent_id, &row.version)
        .await?
        .ok_or_else(|| ApiError::not_found("agent version was not stored"))?;
    Ok(Json(json!({
        "inserted": inserted,
        "summary": agent::agent_summary_value(&summary),
        "version": agent::agent_version_value(&stored),
        "config": definition,
        "contentHash": row.content_hash,
    })))
}

async fn get_agent(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let summary = state
        .db
        .get_agent(&id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("agent `{id}` not found")))?;
    let versions = state.db.list_agent_versions(&id).await?;
    let latest_config = versions
        .first()
        .and_then(|row| serde_json::from_str::<Value>(&row.config_json).ok());
    let published = state.db.latest_published_agent_version(&id).await?;
    Ok(Json(json!({
        "summary": agent::agent_summary_value(&summary),
        "versions": versions.iter().map(agent::agent_version_value).collect::<Vec<_>>(),
        "latestConfig": latest_config,
        "publishedVersion": published.as_ref().map(agent::agent_version_value),
    })))
}

async fn get_agent_version(
    State(state): State<ApiState>,
    Path((id, version)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let row = state
        .db
        .get_agent_version(&id, &version)
        .await?
        .ok_or_else(|| {
            ApiError::not_found(format!("agent `{id}` version `{version}` not found"))
        })?;
    let config: Value = serde_json::from_str(&row.config_json)?;
    Ok(Json(json!({
        "agentId": row.agent_id,
        "version": row.version,
        "name": row.name,
        "description": row.description,
        "contentHash": row.content_hash,
        "status": row.status,
        "createdAt": row.created_at,
        "publishedAt": row.published_at,
        "config": config,
    })))
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct PublishAgentRequest {
    version: Option<String>,
}

async fn publish_agent(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(request): Json<PublishAgentRequest>,
) -> Result<Json<Value>, ApiError> {
    let summary = state
        .db
        .get_agent(&id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("agent `{id}` not found")))?;
    let version = request.version.unwrap_or(summary.latest_version);
    let row = state
        .db
        .get_agent_version(&id, &version)
        .await?
        .ok_or_else(|| {
            ApiError::not_found(format!("agent `{id}` version `{version}` not found"))
        })?;
    let config: Value = serde_json::from_str(&row.config_json)?;
    let (_, hash) = agent::canonical_agent_value(&config).map_err(|issues| {
        ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "AGENT_INVALID",
            "stored agent configuration is invalid",
        )
        .with_details(json!(issues))
    })?;
    if hash != row.content_hash {
        return Err(ApiError::conflict(
            "AGENT_VERSION_FROZEN",
            format!("agent `{id}` version `{version}` content hash mismatch"),
        ));
    }
    let published_at = now_rfc3339();
    let changed = state
        .db
        .publish_agent_version(&id, &version, &published_at)
        .await?;
    let stored = state
        .db
        .get_agent_version(&id, &version)
        .await?
        .ok_or_else(|| ApiError::not_found("agent version disappeared"))?;
    let summary = state
        .db
        .get_agent(&id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("agent `{id}` not found")))?;
    Ok(Json(json!({
        "published": changed || stored.status == "published",
        "summary": agent::agent_summary_value(&summary),
        "version": agent::agent_version_value(&stored),
    })))
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct RunAgentRequest {
    version: Option<String>,
    input: Option<Value>,
}

async fn run_agent(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(request): Json<RunAgentRequest>,
) -> Result<Json<Value>, ApiError> {
    let session = state
        .agents
        .start(AgentStart {
            agent_id: id,
            version: request.version,
            input: request.input.unwrap_or_else(|| json!({})),
            parent_session_id: None,
            depth: 0,
            limit_override: None,
        })
        .await?;
    Ok(Json(agent_session_detail(&state, &session).await?))
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct AgentSessionQuery {
    agent_id: Option<String>,
    limit: Option<i64>,
}

async fn list_agent_sessions(
    State(state): State<ApiState>,
    Query(query): Query<AgentSessionQuery>,
) -> Result<Json<Value>, ApiError> {
    let rows = state
        .db
        .list_agent_sessions(query.agent_id.as_deref(), query.limit.unwrap_or(50))
        .await?;
    let mut sessions = Vec::with_capacity(rows.len());
    for row in &rows {
        sessions.push(agent::agent_session_summary(row)?);
    }
    Ok(Json(json!({"sessions": sessions})))
}

async fn get_agent_session(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let row = state
        .db
        .get_agent_session(&id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("agent session `{id}` not found")))?;
    Ok(Json(agent_session_detail(&state, &row).await?))
}

async fn list_agent_session_events(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    if state.db.get_agent_session(&id).await?.is_none() {
        return Err(ApiError::not_found(format!(
            "agent session `{id}` not found"
        )));
    }
    let events = state.db.list_agent_events(&id, 1_000).await?;
    Ok(Json(json!({
        "events": events.iter().map(agent::event_value).collect::<Vec<_>>(),
    })))
}

async fn continue_agent_session(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let row = state.agents.drive(&id).await?;
    Ok(Json(agent_session_detail(&state, &row).await?))
}

async fn cancel_agent_session(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let row = state.agents.cancel(&id).await?;
    Ok(Json(agent_session_detail(&state, &row).await?))
}

async fn replay_agent_session(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(state.agents.replay(&id).await?))
}

async fn list_agent_approvals(
    State(state): State<ApiState>,
    Query(query): Query<AgentApprovalQuery>,
) -> Result<Json<Value>, ApiError> {
    let rows = state
        .db
        .list_agent_approvals(query.status.as_deref(), query.limit.unwrap_or(50))
        .await?;
    Ok(Json(json!({
        "approvals": rows.iter().map(agent::approval_value).collect::<Vec<_>>(),
    })))
}

#[derive(Debug, Deserialize, Default)]
struct AgentApprovalQuery {
    status: Option<String>,
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DecideAgentApprovalRequest {
    decision: String,
}

async fn decide_agent_approval(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(request): Json<DecideAgentApprovalRequest>,
) -> Result<Json<Value>, ApiError> {
    if !matches!(request.decision.as_str(), "approved" | "rejected") {
        return Err(ApiError::bad_request(
            "AGENT_APPROVAL_DECISION",
            "decision must be approved or rejected",
        ));
    }
    let row: AgentApprovalRow = state
        .db
        .get_agent_approval(&id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("agent approval `{id}` not found")))?;
    if row.status != "pending" {
        return Err(ApiError::conflict(
            "AGENT_APPROVAL_DECIDED",
            format!("approval `{id}` is already `{}`", row.status),
        ));
    }
    let decided = state
        .db
        .decide_agent_approval(&id, &request.decision, &now_rfc3339())
        .await?;
    if !decided {
        return Err(ApiError::conflict(
            "AGENT_APPROVAL_DECIDED",
            "approval was already decided",
        ));
    }
    let session = state
        .agents
        .resume_approval(&row.session_id, &request.decision)
        .await?;
    let approval = state
        .db
        .get_agent_approval(&id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("agent approval `{id}` not found")))?;
    Ok(Json(json!({
        "approval": agent::approval_value(&approval),
        "session": agent_session_detail(&state, &session).await?,
    })))
}

async fn agent_runtime_info(State(state): State<ApiState>) -> Json<Value> {
    let mut planners = vec!["deterministic", "openAi"];
    if state.agents.python_worker_available() {
        planners.push("python");
    }
    Json(json!({
        "schemaVersion": agent::AGENT_SCHEMA,
        "sessionSchemaVersion": agent::AGENT_SESSION_SCHEMA,
        "approvalTtlSeconds": agent::APPROVAL_TTL_SECONDS,
        "defaultLimits": AgentLimits::default(),
        "planners": planners,
        "risk": {
            "workflow.execute": "high",
            "mcp": "high",
            "skill": "medium",
            "agent.handoff": "medium",
            "workflow.list": "low",
            "execution.get": "low",
            "execution.wait": "low"
        },
        "deterministicReplay": "trace-deterministic",
    }))
}

async fn agent_tools(State(state): State<ApiState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(state.agents.tool_catalog().await?))
}

async fn agent_tool_call(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(request): Json<agent::ToolInvokeRequest>,
) -> Result<Json<agent::ToolInvokeResponse>, ApiError> {
    if request.tenant_id != state.tenant_id {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "AGENT_TOOL_TENANT",
            "tool request tenant does not match the routed tenant",
        ));
    }
    let header_tenant = headers
        .get("x-hpa-tenant-id")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::FORBIDDEN,
                "AGENT_TOOL_TENANT",
                "x-hpa-tenant-id header is required",
            )
        })?;
    if header_tenant != state.tenant_id {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "AGENT_TOOL_TENANT",
            "tool request tenant header does not match the routed tenant",
        ));
    }
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::FORBIDDEN,
                "AGENT_TOOL_AUTH",
                "a session-bound tool capability is required",
            )
        })?;
    Ok(Json(
        state
            .agents
            .invoke_python_tool(token.trim(), request)
            .await?,
    ))
}

async fn agent_session_detail(state: &ApiState, row: &AgentSessionRow) -> Result<Value, ApiError> {
    let events: Vec<AgentEventRow> = state.db.list_agent_events(&row.id, 1_000).await?;
    let mut value = agent::agent_session_value(row)?;
    value["events"] = Value::Array(events.iter().map(agent::event_value).collect());
    Ok(value)
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct PluginPathRequest {
    path: String,
    #[serde(default)]
    allow_unsigned: bool,
    #[serde(default)]
    allow_untrusted: bool,
}

async fn list_plugins(State(state): State<ApiState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!({"plugins": state.plugins.list().await?})))
}

async fn plugin_key(State(state): State<ApiState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(state.plugins.key_info().await?))
}

async fn list_plugin_keys(State(state): State<ApiState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!({"keys": state.plugins.trusted_keys().await?})))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrustKeyRequest {
    key_id: String,
    public_key: String,
    #[serde(default)]
    label: String,
    #[serde(default = "default_verified")]
    trust: String,
}

fn default_verified() -> String {
    "verified".to_string()
}

async fn trust_plugin_key(
    State(state): State<ApiState>,
    Json(request): Json<TrustKeyRequest>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        state
            .plugins
            .trust_key(
                &request.key_id,
                &request.public_key,
                if request.label.is_empty() {
                    "trusted publisher"
                } else {
                    &request.label
                },
                &request.trust,
            )
            .await?,
    ))
}

fn bundle_options(request: &PluginPathRequest) -> BundleOptions {
    BundleOptions {
        allow_unsigned: request.allow_unsigned,
        allow_untrusted: request.allow_untrusted,
        require_signature: false,
    }
}

async fn inspect_plugin(
    State(state): State<ApiState>,
    Json(request): Json<PluginPathRequest>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        state
            .plugins
            .inspect(FsPath::new(&request.path), bundle_options(&request))
            .await?,
    ))
}

async fn install_plugin(
    State(state): State<ApiState>,
    Json(request): Json<PluginPathRequest>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        state
            .plugins
            .install(FsPath::new(&request.path), bundle_options(&request))
            .await?,
    ))
}

async fn create_sample_plugin(State(state): State<ApiState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(state.plugins.create_sample().await?))
}

async fn get_plugin(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(state.plugins.detail(&id).await?))
}

async fn get_plugin_version(
    State(state): State<ApiState>,
    Path((id, version)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(state.plugins.package_detail(&id, &version).await?))
}

async fn enable_plugin(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(state.plugins.enable(&id).await?))
}

async fn disable_plugin(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(state.plugins.disable(&id).await?))
}

async fn rollback_plugin(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(state.plugins.rollback(&id).await?))
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct UninstallPluginRequest {
    #[serde(default)]
    force: bool,
}

async fn uninstall_plugin(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(request): Json<UninstallPluginRequest>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(state.plugins.uninstall(&id, request.force).await?))
}

async fn marketplace_list(State(state): State<ApiState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        json!({"plugins": state.plugins.marketplace_list().await?}),
    ))
}

#[derive(Debug, Deserialize)]
struct RemoteMarketplaceQuery {
    source: Option<String>,
}

async fn remote_marketplace_packages(
    State(state): State<ApiState>,
    Query(query): Query<RemoteMarketplaceQuery>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!({
        "packages": state
            .plugins
            .remote_marketplace_packages(query.source.as_deref())
            .await?
    })))
}

struct MarketplaceSseStream {
    receiver: mpsc::Receiver<MarketplaceEventRow>,
}

impl Stream for MarketplaceSseStream {
    type Item = Result<Event, Infallible>;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match this.receiver.poll_recv(context) {
            Poll::Ready(Some(event)) => Poll::Ready(Some(Ok(marketplace_event_to_sse(event)))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

fn marketplace_event_to_sse(event: MarketplaceEventRow) -> Event {
    let payload = json!({
        "sequence": event.sequence,
        "id": event.id,
        "at": event.at,
        "pluginId": event.plugin_id,
        "version": event.version,
        "kind": event.kind,
        "detail": serde_json::from_str::<Value>(&event.detail_json).unwrap_or(Value::Null),
    });
    Event::default()
        .id(event.sequence.to_string())
        .event("marketplace")
        .data(payload.to_string())
}

#[derive(Debug, Deserialize, Default)]
struct MarketplaceEventQuery {
    since: Option<i64>,
}

async fn stream_marketplace_events(
    State(state): State<ApiState>,
    Query(query): Query<MarketplaceEventQuery>,
    headers: HeaderMap,
) -> Response {
    let header_sequence = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    let explicit = query
        .since
        .or((header_sequence > 0).then_some(header_sequence));
    let after = match explicit {
        Some(sequence) => sequence.max(0),
        None => state
            .db
            .latest_marketplace_event_sequence()
            .await
            .unwrap_or(0),
    };
    let (sender, receiver) = mpsc::channel(256);
    let db = state.db.clone();
    tokio::spawn(async move {
        let mut last = after;
        loop {
            match db.list_marketplace_events(last, 500).await {
                Ok(events) if !events.is_empty() => {
                    for event in events {
                        last = event.sequence;
                        if sender.try_send(event).is_err() {
                            return;
                        }
                    }
                }
                Ok(_) => tokio::time::sleep(Duration::from_millis(250)).await,
                Err(_) => return,
            }
        }
    });
    let mut response = Sse::new(MarketplaceSseStream { receiver })
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("hpa marketplace stream"),
        )
        .into_response();
    if let Ok(value) = HeaderValue::from_str(&after.to_string()) {
        response.headers_mut().insert("x-hpa-event-sequence", value);
    }
    response
}

async fn marketplace_event_cursor(State(state): State<ApiState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!({
        "sequence": state.db.latest_marketplace_event_sequence().await?
    })))
}

async fn marketplace_vulnerabilities(
    State(state): State<ApiState>,
    Path((id, version)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        state.plugins.vulnerability_report(&id, &version).await?,
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddMarketplacePublisherRequest {
    id: String,
    name: String,
}

async fn list_marketplace_publishers(
    State(state): State<ApiState>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!({
        "publishers": state.plugins.marketplace_publishers().await?
    })))
}

async fn add_marketplace_publisher(
    State(state): State<ApiState>,
    Json(request): Json<AddMarketplacePublisherRequest>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        state
            .plugins
            .add_publisher(&request.id, &request.name)
            .await?,
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PublisherKeyRequest {
    key_id: String,
    public_key: String,
    label: String,
    valid_from: Option<String>,
    valid_until: Option<String>,
}

async fn add_marketplace_publisher_key(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(request): Json<PublisherKeyRequest>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        state
            .plugins
            .add_publisher_key(
                &id,
                &request.key_id,
                &request.public_key,
                &request.label,
                request.valid_from.as_deref(),
                request.valid_until.as_deref(),
            )
            .await?,
    ))
}

async fn revoke_marketplace_publisher_key(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(request): Json<RevokePublisherKeyRequest>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        state
            .plugins
            .revoke_publisher_key(&id, &request.key_id)
            .await?,
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RevokePublisherKeyRequest {
    key_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddMarketplaceSourceRequest {
    id: String,
    label: String,
    index_uri: String,
    key_id: String,
    public_key: String,
    key_label: String,
}

async fn list_marketplace_sources(State(state): State<ApiState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!({
        "sources": state.plugins.marketplace_sources().await?
    })))
}

async fn add_marketplace_source(
    State(state): State<ApiState>,
    Json(request): Json<AddMarketplaceSourceRequest>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        state
            .plugins
            .add_marketplace_source(
                &request.id,
                &request.label,
                &request.index_uri,
                &request.key_id,
                &request.public_key,
                &request.key_label,
            )
            .await?,
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RotateMarketplaceSourceKeyRequest {
    key_id: String,
    public_key: String,
    label: String,
}

async fn rotate_marketplace_source_key(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(request): Json<RotateMarketplaceSourceKeyRequest>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        state
            .plugins
            .rotate_marketplace_source_key(
                &id,
                &request.key_id,
                &request.public_key,
                &request.label,
            )
            .await?,
    ))
}

async fn refresh_marketplace_source(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(state.plugins.refresh_marketplace_source(&id).await?))
}

async fn install_remote_marketplace_package(
    State(state): State<ApiState>,
    Path((source, id, version)): Path<(String, String, String)>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        state
            .plugins
            .install_remote_package(&source, &id, &version)
            .await?,
    ))
}

async fn publish_marketplace(
    State(state): State<ApiState>,
    Json(request): Json<PluginPathRequest>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        state
            .plugins
            .publish_marketplace(
                FsPath::new(&request.path),
                BundleOptions {
                    allow_unsigned: false,
                    allow_untrusted: true,
                    require_signature: true,
                },
            )
            .await?,
    ))
}

async fn marketplace_detail(
    State(state): State<ApiState>,
    Path((id, version)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(state.plugins.marketplace_detail(&id, &version).await?))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewMarketplaceRequest {
    status: String,
    note: Option<String>,
}

async fn review_marketplace(
    State(state): State<ApiState>,
    Path((id, version)): Path<(String, String)>,
    Json(request): Json<ReviewMarketplaceRequest>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        state
            .plugins
            .set_marketplace_listing(&id, &version, &request.status, request.note.as_deref())
            .await?,
    ))
}

async fn withdraw_marketplace(
    State(state): State<ApiState>,
    Path((id, version)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        state.plugins.withdraw_marketplace(&id, &version).await?,
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateMarketplaceRequest {
    rating: i64,
    comment: Option<String>,
}

async fn rate_marketplace(
    State(state): State<ApiState>,
    Path((id, version)): Path<(String, String)>,
    Json(request): Json<RateMarketplaceRequest>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        state
            .plugins
            .rate_marketplace(&id, &version, request.rating, request.comment.as_deref())
            .await?,
    ))
}

async fn install_marketplace(
    State(state): State<ApiState>,
    Path((id, version)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        state.plugins.install_marketplace(&id, &version).await?,
    ))
}

async fn list_connectors(State(state): State<ApiState>) -> Result<Json<Value>, ApiError> {
    let rows = state.connectors.list_definitions().await?;
    let definitions = rows
        .iter()
        .map(|row| {
            let mut value = connector::definition_value(row);
            value["configShape"] = connector_config_shape(&row.kind);
            value
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({"connectors": definitions})))
}

async fn get_connector(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let row = state.connectors.definition(&id, None).await?;
    let mut value = connector::definition_value(&row);
    value["configShape"] = connector_config_shape(&row.kind);
    Ok(Json(value))
}

async fn list_connections(State(state): State<ApiState>) -> Result<Json<Value>, ApiError> {
    let rows = state.connectors.list_connections().await?;
    let mut connections = Vec::new();
    for row in &rows {
        let definition = state
            .connectors
            .definition(&row.connector_id, Some(&row.connector_version))
            .await
            .ok();
        connections.push(connector::connection_value(row, definition.as_ref()));
    }
    Ok(Json(json!({"connections": connections})))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveConnectionRequest {
    id: Option<String>,
    name: String,
    connector_id: String,
    connector_version: Option<String>,
    config: Value,
    credential_env: Option<String>,
}

async fn save_connection(
    State(state): State<ApiState>,
    Json(request): Json<SaveConnectionRequest>,
) -> Result<Json<Value>, ApiError> {
    let row = state
        .connectors
        .save_connection(ConnectionInput {
            id: request.id,
            name: request.name,
            connector_id: request.connector_id,
            connector_version: request.connector_version,
            config: request.config,
            credential_env: request.credential_env,
        })
        .await?;
    let definition = state
        .connectors
        .definition(&row.connector_id, Some(&row.connector_version))
        .await
        .ok();
    Ok(Json(connector::connection_value(&row, definition.as_ref())))
}

async fn delete_connection(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!({
        "deleted": state.connectors.delete_connection(&id).await?,
    })))
}

async fn test_connection(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!({
        "connectionId": id,
        "test": state.connectors.test_connection(&id).await?,
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InvokeConnectionRequest {
    operation: String,
    #[serde(default)]
    arguments: Value,
}

async fn invoke_connection(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(request): Json<InvokeConnectionRequest>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!({
        "connectionId": id,
        "operation": request.operation,
        "result": state.connectors
            .invoke(&id, &request.operation, request.arguments)
            .await?,
    })))
}

async fn create_sample_connection(State(state): State<ApiState>) -> Result<Json<Value>, ApiError> {
    let existing = state
        .connectors
        .list_connections()
        .await?
        .into_iter()
        .find(|connection| connection.name == "Sample file workspace");
    if let Some(row) = existing {
        let definition = state
            .connectors
            .definition(&row.connector_id, Some(&row.connector_version))
            .await
            .ok();
        return Ok(Json(connector::connection_value(&row, definition.as_ref())));
    }
    let root = connector::seed_connection_workspace(&state.data_dir).await?;
    let row = state
        .connectors
        .save_connection(ConnectionInput {
            id: None,
            name: "Sample file workspace".to_string(),
            connector_id: "file".to_string(),
            connector_version: Some("1.0.0".to_string()),
            config: json!({"root": root, "allowWrite": true}),
            credential_env: None,
        })
        .await?;
    let definition = state
        .connectors
        .definition(&row.connector_id, Some(&row.connector_version))
        .await
        .ok();
    Ok(Json(connector::connection_value(&row, definition.as_ref())))
}

#[derive(Debug, Deserialize, Default)]
struct ConnectorAuditQuery {
    limit: Option<i64>,
}

async fn connector_audit(
    State(state): State<ApiState>,
    Query(query): Query<ConnectorAuditQuery>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!({
        "entries": state.connectors.list_audit(query.limit.unwrap_or(50)).await?,
    })))
}

fn connector_config_shape(kind: &str) -> Value {
    match kind {
        "http" => json!({
            "baseUrl": "http://127.0.0.1:8787",
            "allowedHosts": [],
            "auth": {"kind": "none"},
            "timeoutMs": 15000,
            "maxAttempts": 1,
            "rateLimitPerMinute": 60
        }),
        "file" => json!({
            "root": "/absolute/workspace/path",
            "allowWrite": false
        }),
        _ => json!({}),
    }
}

async fn list_skills(State(state): State<ApiState>) -> Result<Json<Value>, ApiError> {
    let skills: Vec<SkillSummaryRow> = state.db.list_skills().await?;
    Ok(Json(json!({
        "skills": skills,
        "runtimes": runtime_versions(),
        "sandbox": sandbox_label(hpa_skill::detected_sandbox()),
    })))
}

async fn skill_runtimes() -> Json<Value> {
    Json(json!({
        "runtimes": runtime_versions(),
        "sandbox": sandbox_label(hpa_skill::detected_sandbox()),
    }))
}

async fn get_skill(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let summary = state
        .db
        .get_skill(&id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("skill `{id}` not found")))?;
    let versions = state.db.list_skill_versions(&id).await?;
    Ok(Json(json!({
        "summary": summary,
        "versions": versions.iter().map(skill_version_value).collect::<Vec<_>>(),
        "runtimes": runtime_versions(),
        "sandbox": sandbox_label(hpa_skill::detected_sandbox()),
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportSkillRequest {
    path: String,
}

async fn import_skill(
    State(state): State<ApiState>,
    Json(request): Json<ImportSkillRequest>,
) -> Result<Json<Value>, ApiError> {
    let service = skill_service(&state);
    let outcome = service.import(FsPath::new(&request.path)).await?;
    let loaded = service
        .load(&outcome.summary.id, Some(&outcome.version.version))
        .await?;
    notify_mcp_catalog(&state);
    Ok(Json(json!({
        "inserted": outcome.inserted,
        "warnings": outcome.report.issues,
        "skill": skill_detail_value(&loaded),
    })))
}

async fn get_skill_version(
    State(state): State<ApiState>,
    Path((id, version)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let loaded = skill_service(&state).load(&id, Some(&version)).await?;
    Ok(Json(skill_detail_value(&loaded)))
}

async fn get_skill_source(
    State(state): State<ApiState>,
    Path((id, version)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let source = skill_service(&state).source(&id, Some(&version)).await?;
    Ok(Json(serde_json::to_value(source)?))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveSkillDraftRequest {
    version: String,
    files: BTreeMap<String, String>,
}

async fn save_skill_draft(
    State(state): State<ApiState>,
    Path((id, version)): Path<(String, String)>,
    Json(request): Json<SaveSkillDraftRequest>,
) -> Result<Json<Value>, ApiError> {
    let service = skill_service(&state);
    let outcome = service
        .save_draft(&id, &version, &request.version, request.files)
        .await?;
    let loaded = service
        .load(&outcome.summary.id, Some(&outcome.version.version))
        .await?;
    notify_mcp_catalog(&state);
    Ok(Json(json!({
        "inserted": outcome.inserted,
        "warnings": outcome.report.issues,
        "skill": skill_detail_value(&loaded),
    })))
}

async fn sign_skill(
    State(state): State<ApiState>,
    Path((id, version)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let signature = skill_service(&state).sign(&id, Some(&version)).await?;
    Ok(Json(json!({"signature": signature})))
}

async fn publish_skill(
    State(state): State<ApiState>,
    Path((id, version)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let published = skill_service(&state).publish(&id, Some(&version)).await?;
    notify_mcp_catalog(&state);
    Ok(Json(json!({"published": published})))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportSkillRequest {
    path: String,
}

async fn export_skill(
    State(state): State<ApiState>,
    Path((id, version)): Path<(String, String)>,
    Json(request): Json<ExportSkillRequest>,
) -> Result<Json<Value>, ApiError> {
    skill_service(&state)
        .export(&id, Some(&version), FsPath::new(&request.path))
        .await?;
    Ok(Json(json!({"exported": true, "path": request.path})))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunSkillRequest {
    input: Option<Value>,
    script: Option<String>,
}

async fn run_skill(
    State(state): State<ApiState>,
    Path((id, version)): Path<(String, String)>,
    Json(request): Json<RunSkillRequest>,
) -> Result<Json<Value>, ApiError> {
    let service = skill_service(&state);
    let outcome = service
        .run(
            &id,
            Some(&version),
            request.script.as_deref(),
            request.input.unwrap_or_else(|| json!({})),
        )
        .await?;
    let artifacts = service.artifacts(&outcome.id).await?;
    Ok(Json(json!({"run": outcome, "artifacts": artifacts})))
}

#[derive(Debug, Deserialize, Default)]
struct TestSkillRequest {
    case: Option<String>,
}

async fn test_skill(
    State(state): State<ApiState>,
    Path((id, version)): Path<(String, String)>,
    Json(request): Json<TestSkillRequest>,
) -> Result<Json<Value>, ApiError> {
    let results: Vec<TestResult> = skill_service(&state)
        .test(&id, Some(&version), request.case.as_deref())
        .await?;
    Ok(Json(json!({
        "passed": results.iter().all(|result| result.passed),
        "results": results,
    })))
}

#[derive(Debug, Deserialize)]
struct SkillRunListQuery {
    limit: Option<i64>,
}

async fn list_skill_runs(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Query(query): Query<SkillRunListQuery>,
) -> Result<Json<Value>, ApiError> {
    let runs = skill_service(&state)
        .runs(Some(&id), query.limit.unwrap_or(20))
        .await?;
    Ok(Json(json!({
        "runs": runs.iter().map(skill_run_summary_value).collect::<Vec<_>>(),
    })))
}

async fn get_skill_run(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let row = state
        .db
        .get_skill_run(&id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("skill run `{id}` not found")))?;
    let artifacts = state.db.list_skill_run_artifacts(&id).await?;
    Ok(Json(json!({
        "run": skill_run_detail_value(&row),
        "artifacts": artifacts,
    })))
}

async fn list_skill_run_artifacts(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let artifacts = state.db.list_skill_run_artifacts(&id).await?;
    Ok(Json(json!({"artifacts": artifacts})))
}

async fn get_skill_artifact(
    State(state): State<ApiState>,
    Path((run_id, artifact_id)): Path<(String, String)>,
) -> Result<(HeaderMap, Bytes), ApiError> {
    let (artifact, content) = skill_service(&state)
        .skill_artifact(&run_id, &artifact_id)
        .await?;
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&artifact.mime_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!(
            "attachment; filename=\"{}\"",
            safe_header_name(&artifact.name)
        ))
        .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );
    Ok((headers, Bytes::from(content)))
}

fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

// ---- triggers ----------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
struct TriggerListQuery {
    #[serde(default, rename = "workflowId")]
    workflow_id: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

fn trigger_error(error: crate::scheduler::TriggerError) -> ApiError {
    use crate::scheduler::TriggerError;
    match error {
        TriggerError::NotFound {
            workflow_id,
            trigger_id,
        } => ApiError::not_found(format!(
            "trigger `{trigger_id}` of workflow `{workflow_id}` was not found"
        )),
        TriggerError::Disabled { trigger_id } => ApiError::conflict(
            "TRIGGER_DISABLED",
            format!("trigger `{trigger_id}` is disabled"),
        ),
        TriggerError::Unsupported { trigger_id, kind } => ApiError::bad_request(
            "TRIGGER_UNSUPPORTED",
            format!("trigger `{trigger_id}` of kind `{kind}` cannot be fired"),
        ),
        TriggerError::Duplicate { event_id } => ApiError::conflict(
            "TRIGGER_DUPLICATE",
            format!("event `{event_id}` was already delivered"),
        ),
        TriggerError::NoSubscribers { topic } => ApiError::not_found(format!(
            "no enabled event trigger subscribes to topic `{topic}`"
        )),
        TriggerError::VersionMissing {
            workflow_id,
            version,
        } => ApiError::conflict(
            "TRIGGER_VERSION_MISSING",
            format!("workflow `{workflow_id}` has no published version `{version}`"),
        ),
        TriggerError::Invalid(message) => ApiError::bad_request("TRIGGER_INVALID", message),
        TriggerError::Runtime(message) => ApiError::internal(message),
        TriggerError::Db(error) => ApiError::internal(error.to_string()),
    }
}

async fn publish_event_api(
    State(state): State<ApiState>,
    Path(topic): Path<String>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let event_id = headers
        .get("x-hpa-event-id")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("evt_{}", Uuid::now_v7()));
    let host = state.trigger_host();
    if let Some(publisher) = state.event_bus.as_ref() {
        crate::scheduler::ensure_event_subscribers(&host, &topic)
            .await
            .map_err(trigger_error)?;
        let outcome = publisher
            .publish(&topic, &event_id, &payload)
            .await
            .map_err(|error| match error {
                crate::event_bus::EventBusError::IdempotencyConflict { event_id } => {
                    ApiError::conflict(
                        "EVENT_ID_CONFLICT",
                        format!("event `{event_id}` was already published with different content"),
                    )
                }
                other => ApiError::internal(other.to_string()),
            })?;
        return Ok((
            StatusCode::ACCEPTED,
            Json(json!({
                "topic": topic,
                "eventId": event_id,
                "messageId": outcome.message_id,
                "queued": outcome.queued,
                "status": if outcome.queued { "queued" } else { "duplicate" },
            })),
        ));
    }
    let (event_id, outcomes) = crate::scheduler::fire_event(&host, &topic, payload, Some(event_id))
        .await
        .map_err(trigger_error)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "topic": topic,
            "eventId": event_id,
            "executions": outcomes.iter().map(|outcome| json!({
                "executionId": outcome.execution_id,
                "workflowId": outcome.workflow_id,
                "workflowVersion": outcome.workflow_version,
                "triggerId": outcome.trigger_id,
                "kind": outcome.kind,
            })).collect::<Vec<_>>(),
        })),
    ))
}

fn trigger_value(row: &TriggerStateRow, state: &ApiState) -> Value {
    let trigger = hpa_core::ir::Trigger {
        id: row.trigger_id.clone(),
        trigger_type: match row.kind.as_str() {
            "webhook" => hpa_core::ir::TriggerType::Webhook,
            "event" => hpa_core::ir::TriggerType::Event,
            "mcp" => hpa_core::ir::TriggerType::Mcp,
            _ => hpa_core::ir::TriggerType::Schedule,
        },
        config: serde_json::from_str(&row.config_json).unwrap_or_else(|_| json!({})),
    };
    let mut schedule = Value::Null;
    let mut webhook = Value::Null;
    let mut event = Value::Null;
    match hpa_core::trigger::parse_trigger(&trigger) {
        Ok(hpa_core::trigger::TriggerSpec::Schedule(spec)) => {
            schedule = json!({
                "cron": spec.cron_raw,
                "timezone": spec.offset_text,
                "misfire": match spec.misfire {
                    hpa_core::trigger::MisfirePolicy::Skip => "skip",
                    hpa_core::trigger::MisfirePolicy::RunOnce => "run_once",
                    hpa_core::trigger::MisfirePolicy::CatchUp => "catch_up",
                },
                "maxCatchUp": spec.max_catch_up,
                "input": spec.input,
            });
        }
        Ok(hpa_core::trigger::TriggerSpec::Webhook(spec)) => {
            webhook = json!({
                "url": format!(
                    "{}/hooks/{}/{}/{}",
                    state.mcp_base_url, state.tenant_id, row.workflow_id, row.trigger_id
                ),
                "hasSecret": spec.secret.is_some(),
                "slug": spec.slug,
                "signatureHeader": "X-HPA-Signature",
            });
        }
        Ok(hpa_core::trigger::TriggerSpec::Event(spec)) => {
            event = json!({
                "topic": spec.topic,
                "publishUrl": format!(
                    "{}/api/events/{}",
                    state.mcp_base_url, spec.topic
                ),
            });
        }
        _ => {}
    }
    json!({
        "workflowId": row.workflow_id,
        "triggerId": row.trigger_id,
        "kind": row.kind,
        "enabled": row.enabled != 0,
        "workflowVersion": row.workflow_version,
        "schedule": schedule,
        "webhook": webhook,
        "event": event,
        "nextFireAt": row.next_fire_at,
        "lastFireAt": row.last_fire_at,
        "lastExecutionId": row.last_execution_id,
        "lastStatus": row.last_status,
        "updatedAt": row.updated_at,
    })
}

async fn list_triggers_api(
    State(state): State<ApiState>,
    Query(query): Query<TriggerListQuery>,
) -> Result<Json<Value>, ApiError> {
    let rows = state.db.list_triggers().await?;
    let values = rows
        .iter()
        .filter(|row| {
            query
                .workflow_id
                .as_deref()
                .is_none_or(|workflow_id| row.workflow_id == workflow_id)
        })
        .map(|row| trigger_value(row, &state))
        .collect::<Vec<_>>();
    Ok(Json(json!({
        "tenantId": state.tenant_id,
        "triggers": values,
    })))
}

async fn enable_trigger_api(
    State(state): State<ApiState>,
    Path((workflow_id, trigger_id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let next =
        crate::scheduler::set_enabled(&state.trigger_host(), &workflow_id, &trigger_id, true)
            .await
            .map_err(trigger_error)?;
    Ok(Json(json!({
        "workflowId": workflow_id,
        "triggerId": trigger_id,
        "enabled": true,
        "nextFireAt": next,
    })))
}

async fn disable_trigger_api(
    State(state): State<ApiState>,
    Path((workflow_id, trigger_id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    crate::scheduler::set_enabled(&state.trigger_host(), &workflow_id, &trigger_id, false)
        .await
        .map_err(trigger_error)?;
    Ok(Json(json!({
        "workflowId": workflow_id,
        "triggerId": trigger_id,
        "enabled": false,
        "nextFireAt": Value::Null,
    })))
}

#[derive(Debug, Deserialize, Default)]
struct FireTriggerRequest {
    #[serde(default)]
    input: Option<Value>,
}

async fn fire_trigger_api(
    State(state): State<ApiState>,
    Path((workflow_id, trigger_id)): Path<(String, String)>,
    body: Option<Json<FireTriggerRequest>>,
) -> Result<Json<Value>, ApiError> {
    let input = body
        .and_then(|Json(request)| request.input)
        .unwrap_or_else(|| json!({}));
    let outcome = crate::scheduler::fire_now(
        &state.trigger_host(),
        crate::scheduler::FireRequest {
            workflow_id,
            trigger_id,
            input,
            event_id: None,
            scheduled_at: None,
            origin: "manual".to_string(),
        },
    )
    .await
    .map_err(trigger_error)?;
    Ok(Json(json!({
        "executionId": outcome.execution_id,
        "workflowId": outcome.workflow_id,
        "workflowVersion": outcome.workflow_version,
        "triggerId": outcome.trigger_id,
        "kind": outcome.kind,
    })))
}

async fn list_trigger_fires_api(
    State(state): State<ApiState>,
    Query(query): Query<TriggerListQuery>,
) -> Result<Json<Value>, ApiError> {
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let fires = state.db.list_trigger_fires(limit).await?;
    Ok(Json(json!({ "fires": fires })))
}
