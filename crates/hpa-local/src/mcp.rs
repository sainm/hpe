use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use hpa_core::compiler::compile_workflow;
use hpa_core::ir::Workflow;
use hpa_core::runtime::{cancel_execution, new_state, start_event};
use hpa_skill::SkillManifest;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as AsyncBufReader};
use tracing::warn;
use uuid::Uuid;

use crate::db::{Db, DbError, ExecutionRow, McpApprovalRow, McpAuditRow, McpTokenRow};
use crate::driver::RuntimeManager;
use crate::skills::{SkillService, SkillServiceError};

pub const MODERN_VERSION: &str = "2026-07-28";
pub const LEGACY_VERSION: &str = "2025-11-25";
pub const TASKS_EXTENSION: &str = "io.modelcontextprotocol/tasks";
pub const SUPPORTED_VERSIONS: &[&str] =
    &[MODERN_VERSION, LEGACY_VERSION, "2025-06-18", "2024-11-05"];
pub const SCOPE_READ: &str = "mcp:read";
pub const SCOPE_EXECUTE: &str = "mcp:execute";
pub const APPROVAL_TTL_SECONDS: i64 = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Era {
    Modern,
    Legacy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolNameStyle {
    Canonical,
    Slug,
}

#[derive(Debug, Clone)]
pub struct AuthContext {
    pub token_id: String,
    pub tenant_id: String,
    pub scopes: Vec<String>,
    pub label: String,
}

impl AuthContext {
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|item| item == scope)
    }
}

#[derive(Clone)]
pub struct McpService {
    pub skills: SkillService,
    pub runtime: RuntimeManager,
}

impl McpService {
    pub fn new(skills: SkillService, runtime: RuntimeManager) -> Self {
        Self { skills, runtime }
    }

    fn db(&self) -> &Db {
        &self.skills.db
    }
}

#[derive(Debug, Clone)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }
}

#[derive(Debug)]
pub struct RequestOutcome {
    pub response: Option<Value>,
    pub new_session: Option<String>,
}

#[derive(Clone)]
struct ToolEntry {
    name: String,
    spec: Value,
    target: ToolTarget,
    scope: String,
    approval: bool,
}

#[derive(Clone)]
enum ToolTarget {
    Skill {
        skill_id: String,
        version: String,
        script_id: String,
    },
    WorkflowExecute,
    WorkflowList,
    ExecutionGet,
}

pub async fn serve_stdio(
    service: McpService,
    auth: AuthContext,
) -> Result<(), Box<dyn std::error::Error>> {
    let stdin = tokio::io::stdin();
    let mut lines = AsyncBufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();
    let mut era: Option<Era> = None;
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(line.trim()) {
            Ok(request) => request,
            Err(error) => {
                let response = rpc_error(
                    Value::Null,
                    &RpcError::new(-32700, format!("parse error: {error}")),
                );
                write_response(&mut stdout, &response).await?;
                continue;
            }
        };
        let request_era = detect_era(&request, era);
        let outcome = handle_request(&service, &auth, request_era, &request).await;
        if request_era == Era::Legacy
            && era.is_none()
            && request.get("method") == Some(&json!("initialize"))
        {
            era = Some(Era::Legacy);
        } else if request_era == Era::Modern {
            era = Some(Era::Modern);
        }
        if let Some(response) = outcome.response {
            write_response(&mut stdout, &response).await?;
        }
    }
    Ok(())
}

async fn write_response(
    stdout: &mut tokio::io::Stdout,
    response: &Value,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut encoded = serde_json::to_string(response)?;
    encoded.push('\n');
    stdout.write_all(encoded.as_bytes()).await?;
    stdout.flush().await?;
    Ok(())
}

pub fn detect_era(request: &Value, current: Option<Era>) -> Era {
    if request.get("method").and_then(Value::as_str) == Some("initialize") {
        return Era::Legacy;
    }
    if request
        .get("params")
        .and_then(|params| params.get("_meta"))
        .and_then(|meta| meta.get("io.modelcontextprotocol/protocolVersion"))
        .and_then(Value::as_str)
        == Some(MODERN_VERSION)
    {
        return Era::Modern;
    }
    current.unwrap_or(Era::Legacy)
}

pub async fn handle_request(
    service: &McpService,
    auth: &AuthContext,
    era: Era,
    request: &Value,
) -> RequestOutcome {
    handle_request_styled(service, auth, era, request, ToolNameStyle::Canonical).await
}

pub async fn handle_request_styled(
    service: &McpService,
    auth: &AuthContext,
    era: Era,
    request: &Value,
    tool_name_style: ToolNameStyle,
) -> RequestOutcome {
    handle_request_inner(service, auth, era, request, true, tool_name_style).await
}

async fn handle_request_inner(
    service: &McpService,
    auth: &AuthContext,
    era: Era,
    request: &Value,
    write_audit: bool,
    tool_name_style: ToolNameStyle,
) -> RequestOutcome {
    let started = Instant::now();
    let id = request.get("id").cloned();
    let Some(id) = id else {
        return RequestOutcome {
            response: None,
            new_session: None,
        };
    };
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let params = request.get("params").cloned().unwrap_or(Value::Null);
    let tool_name = method_tool_name(&method, &params);
    let mut new_session = None;
    let result = if method == "initialize" {
        new_session = Some(format!("mcpsess_{}", Uuid::now_v7()));
        initialize_result(&params)
    } else if era == Era::Modern {
        match validate_modern(&params) {
            Ok(()) => dispatch(service, auth, era, &method, &params, tool_name_style).await,
            Err(error) => Err(error),
        }
    } else {
        dispatch(service, auth, era, &method, &params, tool_name_style).await
    };
    let status = match &result {
        Ok(value) if value.get("resultType").and_then(Value::as_str) == Some("input_required") => {
            "approval_pending"
        }
        Ok(_) => "ok",
        Err(error) if error.code == -32003 => "denied",
        Err(_) => "error",
    };
    let latency_ms = started.elapsed().as_millis() as i64;
    let audit = McpAuditRow {
        id: format!("mcpaudit_{}", Uuid::now_v7()),
        at: now_rfc3339(),
        token_id: Some(auth.token_id.clone()),
        method: method.clone(),
        tool_name,
        status: status.to_string(),
        latency_ms,
        detail_json: Some(
            json!({
                "era": if era == Era::Modern { "modern" } else { "legacy" },
                "token": auth.label,
                "resultType": result.as_ref().ok().and_then(|value| value.get("resultType").cloned()),
            })
            .to_string(),
        ),
    };
    if write_audit && let Err(error) = service.db().insert_mcp_audit(&audit).await {
        warn!(error = %error, "cannot persist MCP audit");
    }
    let response = Some(match result {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(error) => rpc_error(id, &error),
    });
    RequestOutcome {
        response,
        new_session,
    }
}

fn rpc_error(id: Value, error: &RpcError) -> Value {
    let mut body = json!({"code": error.code, "message": error.message});
    if let Some(data) = &error.data {
        body["data"] = data.clone();
    }
    json!({"jsonrpc": "2.0", "id": id, "error": body})
}

fn method_tool_name(method: &str, params: &Value) -> Option<String> {
    match method {
        "tools/call" => params
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string),
        "prompts/get" => params
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string),
        "resources/read" => params
            .get("uri")
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => None,
    }
}

fn validate_modern(params: &Value) -> Result<(), RpcError> {
    let meta = params
        .get("_meta")
        .and_then(Value::as_object)
        .ok_or_else(|| RpcError::new(-32602, "missing request `_meta`"))?;
    let version = meta
        .get("io.modelcontextprotocol/protocolVersion")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::new(-32602, "missing protocol version in `_meta`"))?;
    if version != MODERN_VERSION {
        return Err(
            RpcError::new(-32022, format!("unsupported protocol version `{version}`"))
                .with_data(json!({"requested": version, "supported": SUPPORTED_VERSIONS})),
        );
    }
    if !meta
        .get("io.modelcontextprotocol/clientCapabilities")
        .is_some_and(Value::is_object)
    {
        return Err(RpcError::new(
            -32602,
            "missing clientCapabilities in `_meta`",
        ));
    }
    Ok(())
}

fn initialize_result(params: &Value) -> Result<Value, RpcError> {
    let requested = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(LEGACY_VERSION);
    if !SUPPORTED_VERSIONS.contains(&requested) {
        return Err(RpcError::new(
            -32001,
            format!("unsupported protocol version `{requested}`"),
        )
        .with_data(json!({"supportedVersions": SUPPORTED_VERSIONS})));
    }
    let protocol_version = if requested == MODERN_VERSION {
        LEGACY_VERSION
    } else {
        requested
    };
    let mut capabilities = json!({
        "tools": {"listChanged": false},
        "resources": {"subscribe": false, "listChanged": false},
        "prompts": {"listChanged": false},
    });
    if conformance_mode() {
        capabilities["logging"] = json!({});
    }
    Ok(json!({
        "protocolVersion": protocol_version,
        "capabilities": capabilities,
        "serverInfo": server_info(),
        "instructions": "hpa local MCP gateway. Tools run in the local sandbox; high-risk tools require approval.",
    }))
}

fn server_info() -> Value {
    json!({"name": "hpa-local", "version": env!("CARGO_PKG_VERSION")})
}

fn modern_meta() -> Value {
    json!({"io.modelcontextprotocol/serverInfo": server_info()})
}

pub(crate) fn finish(era: Era, mut value: Value) -> Value {
    if era == Era::Modern {
        value["resultType"] = json!("complete");
        let meta = value
            .as_object_mut()
            .map(|object| object.entry("_meta").or_insert_with(|| json!({})));
        if let Some(Value::Object(meta)) = meta {
            meta.insert(
                "io.modelcontextprotocol/serverInfo".to_string(),
                server_info(),
            );
        }
    }
    value
}

fn finish_list(era: Era, mut value: Value) -> Value {
    value = finish(era, value);
    if era == Era::Modern {
        value["cacheScope"] = json!("private");
        value["ttlMs"] = json!(30_000);
    }
    value
}

fn require_scope(auth: &AuthContext, scope: &str) -> Result<(), RpcError> {
    if auth.has_scope(scope) {
        Ok(())
    } else {
        Err(
            RpcError::new(-32003, format!("token is missing scope `{scope}`"))
                .with_data(json!({"requiredScope": scope})),
        )
    }
}

async fn dispatch(
    service: &McpService,
    auth: &AuthContext,
    era: Era,
    method: &str,
    params: &Value,
    tool_name_style: ToolNameStyle,
) -> Result<Value, RpcError> {
    match method {
        "server/discover" => {
            if era != Era::Modern {
                return Err(RpcError::new(-32601, "server/discover requires 2026-07-28"));
            }
            Ok(json!({
                "supportedVersions": SUPPORTED_VERSIONS,
                "capabilities": {
                    "tools": {"listChanged": true},
                    "resources": {"subscribe": false, "listChanged": true},
                    "prompts": {"listChanged": true},
                    "completions": {},
                    "extensions": {
                        (TASKS_EXTENSION): {
                            "get": true,
                            "update": true,
                            "cancel": true
                        }
                    }
                },
                "serverInfo": server_info(),
                "instructions": "hpa local MCP gateway.",
                "resultType": "complete",
                "cacheScope": "public",
                "ttlMs": 30000,
                "_meta": modern_meta(),
            }))
        }
        "ping" => Ok(finish(era, json!({}))),
        "logging/setLevel" => {
            if !conformance_mode() {
                return Err(RpcError::new(-32601, "logging/setLevel is not supported"));
            }
            let level = params
                .get("level")
                .and_then(Value::as_str)
                .ok_or_else(|| RpcError::new(-32602, "logging/setLevel requires level"))?;
            if ![
                "debug",
                "info",
                "notice",
                "warning",
                "error",
                "critical",
                "alert",
                "emergency",
            ]
            .contains(&level)
            {
                return Err(RpcError::new(-32602, "unsupported logging level"));
            }
            Ok(finish(era, json!({})))
        }
        "tools/list" => {
            require_scope(auth, SCOPE_READ)?;
            let tools = build_tools(service, auth).await?;
            let mut specs = conformance_mode()
                .then(conformance_tool_specs)
                .unwrap_or_default();
            specs.extend(tool_specs(&tools, tool_name_style));
            Ok(finish_list(era, json!({"tools": specs})))
        }
        "tools/call" => {
            if conformance_mode()
                && let Some(result) = conformance_tool_call(params, era, &auth.tenant_id)?
            {
                return Ok(result);
            }
            let tools = build_tools(service, auth).await?;
            call_tool(service, auth, era, params, &tools, tool_name_style).await
        }
        "resources/list" => {
            require_scope(auth, SCOPE_READ)?;
            let resources = list_resources(service, auth).await?;
            Ok(finish_list(era, json!({"resources": resources})))
        }
        "resources/templates/list" => {
            require_scope(auth, SCOPE_READ)?;
            Ok(finish_list(
                era,
                json!({"resourceTemplates": resource_templates(auth)}),
            ))
        }
        "resources/read" => {
            require_scope(auth, SCOPE_READ)?;
            let uri = params
                .get("uri")
                .and_then(Value::as_str)
                .ok_or_else(|| RpcError::new(-32602, "resources/read requires a uri"))?;
            let content = read_resource(service, auth, uri).await?;
            let mut result = json!({"contents": [content]});
            if era == Era::Modern {
                result["cacheScope"] = json!("private");
                result["ttlMs"] = json!(30000);
            }
            Ok(finish(era, result))
        }
        "prompts/list" => {
            require_scope(auth, SCOPE_READ)?;
            let prompts = list_prompts(service, auth).await?;
            Ok(finish_list(era, json!({"prompts": prompts})))
        }
        "prompts/get" => {
            require_scope(auth, SCOPE_READ)?;
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| RpcError::new(-32602, "prompts/get requires a name"))?;
            if conformance_mode()
                && era == Era::Modern
                && name == "test_input_required_result_prompt"
            {
                return mrtr_prompt(params, era);
            }
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
            let result = get_prompt(service, auth, name, &arguments).await?;
            Ok(finish(era, result))
        }
        "completion/complete" => {
            require_scope(auth, SCOPE_READ)?;
            Ok(finish(
                era,
                json!({
                    "completion": {
                        "values": [],
                        "hasMore": false
                    }
                }),
            ))
        }
        "tasks/get" => {
            require_scope(auth, SCOPE_READ)?;
            if era == Era::Modern {
                require_tasks_extension(params)?;
            }
            let task_id = params
                .get("taskId")
                .and_then(Value::as_str)
                .ok_or_else(|| RpcError::new(-32602, "tasks/get requires taskId"))?;
            if conformance_mode()
                && let Some(task) = conformance_task_by_id(task_id)
            {
                return Ok(conformance_task_detail(&task));
            }
            let task = mcp_task(service, task_id).await?;
            Ok(finish(era, task))
        }
        "tasks/update" => {
            require_scope(auth, SCOPE_EXECUTE)?;
            if era == Era::Modern {
                require_tasks_extension(params)?;
            }
            let task_id = params
                .get("taskId")
                .and_then(Value::as_str)
                .ok_or_else(|| RpcError::new(-32602, "tasks/update requires taskId"))?;
            if conformance_mode() && update_conformance_task(task_id, params, era)? {
                return Ok(finish(era, json!({})));
            }
            let update = params
                .get("update")
                .and_then(Value::as_object)
                .ok_or_else(|| RpcError::new(-32602, "tasks/update requires an update object"))?;
            if update.get("status").and_then(Value::as_str) != Some("cancelled") {
                return Err(RpcError::new(
                    -32602,
                    "only `{\"status\":\"cancelled\"}` is supported by this local task endpoint",
                ));
            }
            let row = service
                .db()
                .get_execution(task_id)
                .await
                .map_err(rpc_db)?
                .ok_or_else(|| RpcError::new(-32602, format!("task `{task_id}` not found")))?;
            let mut state = crate::db::parse_state(&row).map_err(rpc_db)?;
            if state.status != hpa_core::runtime::ExecutionStatus::WaitingApproval {
                return Err(RpcError::new(
                    -32602,
                    "task can only be cancelled while waiting for approval",
                ));
            }
            let events = cancel_execution(&mut state)
                .map_err(|error| RpcError::new(-32603, format!("cannot cancel task: {error}")))?;
            service
                .db()
                .save_execution(&state, &events)
                .await
                .map_err(rpc_db)?;
            let task = mcp_task(service, task_id).await?;
            Ok(finish(era, task))
        }
        "tasks/cancel" => {
            require_scope(auth, SCOPE_EXECUTE)?;
            if era == Era::Modern {
                require_tasks_extension(params)?;
            }
            let task_id = params
                .get("taskId")
                .and_then(Value::as_str)
                .ok_or_else(|| RpcError::new(-32602, "tasks/cancel requires taskId"))?;
            if conformance_mode() && cancel_conformance_task(task_id) {
                return Ok(finish(era, json!({})));
            }
            let row = service
                .db()
                .get_execution(task_id)
                .await
                .map_err(rpc_db)?
                .ok_or_else(|| RpcError::new(-32602, format!("task `{task_id}` not found")))?;
            let mut state = crate::db::parse_state(&row).map_err(rpc_db)?;
            if state.status != hpa_core::runtime::ExecutionStatus::WaitingApproval {
                return Err(RpcError::new(
                    -32602,
                    "task can only be cancelled while waiting for approval",
                ));
            }
            let events = cancel_execution(&mut state)
                .map_err(|error| RpcError::new(-32603, format!("cannot cancel task: {error}")))?;
            service
                .db()
                .save_execution(&state, &events)
                .await
                .map_err(rpc_db)?;
            Ok(finish(era, json!({})))
        }
        "notifications/initialized" | "notifications/cancelled" => Ok(json!({})),
        other => Err(RpcError::new(
            -32601,
            format!("method `{other}` is not supported"),
        )),
    }
}

