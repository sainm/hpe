use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::db::{Db, McpAuditRow};
use crate::mcp::Era;
use crate::mcp::{
    MODERN_VERSION, McpService, SCOPE_EXECUTE, SCOPE_READ, SUPPORTED_VERSIONS, ToolNameStyle,
    authenticate_token, detect_era, handle_request_styled, now_rfc3339,
};
use crate::oauth::OauthService;

const SESSION_TTL: Duration = Duration::from_secs(3600);
const DEFAULT_LIMIT_PER_MINUTE: usize = 120;

struct RateWindow {
    started: Instant,
    count: usize,
}

#[derive(Clone)]
pub struct McpGatewayState {
    pub service: McpService,
    pub base_url: String,
    oauth: Option<OauthService>,
    sessions: Arc<Mutex<HashMap<String, Instant>>>,
    rate: Arc<Mutex<HashMap<String, RateWindow>>>,
    limit_per_minute: usize,
}

impl McpGatewayState {
    pub fn new(service: McpService, base_url: String) -> Self {
        Self {
            service,
            base_url,
            oauth: None,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            rate: Arc::new(Mutex::new(HashMap::new())),
            limit_per_minute: DEFAULT_LIMIT_PER_MINUTE,
        }
    }

    pub fn with_oauth(mut self, oauth: OauthService) -> Self {
        self.oauth = Some(oauth);
        self
    }

    pub fn with_rate_limit(mut self, limit_per_minute: usize) -> Self {
        self.limit_per_minute = limit_per_minute.max(1);
        self
    }

    fn check_rate(&self, token_id: &str) -> Result<(), u64> {
        let mut windows = self.rate.lock().expect("rate limiter");
        let window = windows
            .entry(token_id.to_string())
            .or_insert_with(|| RateWindow {
                started: Instant::now(),
                count: 0,
            });
        if window.started.elapsed() >= Duration::from_secs(60) {
            window.started = Instant::now();
            window.count = 0;
        }
        if window.count >= self.limit_per_minute {
            let retry = 60u64.saturating_sub(window.started.elapsed().as_secs());
            return Err(retry.max(1));
        }
        window.count += 1;
        Ok(())
    }

    fn open_session(&self) -> String {
        let id = format!("mcpsess_{}", Uuid::now_v7());
        let mut sessions = self.sessions.lock().expect("sessions");
        sessions.retain(|_, created| created.elapsed() < SESSION_TTL);
        sessions.insert(id.clone(), Instant::now());
        id
    }

    fn has_session(&self, id: &str) -> bool {
        let mut sessions = self.sessions.lock().expect("sessions");
        sessions.retain(|_, created| created.elapsed() < SESSION_TTL);
        sessions.contains_key(id)
    }

    fn close_session(&self, id: &str) {
        self.sessions.lock().expect("sessions").remove(id);
    }
}

pub fn router(state: McpGatewayState) -> Router {
    Router::new()
        .route("/mcp", post(handle_mcp).get(method_not_allowed))
        .route("/mcp", delete(handle_delete))
        .route(
            "/.well-known/oauth-protected-resource",
            get(protected_resource_metadata),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(protected_resource_metadata),
        )
        .route(
            "/mcp/.well-known/oauth-protected-resource",
            get(protected_resource_metadata),
        )
        .route_layer(axum::middleware::from_fn(crate::metrics::track_http))
        .with_state(state)
}

async fn method_not_allowed() -> Response {
    let mut response = StatusCode::METHOD_NOT_ALLOWED.into_response();
    response
        .headers_mut()
        .insert(header::ALLOW, HeaderValue::from_static("POST, DELETE"));
    response
}

async fn handle_delete(State(state): State<McpGatewayState>, headers: HeaderMap) -> Response {
    let Some(session) = header_text(&headers, "mcp-session-id") else {
        return plain_error(StatusCode::BAD_REQUEST, "missing MCP-Session-Id");
    };
    state.close_session(&session);
    StatusCode::NO_CONTENT.into_response()
}

async fn protected_resource_metadata(State(state): State<McpGatewayState>) -> Json<Value> {
    Json(json!({
        "resource": format!("{}/mcp", state.base_url),
        "resource_name": "hpa local MCP",
        "authorization_servers": [state.base_url],
        "bearer_methods_supported": ["header"],
        "scopes_supported": [SCOPE_READ, SCOPE_EXECUTE],
        "resource_documentation": format!("{}/settings", state.base_url),
    }))
}

async fn oauth_auth(
    state: &McpGatewayState,
    token: &str,
) -> Result<Option<crate::mcp::AuthContext>, String> {
    let Some(oauth) = state.oauth.as_ref() else {
        return Ok(None);
    };
    if token.split('.').count() != 3 {
        return Ok(None);
    }
    if oauth.audience() != format!("{}/mcp", state.base_url) {
        return Ok(None);
    }
    let access = oauth
        .verify_access_token(token)
        .await
        .map_err(|error| error.description())?;
    if access.tenant_id != "local" {
        return Err("token was issued for another tenant".to_string());
    }
    Ok(Some(crate::mcp::AuthContext {
        token_id: format!("oauth:{}:{}", access.client_id, access.jti),
        tenant_id: access.tenant_id,
        scopes: access.scopes,
        label: format!("oauth:{}", access.client_id),
    }))
}

async fn handle_mcp(
    State(state): State<McpGatewayState>,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if method != Method::POST {
        return method_not_allowed().await;
    }
    if let Some(origin) = header_text(&headers, "origin")
        && !valid_origin(&origin)
    {
        return plain_error(StatusCode::FORBIDDEN, "origin is not allowed");
    }
    let request: Value = match serde_json::from_slice(&body) {
        Ok(request @ Value::Object(_)) => request,
        Ok(_) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                None,
                -32600,
                "batch or non-object JSON-RPC payloads are not supported",
            );
        }
        Err(error) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                None,
                -32700,
                format!("parse error: {error}"),
            );
        }
    };
    let request_id = request.get("id").cloned().unwrap_or(Value::Null);
    let rpc_method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if rpc_method.is_empty() {
        return json_error(
            StatusCode::BAD_REQUEST,
            Some(request_id),
            -32600,
            "missing JSON-RPC method",
        );
    }
    let modern_header = header_text(&headers, "mcp-protocol-version");
    if let Some(version) = &modern_header
        && !SUPPORTED_VERSIONS.contains(&version.as_str())
    {
        audit(
            &state.service.skills.db,
            None,
            &rpc_method,
            "unsupported_version",
            json!({"version": version}),
        )
        .await;
        return json_error(
            StatusCode::BAD_REQUEST,
            Some(request_id),
            -32001,
            format!("unsupported protocol version `{version}`"),
        );
    }
    let era = if rpc_method == "initialize" {
        if modern_header.as_deref() == Some(MODERN_VERSION) {
            return json_error(
                StatusCode::BAD_REQUEST,
                Some(request_id),
                -32001,
                "initialize is a legacy method; modern clients use server/discover",
            );
        }
        Era::Legacy
    } else {
        detect_era(&request, None)
    };
    if era == Era::Modern {
        if modern_header.as_deref() != Some(MODERN_VERSION) {
            return json_error(
                StatusCode::BAD_REQUEST,
                Some(request_id),
                -32000,
                "MCP-Protocol-Version header is required",
            );
        }
        if let Some(error) = validate_modern_headers(&headers, &request, &rpc_method) {
            return error;
        }
    } else if modern_header.as_deref() == Some(MODERN_VERSION) {
        return json_error(
            StatusCode::BAD_REQUEST,
            Some(request_id),
            -32000,
            "modern protocol header requires modern request metadata",
        );
    }
    if era == Era::Legacy && rpc_method != "initialize" {
        let Some(session) = header_text(&headers, "mcp-session-id") else {
            return json_error(
                StatusCode::BAD_REQUEST,
                Some(request_id),
                -32000,
                "MCP-Session-Id header is required for legacy requests",
            );
        };
        if !state.has_session(&session) {
            return json_error(
                StatusCode::NOT_FOUND,
                Some(request_id),
                -32000,
                "unknown or expired MCP session",
            );
        }
    }
    let auth_header = header_text(&headers, "authorization");
    let Some(token) = auth_header
        .as_deref()
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        audit(
            &state.service.skills.db,
            None,
            &rpc_method,
            "unauthorized",
            json!({"reason": "missing bearer token"}),
        )
        .await;
        return unauthorized(&state, "missing bearer token", false);
    };
    let auth = match authenticate_token(&state.service.skills.db, token, &now_rfc3339()).await {
        Ok(Some(auth)) => auth,
        Ok(None) => match oauth_auth(&state, token).await {
            Ok(Some(auth)) => auth,
            Ok(None) => {
                audit(
                    &state.service.skills.db,
                    None,
                    &rpc_method,
                    "unauthorized",
                    json!({"reason": "invalid token"}),
                )
                .await;
                return unauthorized(&state, "invalid token", true);
            }
            Err(error) => {
                audit(
                    &state.service.skills.db,
                    None,
                    &rpc_method,
                    "unauthorized",
                    json!({"reason": "invalid oauth token", "detail": error}),
                )
                .await;
                return unauthorized(&state, "invalid token", true);
            }
        },
        Err(error) => {
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                Some(request_id),
                -32603,
                error.to_string(),
            );
        }
    };
    if let Err(retry_after) = state.check_rate(&auth.token_id) {
        audit(
            &state.service.skills.db,
            Some(&auth.token_id),
            &rpc_method,
            "rate_limited",
            json!({}),
        )
        .await;
        let mut response = json_error(
            StatusCode::TOO_MANY_REQUESTS,
            Some(request_id),
            -32000,
            "MCP rate limit exceeded",
        );
        response.headers_mut().insert(
            header::RETRY_AFTER,
            HeaderValue::from_str(&retry_after.to_string())
                .unwrap_or_else(|_| HeaderValue::from_static("60")),
        );
        return response;
    }
    let is_notification = request.get("id").is_none();
    let tool_name_style = match header_text(&headers, "x-hpa-tool-name-style").as_deref() {
        Some("slug") => ToolNameStyle::Slug,
        _ => ToolNameStyle::Canonical,
    };
    let outcome =
        handle_request_styled(&state.service, &auth, era, &request, tool_name_style).await;
    let mut response_headers = HeaderMap::new();
    if era == Era::Modern {
        response_headers.insert(
            "mcp-protocol-version",
            HeaderValue::from_static(MODERN_VERSION),
        );
    }
    let session = if let Some(session) = outcome.new_session {
        let id = state.open_session();
        let _ = session;
        response_headers.insert(
            "mcp-session-id",
            HeaderValue::from_str(&id).unwrap_or_else(|_| HeaderValue::from_static("session")),
        );
        Some(id)
    } else {
        header_text(&headers, "mcp-session-id")
    };
    if let Some(session) = session
        && era == Era::Legacy
    {
        response_headers.insert(
            "mcp-session-id",
            HeaderValue::from_str(&session).unwrap_or_else(|_| HeaderValue::from_static("session")),
        );
    }
    if is_notification {
        let mut response = StatusCode::ACCEPTED.into_response();
        response.headers_mut().extend(response_headers);
        return response;
    }
    let response = outcome
        .response
        .unwrap_or_else(|| json!({"jsonrpc": "2.0", "id": request_id, "result": {}}));
    let mut response = (StatusCode::OK, Json(response)).into_response();
    response.headers_mut().extend(response_headers);
    response
}