fn conformance_mode() -> bool {
    std::env::var("HPA_MCP_CONFORMANCE")
        .map(|value| matches!(value.as_str(), "1" | "true"))
        .unwrap_or(false)
}

const CONFORMANCE_PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";
const CONFORMANCE_WAV: &str = "UklGRiQAAABXQVZFZm10IBAAAAABAAEAESsAACJWAAACABAAZGF0YQAAAAA=";

fn conformance_tool_specs() -> Vec<Value> {
    let mut specs = vec![
        json!({
            "name": "test_simple_text",
            "title": "Conformance simple text",
            "description": "Returns a deterministic text result for MCP conformance tests.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false
            },
            "_meta": {"hpa/conformanceFixture": true}
        }),
        json!({
            "name": "test_error_handling",
            "title": "Conformance tool error",
            "description": "Returns a deterministic tool error for MCP conformance tests.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false
            },
            "_meta": {"hpa/conformanceFixture": true}
        }),
        json!({
            "name": "test_missing_capability",
            "title": "Conformance missing capability",
            "description": "Requires sampling and returns MissingRequiredClientCapabilityError when undeclared.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false
            },
            "_meta": {"hpa/conformanceFixture": true}
        }),
        json!({
            "name": "test_streaming_elicitation",
            "title": "Conformance streaming elicitation",
            "description": "Returns a deterministic result for response-stream checks.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false
            },
            "_meta": {"hpa/conformanceFixture": true}
        }),
        json!({
            "name": "test_logging_tool",
            "title": "Conformance logging tool",
            "description": "Returns a result without emitting log messages unless a logLevel is supplied.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false
            },
            "_meta": {"hpa/conformanceFixture": true}
        }),
        json!({
            "name": "test_trigger_tool_change",
            "title": "Conformance tool change trigger",
            "description": "Returns a deterministic result for catalog-change subscription checks.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false
            },
            "_meta": {"hpa/conformanceFixture": true}
        }),
        json!({
            "name": "test_image_content",
            "title": "Conformance image content",
            "description": "Returns image content for conformance tests.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false},
            "_meta": {"hpa/conformanceFixture": true}
        }),
        json!({
            "name": "test_audio_content",
            "title": "Conformance audio content",
            "description": "Returns audio content for conformance tests.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false},
            "_meta": {"hpa/conformanceFixture": true}
        }),
        json!({
            "name": "test_embedded_resource",
            "title": "Conformance embedded resource",
            "description": "Returns embedded resource content for conformance tests.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false},
            "_meta": {"hpa/conformanceFixture": true}
        }),
        json!({
            "name": "test_multiple_content_types",
            "title": "Conformance mixed content",
            "description": "Returns text, image and resource content for conformance tests.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false},
            "_meta": {"hpa/conformanceFixture": true}
        }),
        json!({
            "name": "test_tool_with_progress",
            "title": "Conformance progress tool",
            "description": "Returns text and reports progress when a progressToken is provided.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false},
            "_meta": {"hpa/conformanceFixture": true}
        }),
        json!({
            "name": "json_schema_2020_12_tool",
            "title": "Conformance JSON Schema 2020-12",
            "description": "Tool with JSON Schema 2020-12 features",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "$defs": {
                    "address": {
                        "$anchor": "addressDef",
                        "type": "object",
                        "properties": {
                            "street": {"type": "string"},
                            "city": {"type": "string"}
                        }
                    }
                },
                "properties": {
                    "name": {"type": "string"},
                    "address": {"$ref": "#/$defs/address"},
                    "contactMethod": {"type": "string", "enum": ["phone", "email"]},
                    "phone": {"type": "string"},
                    "email": {"type": "string"}
                },
                "allOf": [{"anyOf": [{"required": ["phone"]}, {"required": ["email"]}]}],
                "if": {"properties": {"contactMethod": {"const": "phone"}}, "required": ["contactMethod"]},
                "then": {"required": ["phone"]},
                "else": {"required": ["email"]},
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false},
            "_meta": {"hpa/conformanceFixture": true}
        }),
        json!({
            "name": "test_custom_headers",
            "title": "Conformance custom headers",
            "description": "Validates Mcp-Param custom header forwarding for conformance tests.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "value": {"type": "string", "x-mcp-header": "Value"}
                },
                "required": ["value"],
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false},
            "_meta": {"hpa/conformanceFixture": true}
        }),
    ];
    for (name, description) in [
        (
            "test_input_required_result_elicitation",
            "Returns an InputRequiredResult for elicitation and completes after the retry.",
        ),
        (
            "test_input_required_result_sampling",
            "Returns an InputRequiredResult for sampling and completes after the retry.",
        ),
        (
            "test_input_required_result_list_roots",
            "Returns an InputRequiredResult for roots/list and completes after the retry.",
        ),
        (
            "test_input_required_result_request_state",
            "Returns an InputRequiredResult with an integrity-protected requestState.",
        ),
        (
            "test_input_required_result_multiple_inputs",
            "Returns an InputRequiredResult with elicitation, sampling and roots/list requests.",
        ),
        (
            "test_input_required_result_multi_round",
            "Returns successive InputRequiredResults with evolving requestState.",
        ),
        (
            "test_input_required_result_tampered_state",
            "Returns an InputRequiredResult whose requestState rejects tampering.",
        ),
        (
            "test_input_required_result_capabilities",
            "Returns InputRequiredResults filtered by the client capabilities.",
        ),
    ] {
        specs.push(json!({
            "name": name,
            "title": name,
            "description": description,
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false},
            "_meta": {"hpa/conformanceFixture": true}
        }));
    }
    specs.extend(task_conformance_tool_specs());
    specs
}

fn task_conformance_tool_specs() -> Vec<Value> {
    vec![
        json!({
            "name": "greet",
            "title": "Tasks sync fixture",
            "description": "Sync-only greeting used by the tasks extension conformance suite.",
            "inputSchema": {
                "type": "object",
                "properties": {"name": {"type": "string"}},
                "required": ["name"],
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false},
            "_meta": {"hpa/conformanceFixture": true, "hpa/taskSupport": "forbidden"}
        }),
        json!({
            "name": "slow_compute",
            "title": "Tasks optional fixture",
            "description": "Task-supporting computation that sleeps for `seconds`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "seconds": {"type": "integer"},
                    "label": {"type": "string"}
                },
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false},
            "_meta": {"hpa/conformanceFixture": true, "hpa/taskSupport": "optional"}
        }),
        json!({
            "name": "failing_job",
            "title": "Tasks required fixture",
            "description": "Task-supporting fixture that reports a tool execution error.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false},
            "_meta": {"hpa/conformanceFixture": true, "hpa/taskSupport": "required"}
        }),
        json!({
            "name": "protocol_error_job",
            "title": "Tasks protocol error fixture",
            "description": "Task-supporting fixture that fails with a protocol-level error.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false},
            "_meta": {"hpa/conformanceFixture": true, "hpa/taskSupport": "optional"}
        }),
        json!({
            "name": "confirm_delete",
            "title": "Tasks MRTR fixture",
            "description": "Task-supporting fixture that parks on an elicitation request.",
            "inputSchema": {
                "type": "object",
                "properties": {"filename": {"type": "string"}},
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false},
            "_meta": {"hpa/conformanceFixture": true, "hpa/taskSupport": "optional"}
        }),
        json!({
            "name": "multi_input",
            "title": "Tasks multiple input fixture",
            "description": "Task-supporting fixture with two simultaneous elicitation requests.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false},
            "_meta": {"hpa/conformanceFixture": true, "hpa/taskSupport": "optional"}
        }),
        json!({
            "name": "test_tool_with_task",
            "title": "Tasks MRTR composition fixture",
            "description": "Gathers elicitation input with MRTR and then escalates to a task.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false},
            "_meta": {"hpa/conformanceFixture": true, "hpa/taskSupport": "required"}
        }),
    ]
}

fn conformance_tool_call(
    params: &Value,
    era: Era,
    tenant_id: &str,
) -> Result<Option<Value>, RpcError> {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return Ok(None);
    };
    match name {
        "test_simple_text" => Ok(Some(finish(
            era,
            json!({
                "content": [{
                    "type": "text",
                    "text": "HPA MCP conformance fixture response"
                }],
                "isError": false
            }),
        ))),
        "test_error_handling" => Ok(Some(finish(
            era,
            json!({
                "content": [{
                    "type": "text",
                    "text": "This tool intentionally returns an error for testing"
                }],
                "isError": true
            }),
        ))),
        "test_missing_capability" => {
            let declared = params
                .get("_meta")
                .and_then(|meta| meta.get("io.modelcontextprotocol/clientCapabilities"))
                .and_then(|capabilities| capabilities.get("sampling"))
                .is_some_and(Value::is_object);
            if declared {
                return Ok(Some(finish(
                    era,
                    json!({
                        "content": [{
                            "type": "text",
                            "text": "Sampling capability declared"
                        }],
                        "isError": false
                    }),
                )));
            }
            Err(
                RpcError::new(-32021, "sampling capability was not declared")
                    .with_data(json!({"requiredCapabilities": {"sampling": {}}})),
            )
        }
        "test_streaming_elicitation" => Ok(Some(finish(
            era,
            json!({
                "content": [{
                    "type": "text",
                    "text": "Streaming elicitation conformance fixture"
                }],
                "isError": false
            }),
        ))),
        "test_logging_tool" => Ok(Some(finish(
            era,
            json!({
                "content": [{
                    "type": "text",
                    "text": "Logging conformance fixture"
                }],
                "isError": false
            }),
        ))),
        "test_trigger_tool_change" => Ok(Some(finish(
            era,
            json!({
                "content": [{
                    "type": "text",
                    "text": "Catalog change conformance fixture"
                }],
                "isError": false
            }),
        ))),
        "test_image_content" => Ok(Some(finish(
            era,
            json!({
                "content": [{
                    "type": "image",
                    "data": CONFORMANCE_PNG,
                    "mimeType": "image/png"
                }],
                "isError": false
            }),
        ))),
        "test_audio_content" => Ok(Some(finish(
            era,
            json!({
                "content": [{
                    "type": "audio",
                    "data": CONFORMANCE_WAV,
                    "mimeType": "audio/wav"
                }],
                "isError": false
            }),
        ))),
        "test_embedded_resource" => Ok(Some(finish(
            era,
            json!({
                "content": [{
                    "type": "resource",
                    "resource": {
                        "uri": "test://embedded-resource",
                        "mimeType": "text/plain",
                        "text": "This is an embedded resource content."
                    }
                }],
                "isError": false
            }),
        ))),
        "test_multiple_content_types" => Ok(Some(finish(
            era,
            json!({
                "content": [
                    {"type": "text", "text": "Multiple content types test:"},
                    {"type": "image", "data": CONFORMANCE_PNG, "mimeType": "image/png"},
                    {
                        "type": "resource",
                        "resource": {
                            "uri": "test://mixed-content-resource",
                            "mimeType": "application/json",
                            "text": "{\"test\":\"data\",\"value\":123}"
                        }
                    }
                ],
                "isError": false
            }),
        ))),
        "test_tool_with_progress" => Ok(Some(finish(
            era,
            json!({
                "content": [{
                    "type": "text",
                    "text": "Progress conformance fixture completed"
                }],
                "isError": false
            }),
        ))),
        "json_schema_2020_12_tool" => Ok(Some(finish(
            era,
            json!({
                "content": [{
                    "type": "text",
                    "text": "JSON Schema 2020-12 conformance fixture"
                }],
                "isError": false
            }),
        ))),
        "test_custom_headers" => Ok(Some(finish(
            era,
            json!({
                "content": [{
                    "type": "text",
                    "text": "Custom header conformance fixture"
                }],
                "isError": false
            }),
        ))),
        name if name.starts_with("test_input_required_result_") => {
            Ok(Some(mrtr_tool_call(name, params, era)?))
        }
        name if is_task_fixture(name) => {
            Ok(Some(task_fixture_tool_call(name, params, era, tenant_id)?))
        }
        _ => Ok(None),
    }
}

fn is_task_fixture(name: &str) -> bool {
    matches!(
        name,
        "greet"
            | "slow_compute"
            | "failing_job"
            | "protocol_error_job"
            | "confirm_delete"
            | "multi_input"
            | "test_tool_with_task"
    )
}

fn mrtr_tool_call(name: &str, params: &Value, era: Era) -> Result<Value, RpcError> {
    match name {
        "test_input_required_result_elicitation" => mrtr_elicitation(params, era),
        "test_input_required_result_sampling" => mrtr_sampling(params, era),
        "test_input_required_result_list_roots" => mrtr_list_roots(params, era),
        "test_input_required_result_request_state" => mrtr_request_state(params, era),
        "test_input_required_result_multiple_inputs" => mrtr_multiple_inputs(params, era),
        "test_input_required_result_multi_round" => mrtr_multi_round(params, era),
        "test_input_required_result_tampered_state" => mrtr_tampered_state(params, era),
        "test_input_required_result_capabilities" => mrtr_capabilities(params, era),
        other => Err(RpcError::new(
            -32601,
            format!("unknown input_required fixture `{other}`"),
        )),
    }
}

fn mrtr_secret() -> &'static str {
    static SECRET: OnceLock<String> = OnceLock::new();
    SECRET.get_or_init(|| format!("mrtr-{}", Uuid::now_v7()))
}

fn mrtr_tag(payload: &str) -> String {
    let key = mrtr_secret().as_bytes();
    let mut block = [0u8; 64];
    if key.len() > 64 {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = [0x36u8; 64];
    let mut outer_pad = [0x5cu8; 64];
    for index in 0..64 {
        inner_pad[index] ^= block[index];
        outer_pad[index] ^= block[index];
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(payload.as_bytes());
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner);
    format!("{:x}", outer.finalize())
}

fn mrtr_equal(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

fn mrtr_state(label: &str) -> String {
    let payload = format!("{label}.{}", Uuid::now_v7());
    let tag = mrtr_tag(&payload);
    format!("{payload}.{tag}")
}

fn mrtr_state_valid(state: Option<&str>, label: &str) -> bool {
    let Some(state) = state else {
        return false;
    };
    let Some((payload, tag)) = state.rsplit_once('.') else {
        return false;
    };
    if !payload.starts_with(&format!("{label}.")) {
        return false;
    }
    mrtr_equal(&mrtr_tag(payload), tag)
}

fn mrtr_responses(params: &Value) -> Result<Option<&serde_json::Map<String, Value>>, RpcError> {
    match params.get("inputResponses") {
        None => Ok(None),
        Some(Value::Object(responses)) => Ok(Some(responses)),
        Some(_) => Err(RpcError::new(-32602, "inputResponses must be an object")),
    }
}

fn elicitation_request_value(message: &str, property: &str, kind: &str) -> Value {
    let mut properties = serde_json::Map::new();
    properties.insert(property.to_string(), json!({"type": kind}));
    json!({
        "method": "elicitation/create",
        "params": {
            "message": message,
            "requestedSchema": {
                "type": "object",
                "properties": properties,
                "required": [property]
            }
        }
    })
}

fn elicitation_request(key: &str, message: &str, property: &str, kind: &str) -> (String, Value) {
    (
        key.to_string(),
        elicitation_request_value(message, property, kind),
    )
}

fn mrtr_input_required(requests: Vec<(String, Value)>, state: Option<String>) -> Value {
    let mut result = json!({
        "resultType": "input_required",
        "inputRequests": Value::Object(requests.into_iter().collect()),
        "_meta": modern_meta(),
    });
    if let Some(state) = state {
        result["requestState"] = json!(state);
    }
    result
}

fn mrtr_complete_text(era: Era, text: String) -> Value {
    finish(
        era,
        json!({
            "content": [{"type": "text", "text": text}],
            "isError": false
        }),
    )
}

fn elicitation_content<'a>(
    responses: &'a serde_json::Map<String, Value>,
    key: &str,
    field: &str,
) -> Option<&'a Value> {
    responses
        .get(key)
        .and_then(Value::as_object)
        .and_then(|response| response.get("content"))
        .and_then(Value::as_object)
        .and_then(|content| content.get(field))
}

fn mrtr_elicitation(params: &Value, era: Era) -> Result<Value, RpcError> {
    let request = || elicitation_request("user_name", "What is your name?", "name", "string");
    let Some(responses) = mrtr_responses(params)? else {
        return Ok(mrtr_input_required(vec![request()], None));
    };
    let Some(name) = elicitation_content(responses, "user_name", "name").and_then(Value::as_str)
    else {
        return Ok(mrtr_input_required(vec![request()], None));
    };
    Ok(mrtr_complete_text(era, format!("Hello, {name}!")))
}

fn sampling_request(key: &str, prompt: &str, max_tokens: i64) -> (String, Value) {
    (
        key.to_string(),
        json!({
            "method": "sampling/createMessage",
            "params": {
                "messages": [{
                    "role": "user",
                    "content": {"type": "text", "text": prompt}
                }],
                "maxTokens": max_tokens
            }
        }),
    )
}

fn roots_request(key: &str) -> (String, Value) {
    (
        key.to_string(),
        json!({"method": "roots/list", "params": {}}),
    )
}

fn mrtr_sampling(params: &Value, era: Era) -> Result<Value, RpcError> {
    let request = || sampling_request("capital_question", "What is the capital of France?", 100);
    let Some(responses) = mrtr_responses(params)? else {
        return Ok(mrtr_input_required(vec![request()], None));
    };
    let Some(text) = responses
        .get("capital_question")
        .and_then(Value::as_object)
        .and_then(|response| response.get("content"))
        .and_then(|content| content.get("text"))
        .and_then(Value::as_str)
    else {
        return Ok(mrtr_input_required(vec![request()], None));
    };
    Ok(mrtr_complete_text(
        era,
        format!("Sampling response: {text}"),
    ))
}

fn mrtr_list_roots(params: &Value, era: Era) -> Result<Value, RpcError> {
    let request = || roots_request("client_roots");
    let Some(responses) = mrtr_responses(params)? else {
        return Ok(mrtr_input_required(vec![request()], None));
    };
    let Some(roots) = responses
        .get("client_roots")
        .and_then(Value::as_object)
        .and_then(|response| response.get("roots"))
        .and_then(Value::as_array)
    else {
        return Ok(mrtr_input_required(vec![request()], None));
    };
    let uris = roots
        .iter()
        .filter_map(|root| root.get("uri").and_then(Value::as_str))
        .collect::<Vec<_>>();
    Ok(mrtr_complete_text(
        era,
        format!("Client roots: {}", uris.join(", ")),
    ))
}

fn mrtr_request_state(params: &Value, era: Era) -> Result<Value, RpcError> {
    let request = || elicitation_request("confirm", "Please confirm", "ok", "boolean");
    let Some(responses) = mrtr_responses(params)? else {
        return Ok(mrtr_input_required(
            vec![request()],
            Some(mrtr_state("request-state")),
        ));
    };
    if !mrtr_state_valid(
        params.get("requestState").and_then(Value::as_str),
        "request-state",
    ) {
        return Err(RpcError::new(
            -32602,
            "requestState failed integrity verification",
        ));
    }
    if responses
        .get("confirm")
        .and_then(Value::as_object)
        .is_none()
    {
        return Ok(mrtr_input_required(
            vec![request()],
            Some(mrtr_state("request-state")),
        ));
    }
    Ok(mrtr_complete_text(era, "state-ok: confirmed".to_string()))
}

fn mrtr_multiple_requests() -> Vec<(String, Value)> {
    vec![
        elicitation_request("user_name", "What is your name?", "name", "string"),
        sampling_request("greeting", "Generate a greeting", 50),
        roots_request("client_roots"),
    ]
}

fn mrtr_multiple_inputs(params: &Value, era: Era) -> Result<Value, RpcError> {
    let Some(responses) = mrtr_responses(params)? else {
        return Ok(mrtr_input_required(
            mrtr_multiple_requests(),
            Some(mrtr_state("multiple-inputs")),
        ));
    };
    if !mrtr_state_valid(
        params.get("requestState").and_then(Value::as_str),
        "multiple-inputs",
    ) {
        return Err(RpcError::new(
            -32602,
            "requestState failed integrity verification",
        ));
    }
    let missing = mrtr_multiple_requests()
        .into_iter()
        .filter(|(key, _)| responses.get(key).and_then(Value::as_object).is_none())
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Ok(mrtr_input_required(
            missing,
            Some(mrtr_state("multiple-inputs")),
        ));
    }
    Ok(mrtr_complete_text(
        era,
        "All input requests answered".to_string(),
    ))
}

fn mrtr_multi_round(params: &Value, era: Era) -> Result<Value, RpcError> {
    let state = params.get("requestState").and_then(Value::as_str);
    let step1 = || elicitation_request("step1", "Step 1: What is your name?", "name", "string");
    let step2 = || {
        elicitation_request(
            "step2",
            "Step 2: What is your favorite color?",
            "color",
            "string",
        )
    };
    let Some(responses) = mrtr_responses(params)? else {
        return Ok(mrtr_input_required(
            vec![step1()],
            Some(mrtr_state("multi-round-1")),
        ));
    };
    if mrtr_state_valid(state, "multi-round-1") {
        if responses.get("step1").and_then(Value::as_object).is_none() {
            return Ok(mrtr_input_required(
                vec![step1()],
                Some(mrtr_state("multi-round-1")),
            ));
        }
        return Ok(mrtr_input_required(
            vec![step2()],
            Some(mrtr_state("multi-round-2")),
        ));
    }
    if mrtr_state_valid(state, "multi-round-2") {
        if responses.get("step2").and_then(Value::as_object).is_none() {
            return Ok(mrtr_input_required(
                vec![step2()],
                Some(mrtr_state("multi-round-2")),
            ));
        }
        let color = elicitation_content(responses, "step2", "color").and_then(Value::as_str);
        return Ok(mrtr_complete_text(
            era,
            format!("Favorite color: {}", color.unwrap_or_default()),
        ));
    }
    Err(RpcError::new(
        -32602,
        "requestState failed integrity verification",
    ))
}

fn mrtr_tampered_state(params: &Value, era: Era) -> Result<Value, RpcError> {
    let request = || elicitation_request("confirm", "Please confirm", "ok", "boolean");
    let Some(responses) = mrtr_responses(params)? else {
        return Ok(mrtr_input_required(
            vec![request()],
            Some(mrtr_state("tampered")),
        ));
    };
    if !mrtr_state_valid(
        params.get("requestState").and_then(Value::as_str),
        "tampered",
    ) {
        return Err(RpcError::new(
            -32602,
            "requestState failed integrity verification",
        ));
    }
    if responses
        .get("confirm")
        .and_then(Value::as_object)
        .is_none()
    {
        return Ok(mrtr_input_required(
            vec![request()],
            Some(mrtr_state("tampered")),
        ));
    }
    Ok(mrtr_complete_text(
        era,
        "Signed requestState accepted".to_string(),
    ))
}

fn mrtr_capabilities(params: &Value, era: Era) -> Result<Value, RpcError> {
    let capabilities = params
        .get("_meta")
        .and_then(|meta| meta.get("io.modelcontextprotocol/clientCapabilities"))
        .and_then(Value::as_object);
    let declared = |name: &str| {
        capabilities
            .and_then(|capabilities| capabilities.get(name))
            .is_some_and(Value::is_object)
    };
    let mut requests = Vec::new();
    if declared("elicitation") {
        requests.push(elicitation_request(
            "user_name",
            "What is your name?",
            "name",
            "string",
        ));
    }
    if declared("sampling") {
        requests.push(sampling_request("greeting", "Generate a greeting", 50));
    }
    if declared("roots") {
        requests.push(roots_request("client_roots"));
    }
    let Some(responses) = mrtr_responses(params)? else {
        if requests.is_empty() {
            return Ok(mrtr_complete_text(
                era,
                "No input capabilities were declared".to_string(),
            ));
        }
        return Ok(mrtr_input_required(requests, None));
    };
    let missing = requests
        .into_iter()
        .filter(|(key, _)| responses.get(key).and_then(Value::as_object).is_none())
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Ok(mrtr_input_required(missing, None));
    }
    Ok(mrtr_complete_text(
        era,
        "Declared capabilities answered".to_string(),
    ))
}