fn validate_modern_headers(headers: &HeaderMap, request: &Value, method: &str) -> Option<Response> {
    let request_id = request.get("id").cloned().unwrap_or(Value::Null);
    let header_method = header_text(headers, "mcp-method");
    if header_method.as_deref() != Some(method) {
        return Some(json_error(
            StatusCode::BAD_REQUEST,
            Some(request_id),
            -32000,
            "Mcp-Method header does not match the JSON-RPC method",
        ));
    }
    let name_source = match method {
        "tools/call" | "prompts/get" => request
            .get("params")
            .and_then(|params| params.get("name"))
            .and_then(Value::as_str),
        "resources/read" => request
            .get("params")
            .and_then(|params| params.get("uri"))
            .and_then(Value::as_str),
        _ => None,
    };
    if let Some(expected) = name_source {
        let header_name = header_text(headers, "mcp-name");
        if header_name.as_deref() != Some(expected) {
            return Some(json_error(
                StatusCode::BAD_REQUEST,
                Some(request_id),
                -32000,
                "Mcp-Name header does not match the request name",
            ));
        }
    }
    None
}

fn valid_origin(origin: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(origin) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    matches!(
        url.host_str(),
        Some("127.0.0.1") | Some("localhost") | Some("::1")
    )
}

fn header_text(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

fn unauthorized(state: &McpGatewayState, message: &str, invalid_token: bool) -> Response {
    let metadata = format!("{}/.well-known/oauth-protected-resource", state.base_url);
    let challenge = if invalid_token {
        format!(
            "Bearer error=\"invalid_token\", resource_metadata=\"{metadata}\", scope=\"mcp:read mcp:execute\""
        )
    } else {
        format!("Bearer resource_metadata=\"{metadata}\", scope=\"mcp:read mcp:execute\"")
    };
    let mut response = json_error(StatusCode::UNAUTHORIZED, None, -32001, message.to_string());
    if let Ok(value) = HeaderValue::from_str(&challenge) {
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, value);
    }
    response
}

fn plain_error(status: StatusCode, message: &str) -> Response {
    (status, message.to_string()).into_response()
}

fn json_error(
    status: StatusCode,
    id: Option<Value>,
    code: i64,
    message: impl Into<String>,
) -> Response {
    let body = json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "error": {"code": code, "message": message.into()},
    });
    (status, Json(body)).into_response()
}

async fn audit(db: &Db, token_id: Option<&str>, method: &str, status: &str, detail: Value) {
    let row = McpAuditRow {
        id: format!("mcpaudit_{}", Uuid::now_v7()),
        at: now_rfc3339(),
        token_id: token_id.map(str::to_string),
        method: method.to_string(),
        tool_name: None,
        status: status.to_string(),
        latency_ms: 0,
        detail_json: Some(detail.to_string()),
    };
    if let Err(error) = db.insert_mcp_audit(&row).await {
        tracing::warn!(error = %error, "cannot persist MCP gateway audit");
    }
}