fn mrtr_prompt(params: &Value, era: Era) -> Result<Value, RpcError> {
    let request = || {
        elicitation_request(
            "user_context",
            "What context should the prompt use?",
            "context",
            "string",
        )
    };
    let Some(responses) = mrtr_responses(params)? else {
        return Ok(mrtr_input_required(vec![request()], None));
    };
    let Some(context) =
        elicitation_content(responses, "user_context", "context").and_then(Value::as_str)
    else {
        return Ok(mrtr_input_required(vec![request()], None));
    };
    Ok(finish(
        era,
        json!({
            "description": "Prompt built from MRTR input.",
            "messages": [{
                "role": "user",
                "content": {"type": "text", "text": format!("Prompt context: {context}")}
            }]
        }),
    ))
}

#[derive(Clone)]
struct ConformanceTask {
    tenant_id: String,
    id: String,
    status: String,
    created_at: String,
    updated_at: String,
    ttl_ms: i64,
    poll_interval_ms: i64,
    result: Option<Value>,
    error: Option<Value>,
    input_requests: BTreeMap<String, Value>,
    completion_text: Option<String>,
    completion_is_error: bool,
}

fn conformance_task_store() -> &'static Mutex<HashMap<String, ConformanceTask>> {
    static TASKS: OnceLock<Mutex<HashMap<String, ConformanceTask>>> = OnceLock::new();
    TASKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn new_conformance_task(
    tenant_id: &str,
    status: &str,
    input_requests: BTreeMap<String, Value>,
    completion_text: Option<String>,
    completion_is_error: bool,
) -> ConformanceTask {
    let now = now_rfc3339();
    ConformanceTask {
        tenant_id: tenant_id.to_string(),
        id: format!("task_{}", Uuid::now_v7()),
        status: status.to_string(),
        created_at: now.clone(),
        updated_at: now,
        ttl_ms: 60_000,
        poll_interval_ms: 100,
        result: None,
        error: None,
        input_requests,
        completion_text,
        completion_is_error,
    }
}

fn conformance_task_create_result(task: &ConformanceTask) -> Value {
    json!({
        "resultType": "task",
        "taskId": task.id,
        "status": task.status,
        "createdAt": task.created_at,
        "lastUpdatedAt": task.updated_at,
        "ttlMs": task.ttl_ms,
        "pollIntervalMs": task.poll_interval_ms,
        "_meta": modern_meta(),
    })
}

fn conformance_task_detail(task: &ConformanceTask) -> Value {
    let mut detail = json!({
        "resultType": "complete",
        "taskId": task.id,
        "status": task.status,
        "createdAt": task.created_at,
        "lastUpdatedAt": task.updated_at,
        "ttlMs": task.ttl_ms,
        "pollIntervalMs": task.poll_interval_ms,
        "_meta": modern_meta(),
    });
    if let Some(result) = &task.result {
        detail["result"] = result.clone();
    }
    if let Some(error) = &task.error {
        detail["error"] = error.clone();
    }
    if !task.input_requests.is_empty() {
        detail["inputRequests"] = Value::Object(
            task.input_requests
                .clone()
                .into_iter()
                .collect::<serde_json::Map<String, Value>>(),
        );
    }
    detail
}

fn task_notification_payload(task: &ConformanceTask) -> Value {
    let mut payload = json!({
        "taskId": task.id,
        "status": task.status,
        "updatedAt": task.updated_at,
    });
    if let Some(result) = &task.result {
        payload["result"] = result.clone();
    }
    if let Some(error) = &task.error {
        payload["error"] = error.clone();
    }
    payload
}

fn store_conformance_task(task: ConformanceTask) -> Value {
    let notification = task_notification_payload(&task);
    let tenant_id = task.tenant_id.clone();
    let create = conformance_task_create_result(&task);
    let stored = if let Ok(mut tasks) = conformance_task_store().lock() {
        tasks.insert(task.id.clone(), task);
        true
    } else {
        false
    };
    if stored {
        crate::mcp_http::notify_task_changed(&tenant_id, notification);
    }
    create
}

fn conformance_task_by_id(task_id: &str) -> Option<ConformanceTask> {
    conformance_task_store()
        .lock()
        .ok()
        .and_then(|tasks| tasks.get(task_id).cloned())
}

fn spawn_conformance_task_completion(
    task_id: String,
    delay_ms: u64,
    result: Option<Value>,
    error: Option<Value>,
) {
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
        let Ok(mut tasks) = conformance_task_store().lock() else {
            return;
        };
        let Some(task) = tasks.get_mut(&task_id) else {
            return;
        };
        if task.status != "working" {
            return;
        }
        task.status = if error.is_some() {
            "failed"
        } else {
            "completed"
        }
        .to_string();
        task.result = result;
        task.error = error;
        task.updated_at = now_rfc3339();
        let notification = task_notification_payload(task);
        let tenant_id = task.tenant_id.clone();
        drop(tasks);
        crate::mcp_http::notify_task_changed(&tenant_id, notification);
    });
}

fn tasks_extension_declared(params: &Value) -> bool {
    params
        .get("_meta")
        .and_then(|meta| meta.get("io.modelcontextprotocol/clientCapabilities"))
        .and_then(|capabilities| capabilities.get("extensions"))
        .and_then(|extensions| extensions.get(TASKS_EXTENSION))
        .is_some_and(Value::is_object)
}

fn tasks_extension_error() -> RpcError {
    let mut extensions = serde_json::Map::new();
    extensions.insert(TASKS_EXTENSION.to_string(), json!({}));
    let mut required = serde_json::Map::new();
    required.insert("extensions".to_string(), Value::Object(extensions));
    RpcError::new(
        -32021,
        "the io.modelcontextprotocol/tasks extension is required",
    )
    .with_data(json!({"requiredCapabilities": Value::Object(required)}))
}

pub(crate) fn require_tasks_extension(params: &Value) -> Result<(), RpcError> {
    if tasks_extension_declared(params) {
        Ok(())
    } else {
        Err(tasks_extension_error())
    }
}

fn task_fixture_tool_call(
    name: &str,
    params: &Value,
    era: Era,
    tenant_id: &str,
) -> Result<Value, RpcError> {
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    match name {
        "greet" => {
            let who = arguments
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("World");
            Ok(finish(
                era,
                json!({
                    "content": [{"type": "text", "text": format!("Hello, {who}!")}],
                    "isError": false
                }),
            ))
        }
        "slow_compute" => {
            let seconds = arguments
                .get("seconds")
                .and_then(Value::as_i64)
                .unwrap_or(0)
                .max(0);
            let label = arguments
                .get("label")
                .and_then(Value::as_str)
                .unwrap_or("compute");
            let text = format!("Computed {label}");
            if !tasks_extension_declared(params) {
                return Ok(finish(
                    era,
                    json!({
                        "content": [{"type": "text", "text": text}],
                        "isError": false
                    }),
                ));
            }
            let task = new_conformance_task(tenant_id, "working", BTreeMap::new(), None, false);
            let task_id = task.id.clone();
            let create = store_conformance_task(task);
            let result = finish(
                era,
                json!({
                    "content": [{"type": "text", "text": text}],
                    "isError": false
                }),
            );
            spawn_conformance_task_completion(task_id, seconds as u64 * 1000, Some(result), None);
            Ok(create)
        }
        "failing_job" => {
            if !tasks_extension_declared(params) {
                return Err(tasks_extension_error());
            }
            let task = new_conformance_task(tenant_id, "working", BTreeMap::new(), None, false);
            let task_id = task.id.clone();
            let create = store_conformance_task(task);
            let result = finish(
                era,
                json!({
                    "content": [{"type": "text", "text": "failing_job reported a tool error"}],
                    "isError": true
                }),
            );
            spawn_conformance_task_completion(task_id, 800, Some(result), None);
            Ok(create)
        }
        "protocol_error_job" => {
            if !tasks_extension_declared(params) {
                return Err(tasks_extension_error());
            }
            let task = new_conformance_task(tenant_id, "working", BTreeMap::new(), None, false);
            let task_id = task.id.clone();
            let create = store_conformance_task(task);
            spawn_conformance_task_completion(
                task_id,
                800,
                None,
                Some(json!({
                    "code": -32603,
                    "message": "protocol_error_job failed"
                })),
            );
            Ok(create)
        }
        "confirm_delete" => {
            if !tasks_extension_declared(params) {
                return Err(tasks_extension_error());
            }
            let filename = arguments
                .get("filename")
                .and_then(Value::as_str)
                .unwrap_or("file");
            let mut requests = BTreeMap::new();
            requests.insert(
                "confirm_delete".to_string(),
                elicitation_request_value(&format!("Delete {filename}?"), "confirm", "boolean"),
            );
            let task = new_conformance_task(
                tenant_id,
                "input_required",
                requests,
                Some(format!("Deleted {filename}")),
                false,
            );
            Ok(store_conformance_task(task))
        }
        "multi_input" => {
            if !tasks_extension_declared(params) {
                return Err(tasks_extension_error());
            }
            let mut requests = BTreeMap::new();
            requests.insert(
                "first".to_string(),
                elicitation_request_value("First input?", "name", "string"),
            );
            requests.insert(
                "second".to_string(),
                elicitation_request_value("Second input?", "name", "string"),
            );
            let task = new_conformance_task(
                tenant_id,
                "input_required",
                requests,
                Some("Both inputs received".to_string()),
                false,
            );
            Ok(store_conformance_task(task))
        }
        "test_tool_with_task" => {
            if !tasks_extension_declared(params) {
                return Err(tasks_extension_error());
            }
            let Some(responses) = mrtr_responses(params)? else {
                return Ok(mrtr_input_required(
                    vec![elicitation_request(
                        "user_name",
                        "What is your name?",
                        "name",
                        "string",
                    )],
                    None,
                ));
            };
            let Some(user_name) =
                elicitation_content(responses, "user_name", "name").and_then(Value::as_str)
            else {
                return Ok(mrtr_input_required(
                    vec![elicitation_request(
                        "user_name",
                        "What is your name?",
                        "name",
                        "string",
                    )],
                    None,
                ));
            };
            let task = new_conformance_task(tenant_id, "working", BTreeMap::new(), None, false);
            let task_id = task.id.clone();
            let create = store_conformance_task(task);
            let result = finish(
                era,
                json!({
                    "content": [{"type": "text", "text": format!("Hello, {user_name}!")}],
                    "isError": false
                }),
            );
            spawn_conformance_task_completion(task_id, 50, Some(result), None);
            Ok(create)
        }
        other => Err(RpcError::new(
            -32601,
            format!("unknown tasks fixture `{other}`"),
        )),
    }
}

fn update_conformance_task(task_id: &str, params: &Value, era: Era) -> Result<bool, RpcError> {
    let Some(mut task) = conformance_task_by_id(task_id) else {
        return Ok(false);
    };
    let answered = params
        .get("inputResponses")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut changed = false;
    if !answered.is_empty() {
        let before = task.input_requests.len();
        for key in answered.keys() {
            task.input_requests.remove(key);
        }
        changed = task.input_requests.len() != before;
        task.updated_at = now_rfc3339();
        if task.input_requests.is_empty() && task.status == "input_required" {
            task.status = "completed".to_string();
            if let Some(text) = task.completion_text.clone() {
                task.result = Some(finish(
                    era,
                    json!({
                        "content": [{"type": "text", "text": text}],
                        "isError": task.completion_is_error
                    }),
                ));
            }
            task.updated_at = now_rfc3339();
            changed = true;
        }
    }
    let notification = changed.then(|| task_notification_payload(&task));
    let tenant_id = task.tenant_id.clone();
    if let Ok(mut tasks) = conformance_task_store().lock() {
        tasks.insert(task_id.to_string(), task);
    }
    if let Some(notification) = notification {
        crate::mcp_http::notify_task_changed(&tenant_id, notification);
    }
    Ok(true)
}

fn cancel_conformance_task(task_id: &str) -> bool {
    let Ok(mut tasks) = conformance_task_store().lock() else {
        return false;
    };
    let Some(task) = tasks.get_mut(task_id) else {
        return false;
    };
    if !matches!(task.status.as_str(), "working" | "input_required") {
        return true;
    }
    task.status = "cancelled".to_string();
    task.updated_at = now_rfc3339();
    let notification = task_notification_payload(task);
    let tenant_id = task.tenant_id.clone();
    drop(tasks);
    crate::mcp_http::notify_task_changed(&tenant_id, notification);
    true
}

async fn build_tools(service: &McpService, auth: &AuthContext) -> Result<Vec<ToolEntry>, RpcError> {
    let mut tools = Vec::new();
    for skill in service.db().list_skills().await.map_err(rpc_db)? {
        let Some(version) = service
            .db()
            .get_skill_version(&skill.id, &skill.latest_version)
            .await
            .map_err(rpc_db)?
        else {
            continue;
        };
        if version.status != "published" {
            continue;
        }
        let manifest =
            serde_json::from_str::<SkillManifest>(&version.manifest_json).unwrap_or_default();
        for script in &manifest.scripts {
            let name = format!("skill.{}.{}", skill.name, script.id);
            let input_schema = script
                .input_schema
                .clone()
                .unwrap_or_else(|| json!({"type": "object", "additionalProperties": true}));
            let mut spec = json!({
                "name": name,
                "title": format!("{} · {}", skill.name, script.id),
                "description": format!("Run `{}@{}` script `{}` in the local sandbox.", skill.id, version.version, script.id),
                "inputSchema": input_schema,
                "annotations": {
                    "readOnlyHint": false,
                    "destructiveHint": false,
                    "idempotentHint": true,
                    "openWorldHint": false,
                },
            });
            if let Some(output_schema) = &script.output_schema {
                spec["outputSchema"] = output_schema.clone();
            }
            spec["_meta"] = json!({
                "hpa/risk": "medium",
                "hpa/approval": "never",
                "hpa/source": "skill",
                "hpa/tenant": auth.tenant_id,
            });
            tools.push(ToolEntry {
                name,
                spec,
                target: ToolTarget::Skill {
                    skill_id: skill.id.clone(),
                    version: version.version.clone(),
                    script_id: script.id.clone(),
                },
                scope: SCOPE_EXECUTE.to_string(),
                approval: false,
            });
        }
    }
    let published = service
        .db()
        .list_workflows()
        .await
        .map_err(rpc_db)?
        .into_iter()
        .filter(|workflow| workflow.published != 0)
        .collect::<Vec<_>>();
    if !published.is_empty() {
        tools.push(ToolEntry {
            name: "workflow.execute".to_string(),
            spec: json!({
                "name": "workflow.execute",
                "title": "Execute published workflow",
                "description": "Start a published workflow version. High risk: requires local approval.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "workflowId": {"type": "string"},
                        "version": {"type": "string"},
                        "input": {"type": "object"}
                    },
                    "required": ["workflowId", "version"],
                    "additionalProperties": false
                },
                "outputSchema": {
                    "type": "object",
                    "properties": {
                        "executionId": {"type": "string"},
                        "status": {"type": "string"}
                    },
                    "required": ["executionId", "status"]
                },
                "annotations": {
                    "readOnlyHint": false,
                    "destructiveHint": false,
                    "idempotentHint": false,
                    "openWorldHint": false,
                },
                "_meta": {"hpa/risk": "high", "hpa/approval": "always", "hpa/source": "workflow", "hpa/tenant": auth.tenant_id},
            }),
            target: ToolTarget::WorkflowExecute,
            scope: SCOPE_EXECUTE.to_string(),
            approval: true,
        });
    }
    tools.push(ToolEntry {
        name: "workflow.list".to_string(),
        spec: json!({
            "name": "workflow.list",
            "title": "List published workflows",
            "description": "List published workflow ids, names and versions.",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false},
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false,
            },
            "_meta": {"hpa/risk": "low", "hpa/approval": "never", "hpa/source": "workflow", "hpa/tenant": auth.tenant_id},
        }),
        target: ToolTarget::WorkflowList,
        scope: SCOPE_READ.to_string(),
        approval: false,
    });
    tools.push(ToolEntry {
        name: "execution.get".to_string(),
        spec: json!({
            "name": "execution.get",
            "title": "Get execution status",
            "description": "Return status, output and error for an execution.",
            "inputSchema": {
                "type": "object",
                "properties": {"executionId": {"type": "string"}},
                "required": ["executionId"],
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false,
            },
            "_meta": {"hpa/risk": "low", "hpa/approval": "never", "hpa/source": "execution", "hpa/tenant": auth.tenant_id},
        }),
        target: ToolTarget::ExecutionGet,
        scope: SCOPE_READ.to_string(),
        approval: false,
    });
    tools.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(tools)
}

fn tool_specs(tools: &[ToolEntry], style: ToolNameStyle) -> Vec<Value> {
    let mut used = HashSet::new();
    tools
        .iter()
        .map(|tool| {
            let mut spec = tool.spec.clone();
            if style == ToolNameStyle::Slug {
                let wire_name = wire_tool_name(&tool.name, &mut used);
                spec["name"] = Value::String(wire_name);
                if let Some(meta) = spec.get_mut("_meta").and_then(Value::as_object_mut) {
                    meta.insert(
                        "hpa/canonicalName".to_string(),
                        Value::String(tool.name.clone()),
                    );
                }
            }
            spec
        })
        .collect()
}

fn find_tool<'a>(
    tools: &'a [ToolEntry],
    name: &str,
    style: ToolNameStyle,
) -> Option<&'a ToolEntry> {
    match style {
        ToolNameStyle::Canonical => tools.iter().find(|tool| tool.name == name),
        ToolNameStyle::Slug => {
            let mut used = HashSet::new();
            tools.iter().find(|tool| {
                let wire_name = wire_tool_name(&tool.name, &mut used);
                wire_name == name
            })
        }
    }
}

fn wire_tool_name(canonical: &str, used: &mut HashSet<String>) -> String {
    let base = canonical
        .chars()
        .map(|character| {
            if character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || character == '_'
                || character == '-'
            {
                character
            } else if character.is_ascii_uppercase() {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    let mut candidate = base.clone();
    let mut suffix = 2;
    while !used.insert(candidate.clone()) {
        candidate = format!("{base}_{suffix}");
        suffix += 1;
    }
    candidate
}

async fn call_tool(
    service: &McpService,
    auth: &AuthContext,
    era: Era,
    params: &Value,
    tools: &[ToolEntry],
    tool_name_style: ToolNameStyle,
) -> Result<Value, RpcError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::new(-32602, "tools/call requires a name"))?;
    let entry = find_tool(tools, name, tool_name_style)
        .ok_or_else(|| RpcError::new(-32602, format!("tool `{name}` is not registered")))?;
    require_scope(auth, &entry.scope)?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if !arguments.is_object() {
        return Err(RpcError::new(-32602, "tool arguments must be an object"));
    }
    validate_tool_input(entry, &arguments)?;
    if entry.approval {
        if era == Era::Legacy {
            return Ok(tool_result(
                era,
                None,
                "high-risk tools require MCP 2026-07-28 approval".to_string(),
                true,
                json!({"hpa/errorCode": "APPROVAL_REQUIRED"}),
            ));
        }
        if let Some(response) = approval_gate(service, auth, params, entry, &arguments).await? {
            return Ok(response);
        }
    }
    execute_tool(service, auth, era, entry, &arguments).await
}

fn validate_tool_input(entry: &ToolEntry, arguments: &Value) -> Result<(), RpcError> {
    let Some(schema) = entry.spec.get("inputSchema") else {
        return Ok(());
    };
    let validator = jsonschema::validator_for(schema)
        .map_err(|error| RpcError::new(-32603, format!("tool schema is invalid: {error}")))?;
    if validator.is_valid(arguments) {
        return Ok(());
    }
    let message = validator
        .iter_errors(arguments)
        .take(3)
        .map(|error| format!("{}: {error}", error.instance_path()))
        .collect::<Vec<_>>()
        .join("; ");
    Err(RpcError::new(
        -32602,
        format!("arguments do not match inputSchema: {message}"),
    ))
}

async fn approval_gate(
    service: &McpService,
    auth: &AuthContext,
    params: &Value,
    entry: &ToolEntry,
    arguments: &Value,
) -> Result<Option<Value>, RpcError> {
    let arguments_json = serde_json::to_string(arguments)
        .map_err(|error| RpcError::new(-32603, error.to_string()))?;
    let arguments_hash = hash_token(&arguments_json);
    let now = now_rfc3339();
    service
        .db()
        .expire_mcp_approvals(&now)
        .await
        .map_err(rpc_db)?;
    if let Some(state) = params.get("requestState").and_then(Value::as_str) {
        let approval = service
            .db()
            .get_mcp_approval_by_state(state)
            .await
            .map_err(rpc_db)?
            .ok_or_else(|| RpcError::new(-32602, "unknown approval requestState"))?;
        if approval.token_id != auth.token_id
            || approval.tool_name != entry.name
            || approval.arguments_hash != arguments_hash
        {
            return Err(RpcError::new(
                -32602,
                "approval requestState does not match this tool call",
            ));
        }
        return match approval.status.as_str() {
            "pending" => Ok(Some(input_required(&approval))),
            "approved" if approval.used_at.is_none() => {
                let consumed = service
                    .db()
                    .consume_mcp_approval(&approval.id, &now)
                    .await
                    .map_err(rpc_db)?;
                if consumed {
                    Ok(None)
                } else {
                    Ok(Some(tool_error(
                        era_modern(),
                        "APPROVAL_USED",
                        "approval was already consumed",
                    )))
                }
            }
            "approved" => Ok(Some(tool_error(
                era_modern(),
                "APPROVAL_USED",
                "approval was already consumed",
            ))),
            "rejected" => Ok(Some(tool_error(
                era_modern(),
                "APPROVAL_REJECTED",
                "approval was rejected",
            ))),
            "expired" => Ok(Some(tool_error(
                era_modern(),
                "APPROVAL_EXPIRED",
                "approval expired",
            ))),
            _ => Ok(Some(tool_error(
                era_modern(),
                "APPROVAL_INVALID",
                "approval is not usable",
            ))),
        };
    }
    let state = format!("state_{}{}", Uuid::now_v7(), Uuid::now_v7());
    let expires = (OffsetDateTime::now_utc() + time::Duration::seconds(APPROVAL_TTL_SECONDS))
        .format(&Rfc3339)
        .unwrap_or_default();
    let approval = service
        .db()
        .create_mcp_approval(
            &auth.token_id,
            &entry.name,
            &arguments_json,
            &arguments_hash,
            &state,
            "high",
            &now,
            &expires,
        )
        .await
        .map_err(rpc_db)?;
    Ok(Some(input_required(&approval)))
}

fn era_modern() -> Era {
    Era::Modern
}

fn input_required(approval: &McpApprovalRow) -> Value {
    json!({
        "resultType": "input_required",
        "inputRequests": {
            "approval": {
                "method": "elicitation/create",
                "params": {
                    "message": format!("Approve high-risk tool `{}` before it runs.", approval.tool_name),
                    "requestedSchema": {
                        "type": "object",
                        "properties": {
                            "approved": {"type": "boolean", "title": "Approve"}
                        },
                        "required": ["approved"]
                    }
                }
            }
        },
        "requestState": approval.request_state,
        "_meta": {
            "io.modelcontextprotocol/serverInfo": server_info(),
            "hpa/approvalId": approval.id,
            "hpa/expiresAt": approval.expires_at,
        }
    })
}

fn tool_error(era: Era, code: &str, message: impl Into<String>) -> Value {
    tool_result(
        era,
        None,
        message.into(),
        true,
        json!({"hpa/errorCode": code}),
    )
}

fn tool_result(
    era: Era,
    structured: Option<Value>,
    text: String,
    is_error: bool,
    meta: Value,
) -> Value {
    let mut result = json!({
        "content": [{"type": "text", "text": text}],
        "isError": is_error,
    });
    if let Some(structured) = structured {
        result["structuredContent"] = structured;
    }
    let mut response_meta = json!({"io.modelcontextprotocol/serverInfo": server_info()});
    if let (Some(response_object), Some(meta_object)) =
        (response_meta.as_object_mut(), meta.as_object())
    {
        for (key, value) in meta_object {
            response_object.insert(key.clone(), value.clone());
        }
    }
    result["_meta"] = response_meta;
    if era == Era::Modern {
        result["resultType"] = json!("complete");
    }
    result
}

async fn execute_tool(
    service: &McpService,
    auth: &AuthContext,
    era: Era,
    entry: &ToolEntry,
    arguments: &Value,
) -> Result<Value, RpcError> {
    match &entry.target {
        ToolTarget::Skill {
            skill_id,
            version,
            script_id,
        } => {
            let outcome = service
                .skills
                .run(skill_id, Some(version), Some(script_id), arguments.clone())
                .await;
            match outcome {
                Ok(outcome) => {
                    let text = outcome
                        .output
                        .as_ref()
                        .map(|value| serde_json::to_string_pretty(value).unwrap_or_default())
                        .filter(|text| !text.is_empty())
                        .unwrap_or_else(|| outcome.stdout.clone());
                    let meta = json!({
                        "hpa/runId": outcome.id,
                        "hpa/sandbox": outcome.sandbox,
                        "hpa/artifacts": outcome.artifacts.iter().map(|artifact| artifact.name.clone()).collect::<Vec<_>>(),
                    });
                    Ok(tool_result(
                        era,
                        outcome.output.clone(),
                        text,
                        !outcome.succeeded(),
                        meta,
                    ))
                }
                Err(error) => Ok(tool_result(
                    era,
                    None,
                    error.to_string(),
                    true,
                    json!({"hpa/errorCode": skill_error_code(&error)}),
                )),
            }
        }
        ToolTarget::WorkflowExecute => execute_workflow(service, auth, era, arguments).await,
        ToolTarget::WorkflowList => {
            let workflows = service
                .db()
                .list_workflows()
                .await
                .map_err(rpc_db)?
                .into_iter()
                .filter(|workflow| workflow.published != 0)
                .map(|workflow| {
                    json!({
                        "workflowId": workflow.id,
                        "name": workflow.name,
                        "version": workflow.version,
                    })
                })
                .collect::<Vec<_>>();
            Ok(tool_result(
                era,
                Some(json!({"workflows": workflows})),
                serde_json::to_string_pretty(&workflows).unwrap_or_default(),
                false,
                json!({}),
            ))
        }
        ToolTarget::ExecutionGet => {
            let execution_id = arguments
                .get("executionId")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let row = service
                .db()
                .get_execution(execution_id)
                .await
                .map_err(rpc_db)?
                .ok_or_else(|| {
                    RpcError::new(-32602, format!("execution `{execution_id}` not found"))
                })?;
            let state = crate::db::parse_state(&row).map_err(rpc_db)?;
            let value = json!({
                "executionId": state.id,
                "workflowId": state.workflow_id,
                "workflowVersion": state.workflow_version,
                "status": state.status,
                "output": state.output,
                "error": state.error,
                "updatedAt": state.updated_at,
            });
            Ok(tool_result(
                era,
                Some(value.clone()),
                serde_json::to_string_pretty(&value).unwrap_or_default(),
                false,
                json!({}),
            ))
        }
    }
}

async fn execute_workflow(
    service: &McpService,
    auth: &AuthContext,
    era: Era,
    arguments: &Value,
) -> Result<Value, RpcError> {
    let workflow_id = arguments
        .get("workflowId")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::new(-32602, "workflowId is required"))?;
    let version = arguments
        .get("version")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::new(-32602, "version is required"))?;
    let Some(version_row) = service
        .db()
        .get_version(workflow_id, version)
        .await
        .map_err(rpc_db)?
    else {
        return Ok(tool_error(
            era,
            "WORKFLOW_VERSION_NOT_FOUND",
            format!("workflow `{workflow_id}@{version}` is not published"),
        ));
    };
    let workflow = serde_json::from_str::<Workflow>(&version_row.ir_json)
        .map_err(|error| RpcError::new(-32603, format!("invalid workflow snapshot: {error}")))?;
    let compilation = compile_workflow(&workflow).map_err(|issues| {
        RpcError::new(-32603, serde_json::to_string(&issues).unwrap_or_default())
    })?;
    let input = arguments.get("input").cloned().unwrap_or_else(|| json!({}));
    let execution_id = format!("exec_{}", Uuid::now_v7());
    let mut execution = new_state(
        &workflow,
        &compilation.plan,
        &execution_id,
        input,
        BTreeMap::new(),
        auth.tenant_id.clone(),
    )
    .map_err(|error| RpcError::new(-32602, error.to_string()))?;
    let event = start_event(&mut execution);
    let row = ExecutionRow {
        id: execution.id.clone(),
        workflow_id: execution.workflow_id.clone(),
        workflow_version: execution.workflow_version.clone(),
        status: "running".to_string(),
        state_json: serde_json::to_string(&execution)
            .map_err(|error| RpcError::new(-32603, error.to_string()))?,
        workflow_json: version_row.ir_json,
        output_json: None,
        error_json: None,
        started_at: execution.started_at.clone(),
        updated_at: execution.updated_at.clone(),
    };
    service
        .db()
        .insert_execution(&row, &event)
        .await
        .map_err(rpc_db)?;
    service.runtime.trigger(execution_id.clone());
    let value = json!({
        "executionId": execution_id,
        "taskId": execution_id,
        "status": "running"
    });
    Ok(tool_result(
        era,
        Some(value.clone()),
        serde_json::to_string_pretty(&value).unwrap_or_default(),
        false,
        json!({
            "io.modelcontextprotocol/task": {"taskId": execution_id}
        }),
    ))
}

async fn mcp_task(service: &McpService, task_id: &str) -> Result<Value, RpcError> {
    let row = service
        .db()
        .get_execution(task_id)
        .await
        .map_err(rpc_db)?
        .ok_or_else(|| RpcError::new(-32602, format!("task `{task_id}` not found")))?;
    let state = crate::db::parse_state(&row).map_err(rpc_db)?;
    Ok(json!({
        "taskId": task_id,
        "executionId": task_id,
        "status": crate::db::status_text(state.status),
        "output": state.output,
        "error": state.error,
        "waiting": state.waiting,
        "updatedAt": state.updated_at,
        "resultType": "complete",
    }))
}

fn skill_error_code(error: &SkillServiceError) -> String {
    match error {
        SkillServiceError::Skill(error) => error.code().to_string(),
        SkillServiceError::NotFound(_) => "SKILL_NOT_FOUND".to_string(),
        SkillServiceError::Invalid { code, .. } => code.clone(),
        SkillServiceError::Db(DbError::SkillVersionFrozen { .. }) => {
            "SKILL_VERSION_FROZEN".to_string()
        }
        _ => "SKILL_ERROR".to_string(),
    }
}

fn rpc_db(error: DbError) -> RpcError {
    RpcError::new(-32603, error.to_string())
}

async fn list_resources(service: &McpService, auth: &AuthContext) -> Result<Vec<Value>, RpcError> {
    let mut resources = Vec::new();
    for skill in service.db().list_skills().await.map_err(rpc_db)? {
        let Some(version) = service
            .db()
            .get_skill_version(&skill.id, &skill.latest_version)
            .await
            .map_err(rpc_db)?
        else {
            continue;
        };
        if version.status != "published" {
            continue;
        }
        resources.push(json!({
            "uri": format!("hpa://tenant/{}/skills/{}/versions/{}", auth.tenant_id, skill.id, version.version),
            "name": format!("{} {}", skill.name, version.version),
            "description": "Published Agent Skill snapshot.",
            "mimeType": "application/json",
        }));
    }
    for workflow in service.db().list_workflows().await.map_err(rpc_db)? {
        if workflow.published == 0 {
            continue;
        }
        if let Some(version) = service
            .db()
            .get_version(&workflow.id, &workflow.version)
            .await
            .map_err(rpc_db)?
        {
            resources.push(json!({
                "uri": format!("hpa://tenant/{}/workflows/{}/versions/{}", auth.tenant_id, workflow.id, version.version),
                "name": format!("{} {}", workflow.name, version.version),
                "description": "Published workflow snapshot.",
                "mimeType": "application/json",
            }));
        }
    }
    for execution in service.db().list_executions(10).await.map_err(rpc_db)? {
        resources.push(json!({
            "uri": format!("hpa://tenant/{}/executions/{}", auth.tenant_id, execution.id),
            "name": format!("Execution {}", execution.id),
            "description": "Execution state snapshot.",
            "mimeType": "application/json",
        }));
    }
    for artifact in service
        .db()
        .list_artifacts(None)
        .await
        .map_err(rpc_db)?
        .into_iter()
        .take(10)
    {
        resources.push(json!({
            "uri": format!("hpa://tenant/{}/artifacts/{}", auth.tenant_id, artifact.id),
            "name": artifact.name,
            "description": "Execution artifact.",
            "mimeType": artifact.mime_type,
        }));
    }
    if conformance_mode() {
        resources.push(json!({
            "uri": "test://static-text",
            "name": "Conformance static text",
            "description": "Static text resource for conformance tests.",
            "mimeType": "text/plain"
        }));
        resources.push(json!({
            "uri": "test://static-binary",
            "name": "Conformance static binary",
            "description": "Static binary resource for conformance tests.",
            "mimeType": "image/png"
        }));
    }
    Ok(resources)
}

fn resource_templates(auth: &AuthContext) -> Vec<Value> {
    let tenant = auth.tenant_id.as_str();
    let mut templates = vec![
        json!({
            "uriTemplate": format!("hpa://tenant/{tenant}/skills/{{skillId}}/versions/{{version}}"),
            "name": "Published skill snapshot",
            "mimeType": "application/json",
        }),
        json!({
            "uriTemplate": format!("hpa://tenant/{tenant}/workflows/{{workflowId}}/versions/{{version}}"),
            "name": "Published workflow snapshot",
            "mimeType": "application/json",
        }),
        json!({
            "uriTemplate": format!("hpa://tenant/{tenant}/executions/{{executionId}}"),
            "name": "Execution state",
            "mimeType": "application/json",
        }),
        json!({
            "uriTemplate": format!("hpa://tenant/{tenant}/artifacts/{{artifactId}}"),
            "name": "Execution artifact",
            "mimeType": "application/json",
        }),
    ];
    if conformance_mode() {
        templates.push(json!({
            "uriTemplate": "test://template/{id}/data",
            "name": "Conformance resource template",
            "mimeType": "application/json",
        }));
    }
    templates
}

async fn read_resource(
    service: &McpService,
    auth: &AuthContext,
    uri: &str,
) -> Result<Value, RpcError> {
    if conformance_mode() {
        if uri == "test://static-text" {
            return Ok(json!({
                "uri": uri,
                "mimeType": "text/plain",
                "text": "This is the content of the static text resource."
            }));
        }
        if uri == "test://static-binary" {
            return Ok(json!({
                "uri": uri,
                "mimeType": "image/png",
                "blob": CONFORMANCE_PNG
            }));
        }
        if let Some(rest) = uri.strip_prefix("test://template/")
            && let Some(id) = rest.strip_suffix("/data")
            && !id.is_empty()
        {
            return Ok(json!({
                "uri": uri,
                "mimeType": "application/json",
                "text": json!({
                    "id": id,
                    "templateTest": true,
                    "data": format!("Data for ID: {id}")
                })
                .to_string()
            }));
        }
    }
    let rest = uri
        .strip_prefix("hpa://tenant/")
        .ok_or_else(|| RpcError::new(-32602, "unsupported resource uri"))?;
    let segments = rest.split('/').collect::<Vec<_>>();
    if segments.len() < 3 || segments[0] != auth.tenant_id {
        return Err(RpcError::new(
            -32602,
            "resource is not available for this tenant",
        ));
    }
    let kind = segments[1];
    let id = segments[2];
    let value = match kind {
        "skills" => {
            if segments.len() != 5 || segments[3] != "versions" {
                return Err(RpcError::new(-32602, "invalid skill resource uri"));
            }
            let loaded = service
                .skills
                .load(id, Some(segments[4]))
                .await
                .map_err(|error| RpcError::new(-32602, error.to_string()))?;
            json!({
                "skillId": loaded.snapshot.id,
                "version": loaded.snapshot.version,
                "license": loaded.snapshot.license,
                "contentHash": loaded.snapshot.content_hash,
                "manifest": loaded.snapshot.manifest,
                "skillMd": loaded.snapshot.skill_md,
                "report": loaded.report,
            })
        }
        "workflows" => {
            if segments.len() != 5 || segments[3] != "versions" {
                return Err(RpcError::new(-32602, "invalid workflow resource uri"));
            }
            let row = service
                .db()
                .get_version(id, segments[4])
                .await
                .map_err(rpc_db)?
                .ok_or_else(|| RpcError::new(-32602, "workflow version not found"))?;
            json!({
                "workflowId": id,
                "version": row.version,
                "registryVersion": row.registry_version,
                "ir": serde_json::from_str::<Value>(&row.ir_json).unwrap_or(Value::Null),
            })
        }
        "executions" => {
            let row = service
                .db()
                .get_execution(id)
                .await
                .map_err(rpc_db)?
                .ok_or_else(|| RpcError::new(-32602, "execution not found"))?;
            let state = crate::db::parse_state(&row).map_err(rpc_db)?;
            serde_json::to_value(state).map_err(|error| RpcError::new(-32603, error.to_string()))?
        }
        "artifacts" => {
            let artifact = service
                .db()
                .get_artifact(id)
                .await
                .map_err(rpc_db)?
                .ok_or_else(|| RpcError::new(-32602, "artifact not found"))?;
            json!({
                "artifact": {
                    "id": artifact.id,
                    "name": artifact.name,
                    "kind": artifact.kind,
                    "mimeType": artifact.mime_type,
                    "size": artifact.size,
                    "createdAt": artifact.created_at,
                },
                "content": serde_json::from_str::<Value>(&artifact.content_json).unwrap_or(Value::Null),
            })
        }
        _ => return Err(RpcError::new(-32602, "unknown resource kind")),
    };
    Ok(json!({
        "uri": uri,
        "mimeType": "application/json",
        "text": serde_json::to_string_pretty(&value).unwrap_or_default(),
    }))
}

async fn list_prompts(service: &McpService, _auth: &AuthContext) -> Result<Vec<Value>, RpcError> {
    let mut prompts = Vec::new();
    for skill in service.db().list_skills().await.map_err(rpc_db)? {
        let Some(version) = service
            .db()
            .get_skill_version(&skill.id, &skill.latest_version)
            .await
            .map_err(rpc_db)?
        else {
            continue;
        };
        if version.status != "published" {
            continue;
        }
        prompts.push(json!({
            "name": format!("skill.{}", skill.name),
            "title": skill.name,
            "description": format!("Instructions from Agent Skill `{}`.", skill.name),
            "arguments": [],
        }));
    }
    prompts.push(json!({
        "name": "workflow.review",
        "title": "Review a workflow",
        "description": "Review a published workflow IR for risks and gaps.",
        "arguments": [{"name": "workflowId", "description": "Workflow id", "required": true}],
    }));
    if conformance_mode() {
        prompts.push(json!({
            "name": "test_simple_prompt",
            "title": "Conformance simple prompt",
            "description": "Simple prompt for conformance tests.",
            "arguments": [],
        }));
        prompts.push(json!({
            "name": "test_prompt_with_arguments",
            "title": "Conformance parameterized prompt",
            "description": "Parameterized prompt for conformance tests.",
            "arguments": [
                {"name": "arg1", "description": "First argument", "required": true},
                {"name": "arg2", "description": "Second argument", "required": true}
            ],
        }));
        prompts.push(json!({
            "name": "test_prompt_with_embedded_resource",
            "title": "Conformance embedded resource prompt",
            "description": "Prompt with embedded resource for conformance tests.",
            "arguments": [{"name": "resourceUri", "description": "Resource URI", "required": true}],
        }));
        prompts.push(json!({
            "name": "test_prompt_with_image",
            "title": "Conformance image prompt",
            "description": "Prompt with image content for conformance tests.",
            "arguments": [],
        }));
        prompts.push(json!({
            "name": "test_input_required_result_prompt",
            "title": "Conformance input-required prompt",
            "description": "Prompt that requests elicitation input via InputRequiredResult.",
            "arguments": [],
        }));
    }
    Ok(prompts)
}

async fn get_prompt(
    service: &McpService,
    _auth: &AuthContext,
    name: &str,
    arguments: &Value,
) -> Result<Value, RpcError> {
    if conformance_mode() {
        match name {
            "test_simple_prompt" => {
                return Ok(json!({
                    "messages": [{
                        "role": "user",
                        "content": {"type": "text", "text": "This is a simple prompt for testing."}
                    }]
                }));
            }
            "test_prompt_with_arguments" => {
                let arg1 = arguments
                    .get("arg1")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let arg2 = arguments
                    .get("arg2")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                return Ok(json!({
                    "messages": [{
                        "role": "user",
                        "content": {
                            "type": "text",
                            "text": format!("Prompt with arguments: arg1='{arg1}', arg2='{arg2}'")
                        }
                    }]
                }));
            }
            "test_prompt_with_embedded_resource" => {
                let resource_uri = arguments
                    .get("resourceUri")
                    .and_then(Value::as_str)
                    .unwrap_or("test://example-resource");
                return Ok(json!({
                    "messages": [
                        {
                            "role": "user",
                            "content": {
                                "type": "resource",
                                "resource": {
                                    "uri": resource_uri,
                                    "mimeType": "text/plain",
                                    "text": "Embedded resource content for testing."
                                }
                            }
                        },
                        {
                            "role": "user",
                            "content": {
                                "type": "text",
                                "text": "Please process the embedded resource above."
                            }
                        }
                    ]
                }));
            }
            "test_prompt_with_image" => {
                return Ok(json!({
                    "messages": [
                        {
                            "role": "user",
                            "content": {
                                "type": "image",
                                "data": CONFORMANCE_PNG,
                                "mimeType": "image/png"
                            }
                        },
                        {
                            "role": "user",
                            "content": {
                                "type": "text",
                                "text": "Please analyze the image above."
                            }
                        }
                    ]
                }));
            }
            _ => {}
        }
    }
    if let Some(skill_name) = name.strip_prefix("skill.") {
        let skill = service
            .db()
            .list_skills()
            .await
            .map_err(rpc_db)?
            .into_iter()
            .find(|skill| skill.name == skill_name)
            .ok_or_else(|| RpcError::new(-32602, format!("skill `{skill_name}` not found")))?;
        let version = service
            .db()
            .get_skill_version(&skill.id, &skill.latest_version)
            .await
            .map_err(rpc_db)?
            .ok_or_else(|| RpcError::new(-32602, "skill version not found"))?;
        return Ok(json!({
            "description": format!("Instructions for `{}`.", skill.name),
            "messages": [{
                "role": "user",
                "content": {"type": "text", "text": version.skill_md},
            }],
        }));
    }
    if name == "workflow.review" {
        let workflow_id = arguments
            .get("workflowId")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::new(-32602, "workflowId argument is required"))?;
        let workflow = service
            .db()
            .get_workflow(workflow_id)
            .await
            .map_err(rpc_db)?
            .ok_or_else(|| RpcError::new(-32602, "workflow not found"))?;
        let text = format!(
            "Review workflow `{}` ({}). Identify permission, reliability and data-boundary risks. Workflow IR:\n{}",
            workflow.name, workflow.id, workflow.ir_json
        );
        return Ok(json!({
            "description": "Workflow review prompt.",
            "messages": [{"role": "user", "content": {"type": "text", "text": text}}],
        }));
    }
    Err(RpcError::new(-32602, format!("prompt `{name}` not found")))
}

pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn scopes_of(row: &McpTokenRow) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(&row.scopes_json).unwrap_or_default()
}

pub async fn create_pat(
    db: &Db,
    name: &str,
    scopes: &[String],
    tenant_id: &str,
    now: &str,
) -> Result<(String, McpTokenRow), DbError> {
    let token = format!("hpa_pat_{}{}", Uuid::now_v7(), Uuid::now_v7());
    let hash = hash_token(&token);
    let scopes_json = serde_json::to_string(scopes)?;
    let row = db
        .create_mcp_token(name, &hash, &scopes_json, tenant_id, now)
        .await?;
    Ok((token, row))
}

pub async fn authenticate_token(
    db: &Db,
    token: &str,
    now: &str,
) -> Result<Option<AuthContext>, DbError> {
    let hash = hash_token(token);
    let Some(row) = db.find_mcp_token(&hash, now).await? else {
        return Ok(None);
    };
    if row.revoked_at.is_some() {
        return Ok(None);
    }
    let scopes = scopes_of(&row);
    Ok(Some(AuthContext {
        token_id: row.id,
        tenant_id: row.tenant_id,
        scopes,
        label: row.name,
    }))
}

pub async fn stdio_auth(
    db: &Db,
    data_dir: &Path,
) -> Result<AuthContext, Box<dyn std::error::Error>> {
    let now = now_rfc3339();
    if let Ok(token) = std::env::var("HPA_MCP_TOKEN") {
        let Some(auth) = authenticate_token(db, &token, &now).await? else {
            return Err("HPA_MCP_TOKEN is not a valid MCP token".into());
        };
        return Ok(auth);
    }
    let directory = data_dir.join("mcp");
    std::fs::create_dir_all(&directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700));
    }
    let path = directory.join("stdio.capability");
    let token = if path.is_file() {
        std::fs::read_to_string(&path)?.trim().to_string()
    } else {
        let scopes = vec![SCOPE_READ.to_string(), SCOPE_EXECUTE.to_string()];
        let (token, _) = create_pat(db, "stdio capability", &scopes, "local", &now).await?;
        #[cfg(unix)]
        {
            use std::io::Write as _;
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .mode(0o600)
                .open(&path)?;
            file.write_all(token.as_bytes())?;
            file.write_all(b"\n")?;
        }
        #[cfg(not(unix))]
        {
            std::fs::write(&path, format!("{token}\n"))?;
        }
        token
    };
    let Some(auth) = authenticate_token(db, &token, &now).await? else {
        return Err(format!(
            "capability token in {} is unavailable; remove the file to regenerate",
            path.display()
        )
        .into());
    };
    Ok(auth)
}

pub fn token_scopes(row: &McpTokenRow) -> Vec<String> {
    scopes_of(row)
}

pub fn local_admin() -> AuthContext {
    AuthContext {
        token_id: "mcptok_local_ui".to_string(),
        tenant_id: "local".to_string(),
        scopes: vec![SCOPE_READ.to_string(), SCOPE_EXECUTE.to_string()],
        label: "local UI".to_string(),
    }
}

pub async fn admin_request(service: &McpService, method: &str, mut params: Value) -> Value {
    admin_request_with(service, method, &mut params, true).await
}

pub async fn admin_request_quiet(service: &McpService, method: &str, mut params: Value) -> Value {
    admin_request_with(service, method, &mut params, false).await
}

async fn admin_request_with(
    service: &McpService,
    method: &str,
    params: &mut Value,
    write_audit: bool,
) -> Value {
    if !params.is_object() {
        *params = json!({});
    }
    if method != "initialize" && params.get("_meta").is_none() {
        params["_meta"] = json!({
            "io.modelcontextprotocol/protocolVersion": MODERN_VERSION,
            "io.modelcontextprotocol/clientCapabilities": {
                "extensions": { (TASKS_EXTENSION): {} }
            },
            "io.modelcontextprotocol/clientInfo": {"name": "hpa-local-ui", "version": env!("CARGO_PKG_VERSION")},
        });
    }
    let request = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    let outcome = handle_request_inner(
        service,
        &local_admin(),
        Era::Modern,
        &request,
        write_audit,
        ToolNameStyle::Canonical,
    )
    .await;
    outcome
        .response
        .unwrap_or_else(|| json!({"jsonrpc": "2.0", "id": 1, "result": {}}))
}

pub fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default()
}

pub struct McpClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl McpClient {
    pub fn spawn(command: &[String]) -> Result<Self, String> {
        Self::spawn_with_env(command, &BTreeMap::new())
    }

    pub fn spawn_with_env(
        command: &[String],
        env: &BTreeMap<String, String>,
    ) -> Result<Self, String> {
        if command.is_empty() {
            return Err("missing MCP server command".to_string());
        }
        let mut child = Command::new(&command[0])
            .args(&command[1..])
            .envs(env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| format!("cannot start MCP server: {error}"))?;
        let stdin = child.stdin.take().ok_or("MCP server has no stdin")?;
        let stdout = child.stdout.take().ok_or("MCP server has no stdout")?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
        })
    }

    pub fn initialize(&mut self) -> Result<Value, String> {
        let result = self.request(
            "initialize",
            json!({
                "protocolVersion": LEGACY_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "hpa-cli", "version": env!("CARGO_PKG_VERSION")},
            }),
        )?;
        self.notify("notifications/initialized", json!({}))?;
        Ok(result)
    }

    pub fn list_tools(&mut self) -> Result<Value, String> {
        self.request("tools/list", json!({}))
    }

    pub fn list_resources(&mut self) -> Result<Value, String> {
        self.request("resources/list", json!({}))
    }

    pub fn list_prompts(&mut self) -> Result<Value, String> {
        self.request("prompts/list", json!({}))
    }

    pub fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value, String> {
        self.request("tools/call", json!({"name": name, "arguments": arguments}))
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let mut line = serde_json::to_string(&request).map_err(|error| error.to_string())?;
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .map_err(|error| format!("cannot write MCP request: {error}"))?;
        self.stdin.flush().map_err(|error| error.to_string())?;
        loop {
            let mut response = String::new();
            let read = self
                .stdout
                .read_line(&mut response)
                .map_err(|error| format!("cannot read MCP response: {error}"))?;
            if read == 0 {
                return Err("MCP server closed the connection".to_string());
            }
            let value: Value = serde_json::from_str(response.trim())
                .map_err(|error| format!("invalid MCP response: {error}"))?;
            if value.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = value.get("error") {
                return Err(format!("MCP error: {error}"));
            }
            return Ok(value.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        let notification = json!({"jsonrpc": "2.0", "method": method, "params": params});
        let mut line = serde_json::to_string(&notification).map_err(|error| error.to_string())?;
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .map_err(|error| format!("cannot write MCP notification: {error}"))?;
        self.stdin.flush().map_err(|error| error.to_string())
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    async fn fixture_service(label: &str) -> McpService {
        let root = std::env::temp_dir().join(format!("hpa-mcp-{label}-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&root).expect("temp root");
        let db = Db::open(&root.join("hpa.db")).await.expect("db");
        let service = SkillService::new(db.clone(), root);
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/skills/invoice-audit");
        service.import(&path).await.expect("import");
        service
            .publish("skill_invoice_audit", Some("1.0.0"))
            .await
            .expect("publish");
        let runtime = RuntimeManager::new(db);
        McpService::new(service, runtime)
    }

    #[test]
    fn slug_tool_names_are_vscode_compatible() {
        let mut used = HashSet::new();
        assert_eq!(wire_tool_name("workflow.list", &mut used), "workflow_list");
        assert_eq!(
            wire_tool_name("skill.invoice-audit.audit", &mut used),
            "skill_invoice-audit_audit"
        );
        assert_eq!(
            wire_tool_name("workflow_list", &mut used),
            "workflow_list_2"
        );
        assert_eq!(
            wire_tool_name("workflow_execute", &mut used),
            "workflow_execute"
        );
        for name in used {
            assert!(name.chars().all(|character| character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || character == '_'
                || character == '-'));
        }
    }

    fn auth() -> AuthContext {
        AuthContext {
            token_id: "mcptok_test".to_string(),
            tenant_id: "local".to_string(),
            scopes: vec![SCOPE_READ.to_string(), SCOPE_EXECUTE.to_string()],
            label: "test".to_string(),
        }
    }

    fn modern_params(method: &str, params: Value) -> Value {
        let mut params = params;
        params["_meta"] = json!({
            "io.modelcontextprotocol/protocolVersion": MODERN_VERSION,
            "io.modelcontextprotocol/clientCapabilities": {},
            "io.modelcontextprotocol/clientInfo": {"name": "test", "version": "1"},
        });
        let _ = method;
        params
    }

    fn request(id: u64, method: &str, params: Value) -> Value {
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
    }

    #[tokio::test]
    async fn modern_discovery_and_tool_call() {
        let service = fixture_service("modern").await;
        let auth = auth();
        let discover = handle_request(
            &service,
            &auth,
            Era::Modern,
            &request(
                1,
                "server/discover",
                modern_params("server/discover", json!({})),
            ),
        )
        .await
        .response
        .expect("discover response");
        assert_eq!(discover["result"]["resultType"], "complete");
        assert!(
            discover["result"]["capabilities"]["extensions"]
                .get(TASKS_EXTENSION)
                .is_some_and(Value::is_object)
        );
        assert!(discover["result"]["capabilities"].get("tasks").is_none());

        let list = handle_request(
            &service,
            &auth,
            Era::Modern,
            &request(2, "tools/list", modern_params("tools/list", json!({}))),
        )
        .await
        .response
        .expect("tools");
        assert_eq!(list["result"]["resultType"], "complete");
        assert_eq!(list["result"]["cacheScope"], "private");
        let tools = list["result"]["tools"].as_array().expect("tools");
        assert!(
            tools
                .iter()
                .any(|tool| tool["name"] == "skill.invoice-audit.audit")
        );

        let call = handle_request(
            &service,
            &auth,
            Era::Modern,
            &request(
                3,
                "tools/call",
                modern_params(
                    "tools/call",
                    json!({
                        "name": "skill.invoice-audit.audit",
                        "arguments": {"invoiceId": "inv_mcp", "items": [{"name": "a", "amount": 9.5}]}
                    }),
                ),
            ),
        )
        .await
        .response
        .expect("call");
        assert_eq!(call["result"]["resultType"], "complete");
        assert_eq!(call["result"]["structuredContent"]["total"], json!(9.5));
    }

    #[tokio::test]
    async fn modern_requests_require_meta_and_scope() {
        let service = fixture_service("validation").await;
        let auth = auth();
        let missing = handle_request(
            &service,
            &auth,
            Era::Modern,
            &request(1, "tools/list", json!({})),
        )
        .await
        .response
        .expect("response");
        assert!(missing.get("error").is_some());

        let read_only = AuthContext {
            scopes: vec![SCOPE_READ.to_string()],
            ..auth
        };
        let call = handle_request(
            &service,
            &read_only,
            Era::Modern,
            &request(
                2,
                "tools/call",
                modern_params(
                    "tools/call",
                    json!({"name": "skill.invoice-audit.audit", "arguments": {}}),
                ),
            ),
        )
        .await
        .response
        .expect("response");
        assert_eq!(call["error"]["code"], json!(-32003));
    }

    #[tokio::test]
    async fn legacy_initialize_and_call_are_compatible() {
        let service = fixture_service("legacy").await;
        let auth = auth();
        let initialize = handle_request(
            &service,
            &auth,
            Era::Legacy,
            &request(
                1,
                "initialize",
                json!({"protocolVersion": LEGACY_VERSION, "capabilities": {}, "clientInfo": {"name": "legacy", "version": "1"}}),
            ),
        )
        .await;
        assert!(initialize.new_session.is_some());
        let response = initialize.response.expect("initialize");
        assert_eq!(response["result"]["protocolVersion"], json!(LEGACY_VERSION));
        assert!(response["result"].get("resultType").is_none());

        let call = handle_request(
            &service,
            &auth,
            Era::Legacy,
            &request(
                2,
                "tools/call",
                json!({"name": "skill.invoice-audit.audit", "arguments": {"invoiceId": "inv_legacy", "items": []}}),
            ),
        )
        .await
        .response
        .expect("call");
        assert!(call["result"].get("resultType").is_none());
        assert_eq!(call["result"]["structuredContent"]["total"], json!(0));
    }

    #[tokio::test]
    async fn high_risk_tool_requires_server_side_approval() {
        let service = fixture_service("approval").await;
        let workflow_text = include_str!("../../../fixtures/01_sequence.json");
        let workflow = Workflow::from_json_str(workflow_text).expect("workflow");
        let compilation = compile_workflow(&workflow).expect("compile");
        let mut execution = new_state(
            &workflow,
            &compilation.plan,
            "exec_warmup",
            json!({"value": 1}),
            BTreeMap::new(),
            "local",
        )
        .expect("state");
        let _ = start_event(&mut execution);
        let now = now_rfc3339();
        let row = crate::db::WorkflowRow {
            id: workflow.id.clone(),
            name: workflow.name.clone(),
            version: workflow.workflow_version.clone(),
            description: None,
            published: 0,
            ir_json: serde_json::to_string(&workflow).expect("ir"),
            yaml: String::new(),
            created_at: now.clone(),
            updated_at: now.clone(),
        };
        service.skills.db.upsert_workflow(&row).await.expect("save");
        service
            .skills
            .db
            .publish_workflow(&row, "test")
            .await
            .expect("publish");

        let auth = auth();
        let params = modern_params(
            "tools/call",
            json!({
                "name": "workflow.execute",
                "arguments": {"workflowId": workflow.id, "version": workflow.workflow_version, "input": {"value": 2}}
            }),
        );
        let first = handle_request(
            &service,
            &auth,
            Era::Modern,
            &request(1, "tools/call", params.clone()),
        )
        .await
        .response
        .expect("first");
        assert_eq!(first["result"]["resultType"], json!("input_required"));
        let approval_id = first["result"]["_meta"]["hpa/approvalId"]
            .as_str()
            .expect("approval id")
            .to_string();
        let state = first["result"]["requestState"]
            .as_str()
            .expect("state")
            .to_string();

        service
            .skills
            .db
            .decide_mcp_approval(&approval_id, "approved", &now_rfc3339())
            .await
            .expect("approve");
        let mut retry_params = params;
        retry_params["requestState"] = json!(state);
        let second = handle_request(
            &service,
            &auth,
            Era::Modern,
            &request(2, "tools/call", retry_params),
        )
        .await
        .response
        .expect("second");
        assert_eq!(second["result"]["isError"], false, "{second}");
        assert_eq!(second["result"]["resultType"], "complete");
    }

    fn mrtr_params(extra: Value) -> Value {
        let mut params = extra;
        if params.get("_meta").is_none() {
            params["_meta"] = json!({
                "io.modelcontextprotocol/protocolVersion": MODERN_VERSION,
                "io.modelcontextprotocol/clientCapabilities": {
                    "sampling": {},
                    "elicitation": {},
                    "roots": {"listChanged": true}
                },
            });
        }
        params
    }

    #[test]
    fn mrtr_elicitation_retries_and_ignores_extra_keys() {
        let first = mrtr_tool_call(
            "test_input_required_result_elicitation",
            &mrtr_params(json!({})),
            Era::Modern,
        )
        .expect("first");
        assert_eq!(first["resultType"], "input_required");
        assert_eq!(
            first["inputRequests"]["user_name"]["method"],
            "elicitation/create"
        );
        assert!(first.get("requestState").is_none());

        let missing = mrtr_tool_call(
            "test_input_required_result_elicitation",
            &mrtr_params(json!({"inputResponses": {"wrong_key": {"action": "accept"}}})),
            Era::Modern,
        )
        .expect("missing");
        assert_eq!(missing["resultType"], "input_required");

        let complete = mrtr_tool_call(
            "test_input_required_result_elicitation",
            &mrtr_params(json!({
                "inputResponses": {
                    "user_name": {"action": "accept", "content": {"name": "Alice"}},
                    "unexpected": {"action": "accept", "content": {"foo": "bar"}}
                }
            })),
            Era::Modern,
        )
        .expect("complete");
        assert_eq!(complete["resultType"], "complete");
        assert_eq!(complete["content"][0]["text"], "Hello, Alice!");

        let invalid = mrtr_tool_call(
            "test_input_required_result_elicitation",
            &mrtr_params(json!({"inputResponses": {"user_name": 42}})),
            Era::Modern,
        )
        .expect("invalid shape re-requests");
        assert_eq!(invalid["resultType"], "input_required");

        let null = mrtr_tool_call(
            "test_input_required_result_elicitation",
            &mrtr_params(json!({"inputResponses": null})),
            Era::Modern,
        )
        .expect_err("null inputResponses is a protocol error");
        assert_eq!(null.code, -32602);
    }

    #[test]
    fn mrtr_request_state_is_signed_and_multi_round_evolves() {
        let first = mrtr_tool_call(
            "test_input_required_result_request_state",
            &mrtr_params(json!({})),
            Era::Modern,
        )
        .expect("first");
        let state = first["requestState"].as_str().expect("state").to_string();
        let complete = mrtr_tool_call(
            "test_input_required_result_request_state",
            &mrtr_params(json!({
                "inputResponses": {"confirm": {"action": "accept", "content": {"ok": true}}},
                "requestState": state
            })),
            Era::Modern,
        )
        .expect("complete");
        assert_eq!(complete["resultType"], "complete");
        assert!(
            complete["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("state-ok")
        );

        let tampered_first = mrtr_tool_call(
            "test_input_required_result_tampered_state",
            &mrtr_params(json!({})),
            Era::Modern,
        )
        .expect("tampered first");
        let tampered_state = tampered_first["requestState"]
            .as_str()
            .expect("tampered state")
            .to_string();
        let rejected = mrtr_tool_call(
            "test_input_required_result_tampered_state",
            &mrtr_params(json!({
                "inputResponses": {"confirm": {"action": "accept", "content": {"ok": true}}},
                "requestState": format!("{tampered_state}-TAMPERED")
            })),
            Era::Modern,
        )
        .expect_err("tampered state must be rejected");
        assert_eq!(rejected.code, -32602);

        let round1 = mrtr_tool_call(
            "test_input_required_result_multi_round",
            &mrtr_params(json!({})),
            Era::Modern,
        )
        .expect("round 1");
        assert_eq!(round1["resultType"], "input_required");
        let state1 = round1["requestState"]
            .as_str()
            .expect("state 1")
            .to_string();
        let round2 = mrtr_tool_call(
            "test_input_required_result_multi_round",
            &mrtr_params(json!({
                "inputResponses": {"step1": {"action": "accept", "content": {"name": "Alice"}}},
                "requestState": state1
            })),
            Era::Modern,
        )
        .expect("round 2");
        assert_eq!(round2["resultType"], "input_required");
        assert!(round2["inputRequests"]["step2"].is_object());
        let state2 = round2["requestState"].as_str().expect("state 2");
        assert_ne!(state1, state2);
        let round3 = mrtr_tool_call(
            "test_input_required_result_multi_round",
            &mrtr_params(json!({
                "inputResponses": {"step2": {"action": "accept", "content": {"color": "blue"}}},
                "requestState": state2
            })),
            Era::Modern,
        )
        .expect("round 3");
        assert_eq!(round3["resultType"], "complete");
    }

    #[test]
    fn mrtr_multiple_inputs_capabilities_and_prompt() {
        let multiple = mrtr_tool_call(
            "test_input_required_result_multiple_inputs",
            &mrtr_params(json!({})),
            Era::Modern,
        )
        .expect("multiple");
        let methods = ["user_name", "greeting", "client_roots"]
            .iter()
            .map(|key| multiple["inputRequests"][key]["method"].as_str().unwrap())
            .collect::<HashSet<_>>();
        assert_eq!(methods.len(), 3);
        let state = multiple["requestState"].as_str().expect("state");
        let complete = mrtr_tool_call(
            "test_input_required_result_multiple_inputs",
            &mrtr_params(json!({
                "inputResponses": {
                    "user_name": {"action": "accept", "content": {"name": "Alice"}},
                    "greeting": {"role": "assistant", "content": {"type": "text", "text": "hi"}},
                    "client_roots": {"roots": [{"uri": "file:///test/root", "name": "Test Root"}]}
                },
                "requestState": state
            })),
            Era::Modern,
        )
        .expect("multiple complete");
        assert_eq!(complete["resultType"], "complete");

        let capabilities = mrtr_tool_call(
            "test_input_required_result_capabilities",
            &mrtr_params(json!({
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": MODERN_VERSION,
                    "io.modelcontextprotocol/clientCapabilities": {"sampling": {}}
                }
            })),
            Era::Modern,
        )
        .expect("capabilities");
        assert_eq!(capabilities["resultType"], "input_required");
        let requests = capabilities["inputRequests"]
            .as_object()
            .expect("input requests");
        assert!(!requests.is_empty());
        assert!(
            requests
                .values()
                .all(|request| { request["method"] != "elicitation/create" })
        );

        let prompt = mrtr_prompt(&mrtr_params(json!({})), Era::Modern).expect("prompt first");
        assert_eq!(prompt["resultType"], "input_required");
        let prompt_complete = mrtr_prompt(
            &mrtr_params(json!({
                "inputResponses": {"user_context": {"action": "accept", "content": {"context": "test context"}}}
            })),
            Era::Modern,
        )
        .expect("prompt complete");
        assert_eq!(prompt_complete["resultType"], "complete");
        assert!(prompt_complete["messages"].is_array());
    }

    fn tasks_params(declared: bool) -> Value {
        let capabilities = if declared {
            json!({"extensions": {(TASKS_EXTENSION): {}}})
        } else {
            json!({})
        };
        json!({
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": MODERN_VERSION,
                "io.modelcontextprotocol/clientCapabilities": capabilities,
            }
        })
    }

    #[tokio::test]
    async fn tasks_extension_gates_and_tracks_fixture_state() {
        let missing =
            task_fixture_tool_call("failing_job", &tasks_params(false), Era::Modern, "local")
                .expect_err("failing_job is task-supporting and requires the extension");
        assert_eq!(missing.code, -32021);
        assert_eq!(
            missing.data.expect("required capability data")["requiredCapabilities"]["extensions"]
                [TASKS_EXTENSION],
            json!({})
        );

        let sync =
            task_fixture_tool_call("slow_compute", &tasks_params(false), Era::Modern, "local")
                .expect("sync fallback");
        assert_eq!(sync["resultType"], "complete");
        assert!(sync.get("taskId").is_none());
        assert_eq!(sync["content"][0]["text"], "Computed compute");

        let greet = task_fixture_tool_call(
            "greet",
            &json!({"arguments": {"name": "World"}}),
            Era::Modern,
            "local",
        )
        .expect("greet");
        assert_eq!(greet["content"][0]["text"], "Hello, World!");

        let create = task_fixture_tool_call(
            "slow_compute",
            &json!({
                "_meta": {
                    "io.modelcontextprotocol/clientCapabilities": {
                        "extensions": {(TASKS_EXTENSION): {}}
                    }
                },
                "arguments": {"seconds": 2, "label": "unit"},
            }),
            Era::Modern,
            "local",
        )
        .expect("create task");
        assert_eq!(create["resultType"], "task");
        assert!(create.get("result").is_none());
        assert!(create.get("error").is_none());
        assert!(create.get("inputRequests").is_none());
        assert!(create.get("requestState").is_none());
        assert_eq!(create["ttlMs"], json!(60_000));
        assert_eq!(create["pollIntervalMs"], json!(100));
        let task_id = create["taskId"].as_str().expect("task id").to_string();
        let detail = conformance_task_detail(&conformance_task_by_id(&task_id).expect("stored"));
        assert_eq!(detail["status"], "working");
        assert!(cancel_conformance_task(&task_id));
        let cancelled = conformance_task_detail(&conformance_task_by_id(&task_id).expect("stored"));
        assert_eq!(cancelled["status"], "cancelled");
        assert!(cancelled.get("requestState").is_none());

        let multi = task_fixture_tool_call(
            "multi_input",
            &json!({
                "_meta": {
                    "io.modelcontextprotocol/clientCapabilities": {
                        "extensions": {(TASKS_EXTENSION): {}}
                    }
                },
                "arguments": {},
            }),
            Era::Modern,
            "local",
        )
        .expect("multi input");
        let multi_id = multi["taskId"].as_str().expect("multi id").to_string();
        let pending =
            conformance_task_detail(&conformance_task_by_id(&multi_id).expect("multi stored"));
        assert_eq!(pending["status"], "input_required");
        assert_eq!(
            pending["inputRequests"]
                .as_object()
                .expect("requests")
                .len(),
            2
        );
        assert!(
            update_conformance_task(
                &multi_id,
                &json!({"inputResponses": {"first": {"action": "accept"}}}),
                Era::Modern,
            )
            .expect("partial update")
        );
        let partial =
            conformance_task_detail(&conformance_task_by_id(&multi_id).expect("multi stored"));
        assert_eq!(partial["status"], "input_required");
        let remaining = partial["inputRequests"]
            .as_object()
            .expect("remaining requests");
        assert_eq!(remaining.len(), 1);
        assert!(remaining.contains_key("second"));
        assert!(
            update_conformance_task(
                &multi_id,
                &json!({"inputResponses": {"second": {"action": "accept"}}}),
                Era::Modern,
            )
            .expect("final update")
        );
        let completed =
            conformance_task_detail(&conformance_task_by_id(&multi_id).expect("multi stored"));
        assert_eq!(completed["status"], "completed");
        assert_eq!(
            completed["result"]["content"][0]["text"],
            "Both inputs received"
        );
    }

    #[tokio::test]
    async fn tasks_mrtr_composition_escalates_to_task() {
        let params = json!({
            "_meta": {
                "io.modelcontextprotocol/clientCapabilities": {
                    "elicitation": {},
                    "extensions": {(TASKS_EXTENSION): {}}
                }
            },
            "arguments": {},
        });
        let first = task_fixture_tool_call("test_tool_with_task", &params, Era::Modern, "local")
            .expect("round 1");
        assert_eq!(first["resultType"], "input_required");
        assert!(first.get("taskId").is_none());
        assert!(first["inputRequests"].get("user_name").is_some());

        let second = task_fixture_tool_call(
            "test_tool_with_task",
            &json!({
                "_meta": {
                    "io.modelcontextprotocol/clientCapabilities": {
                        "elicitation": {},
                        "extensions": {(TASKS_EXTENSION): {}}
                    }
                },
                "arguments": {},
                "inputResponses": {
                    "user_name": {"action": "accept", "content": {"name": "Alice"}}
                }
            }),
            Era::Modern,
            "local",
        )
        .expect("round 2");
        assert_eq!(second["resultType"], "task");
        assert!(second.get("requestState").is_none());
        assert!(second.get("inputRequests").is_none());
        let task_id = second["taskId"].as_str().expect("task id").to_string();
        for _ in 0..50 {
            let detail =
                conformance_task_detail(&conformance_task_by_id(&task_id).expect("stored"));
            if detail["status"] == "completed" {
                assert_eq!(detail["result"]["content"][0]["text"], "Hello, Alice!");
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("task did not complete");
    }
}
