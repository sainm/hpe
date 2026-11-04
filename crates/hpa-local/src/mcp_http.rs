use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures_util::StreamExt;
use futures_util::stream;
use hpa_local::persistent_mcp::{
    McpNotification, McpStoreError, PersistentTask, PostgresMcpStore, PostgresMcpTaskNode,
    TaskUpdateCommand, TaskUpsert,
};
use serde_json::{Value, json};
use sqlx::postgres::PgPool;
use tokio::sync::broadcast;
use tracing::Instrument as _;
use uuid::Uuid;

use crate::db::{Db, McpAuditRow};
use crate::enterprise::EnterpriseState;
use crate::mcp::Era;
use crate::mcp::{
    MODERN_VERSION, McpService, RpcError, SCOPE_EXECUTE, SCOPE_READ, SUPPORTED_VERSIONS,
    ToolNameStyle, authenticate_token, detect_era, finish, handle_request_styled, now_rfc3339,
};
use crate::oauth::OauthService;

const SESSION_TTL: Duration = Duration::from_secs(3600);
const DEFAULT_LIMIT_PER_MINUTE: usize = 120;
const DEFAULT_NOTIFICATION_MAX_LAG: i64 = 10_000;
const MODERN_REMOVED_METHODS: &[&str] = &[
    "initialize",
    "ping",
    "logging/setLevel",
    "resources/subscribe",
    "resources/unsubscribe",
];

#[derive(Debug, Clone)]
struct CatalogChange {
    tenant_id: String,
    kind: String,
}

fn catalog_changes() -> &'static broadcast::Sender<CatalogChange> {
    static CHANGES: OnceLock<broadcast::Sender<CatalogChange>> = OnceLock::new();
    CHANGES.get_or_init(|| broadcast::channel(128).0)
}

fn persistent_pool() -> &'static OnceLock<PgPool> {
    static POOL: OnceLock<PgPool> = OnceLock::new();
    &POOL
}

pub fn configure_persistent_mcp(pool: PgPool) {
    if persistent_pool().set(pool).is_err() {
        tracing::warn!("persistent MCP pool was already configured");
    }
}

pub fn spawn_task_update_monitor(
    state: McpGatewayState,
    enterprise: EnterpriseState,
) -> Option<tokio::task::JoinHandle<()>> {
    let pool = state.postgres.as_ref()?.clone();
    let node_id = state.node_id.clone();
    Some(tokio::spawn(async move {
        let tick = std::env::var("HPA_MCP_TASK_TICK_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(250);
        loop {
            match enterprise.active_tenants().await {
                Ok(tenants) => {
                    for tenant_id in tenants {
                        let node_store =
                            PostgresMcpTaskNode::new(pool.clone(), &tenant_id, &node_id);
                        if let Err(error) = sync_owned_tasks(&enterprise, &pool, &node_store).await
                        {
                            tracing::warn!(
                                tenant_id,
                                %error,
                                "failed to synchronize persistent MCP task projections"
                            );
                        }
                        let commands = match node_store.claim(20).await {
                            Ok(commands) => commands,
                            Err(error) => {
                                tracing::error!(
                                    tenant_id,
                                    %error,
                                    "failed to claim persistent MCP task updates"
                                );
                                continue;
                            }
                        };
                        for command in commands {
                            if let Err(error) =
                                apply_task_update(&enterprise, &pool, &node_store, &command).await
                            {
                                tracing::warn!(
                                    tenant_id = command.tenant_id,
                                    task_id = command.task_id,
                                    update_id = command.id,
                                    %error,
                                    "persistent MCP task update was not applied"
                                );
                                let terminal = matches!(
                                    command.task_status.as_str(),
                                    "completed" | "failed" | "expired"
                                );
                                let result = if terminal {
                                    node_store.fail(&command.id, &error).await
                                } else {
                                    node_store.retry(&command.id, &error, 2).await
                                };
                                if let Err(store_error) = result {
                                    tracing::error!(
                                        update_id = command.id,
                                        %store_error,
                                        "failed to release persistent MCP task update"
                                    );
                                }
                            }
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(error = %error.message, "MCP task update tenant scan failed");
                }
            }
            tokio::time::sleep(Duration::from_millis(tick)).await;
        }
    }))
}

async fn sync_owned_tasks(
    enterprise: &EnterpriseState,
    pool: &PgPool,
    node_store: &PostgresMcpTaskNode,
) -> Result<(), String> {
    let tasks = node_store
        .owned_tasks(100)
        .await
        .map_err(|error| error.to_string())?;
    for task in tasks {
        let Some(execution_id) = task.execution_id.as_deref() else {
            continue;
        };
        let tenant = enterprise
            .tenant_api_state(&task.tenant_id)
            .await
            .map_err(|error| error.message)?;
        let Some(row) = tenant
            .db
            .get_execution(execution_id)
            .await
            .map_err(|error| error.to_string())?
        else {
            continue;
        };
        let execution = crate::db::parse_state(&row).map_err(|error| error.to_string())?;
        let status = match execution.status {
            hpa_core::runtime::ExecutionStatus::Pending
            | hpa_core::runtime::ExecutionStatus::Running => "working",
            hpa_core::runtime::ExecutionStatus::WaitingApproval => "input_required",
            hpa_core::runtime::ExecutionStatus::Succeeded => "completed",
            hpa_core::runtime::ExecutionStatus::Failed => "failed",
            hpa_core::runtime::ExecutionStatus::Cancelled => "cancelled",
        };
        let error = execution
            .error
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|error| error.to_string())?;
        let approval = execution
            .waiting
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|error| error.to_string())?;
        let store = PostgresMcpStore::new(pool.clone(), &task.tenant_id, &task.token_id);
        let (updated, status_changed) = store
            .update_task_projection(
                &task.id,
                status,
                execution.output.as_ref(),
                error.as_ref(),
                approval.as_ref(),
            )
            .await
            .map_err(|error| error.to_string())?;
        if status_changed {
            store
                .publish_notification(
                    "tasks",
                    &json!({
                        "taskId": updated.id,
                        "status": updated.status,
                        "updatedAt": updated.updated_at.to_string()
                    }),
                )
                .await
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

async fn apply_task_update(
    enterprise: &EnterpriseState,
    pool: &PgPool,
    node_store: &PostgresMcpTaskNode,
    command: &TaskUpdateCommand,
) -> Result<(), String> {
    if command.action != "cancel" {
        return Err(format!("unsupported MCP task action `{}`", command.action));
    }
    if command.task_status == "cancelled" {
        let store = PostgresMcpStore::new(pool.clone(), &command.tenant_id, &command.token_id);
        let task = store
            .get_task(&command.task_id)
            .await
            .map_err(|error| error.to_string())?;
        store
            .publish_notification(
                "tasks",
                &json!({
                    "taskId": task.id,
                    "status": task.status,
                    "updatedAt": task.updated_at.to_string()
                }),
            )
            .await
            .map_err(|error| error.to_string())?;
        node_store
            .complete(&command.id)
            .await
            .map_err(|error| error.to_string())?;
        return Ok(());
    }
    if matches!(
        command.task_status.as_str(),
        "completed" | "failed" | "expired"
    ) {
        return Err(format!(
            "task `{}` is already terminal with status `{}`",
            command.task_id, command.task_status
        ));
    }
    let execution_id = command
        .execution_id
        .as_deref()
        .ok_or_else(|| format!("task `{}` has no execution reference", command.task_id))?;
    let tenant = enterprise
        .tenant_api_state(&command.tenant_id)
        .await
        .map_err(|error| error.message)?;
    let row = tenant
        .db
        .get_execution(execution_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| {
            format!(
                "execution `{execution_id}` is not present on owner node `{}`",
                command.owner_node_id
            )
        })?;
    let mut execution = crate::db::parse_state(&row).map_err(|error| error.to_string())?;
    if execution.status == hpa_core::runtime::ExecutionStatus::WaitingApproval {
        let events = hpa_core::runtime::cancel_execution(&mut execution)
            .map_err(|error| error.to_string())?;
        tenant
            .db
            .save_execution(&execution, &events)
            .await
            .map_err(|error| error.to_string())?;
    } else if execution.status != hpa_core::runtime::ExecutionStatus::Cancelled {
        return Err(format!(
            "execution `{execution_id}` cannot be cancelled from status `{}`",
            crate::db::status_text(execution.status)
        ));
    }
    let store = PostgresMcpStore::new(pool.clone(), &command.tenant_id, &command.token_id);
    let task = match store.cancel_task(&command.task_id).await {
        Ok(task) => task,
        Err(McpStoreError::InvalidTaskTransition { current, .. }) if current == "cancelled" => {
            store
                .get_task(&command.task_id)
                .await
                .map_err(|error| error.to_string())?
        }
        Err(error) => return Err(error.to_string()),
    };
    store
        .publish_notification(
            "tasks",
            &json!({
                "taskId": task.id,
                "status": task.status,
                "updatedAt": task.updated_at.to_string()
            }),
        )
        .await
        .map_err(|error| error.to_string())?;
    node_store
        .complete(&command.id)
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub(crate) fn notify_catalog_changed(tenant_id: &str, kind: &str) {
    let _ = catalog_changes().send(CatalogChange {
        tenant_id: tenant_id.to_string(),
        kind: kind.to_string(),
    });
    if let Some(pool) = persistent_pool().get().cloned() {
        let tenant_id = tenant_id.to_string();
        let kind = kind.to_string();
        tokio::spawn(async move {
            let store = PostgresMcpStore::new(pool, tenant_id.clone(), "catalog-notifier");
            if let Err(error) = store
                .publish_notification(&kind, &json!({"kind": kind}))
                .await
            {
                tracing::error!(
                    tenant_id,
                    kind,
                    %error,
                    "failed to persist MCP catalog notification"
                );
            }
        });
    }
}

struct RateWindow {
    started: Instant,
    count: usize,
}

#[derive(Clone)]
struct SessionRecord {
    created: Instant,
    tenant_id: String,
    token_id: String,
}

#[derive(Clone)]
pub struct McpGatewayState {
    pub service: McpService,
    pub base_url: String,
    node_id: String,
    oauth: Option<OauthService>,
    enterprise: Option<EnterpriseState>,
    postgres: Option<PgPool>,
    sessions: Arc<Mutex<HashMap<String, SessionRecord>>>,
    rate: Arc<Mutex<HashMap<String, RateWindow>>>,
    limit_per_minute: usize,
}

impl McpGatewayState {
    pub fn new(service: McpService, base_url: String) -> Self {
        let node_id = std::env::var("HPA_NODE_ID")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| format!("node:{base_url}"));
        Self {
            service,
            base_url,
            node_id,
            oauth: None,
            enterprise: None,
            postgres: None,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            rate: Arc::new(Mutex::new(HashMap::new())),
            limit_per_minute: DEFAULT_LIMIT_PER_MINUTE,
        }
    }

    pub fn with_oauth(mut self, oauth: OauthService) -> Self {
        self.oauth = Some(oauth);
        self
    }

    pub fn with_enterprise(mut self, enterprise: EnterpriseState) -> Self {
        self.enterprise = Some(enterprise);
        self
    }

    pub fn with_postgres(mut self, pool: PgPool) -> Self {
        configure_persistent_mcp(pool.clone());
        self.postgres = Some(pool);
        self
    }

    #[allow(dead_code)]
    pub fn with_node_id(mut self, node_id: impl Into<String>) -> Self {
        self.node_id = node_id.into();
        self
    }

    pub fn node_id(&self) -> &str {
        &self.node_id
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

    async fn open_session(&self, auth: &crate::mcp::AuthContext) -> Result<String, String> {
        if let Some(store) = self.persistent_store(auth) {
            return store
                .create_session(Some(&auth.label), SESSION_TTL.as_secs() as i32)
                .await
                .map(|session| session.id)
                .map_err(|error| error.to_string());
        }
        let id = format!("mcpsess_{}", Uuid::now_v7());
        let mut sessions = self.sessions.lock().expect("sessions");
        sessions.retain(|_, session| session.created.elapsed() < SESSION_TTL);
        sessions.insert(
            id.clone(),
            SessionRecord {
                created: Instant::now(),
                tenant_id: auth.tenant_id.clone(),
                token_id: auth.token_id.clone(),
            },
        );
        Ok(id)
    }

    async fn has_session(&self, id: &str, auth: &crate::mcp::AuthContext) -> Result<bool, String> {
        if let Some(store) = self.persistent_store(auth) {
            return store
                .validate_session(id)
                .await
                .map_err(|error| error.to_string());
        }
        let mut sessions = self.sessions.lock().expect("sessions");
        sessions.retain(|_, session| session.created.elapsed() < SESSION_TTL);
        Ok(sessions.get(id).is_some_and(|session| {
            session.tenant_id == auth.tenant_id && session.token_id == auth.token_id
        }))
    }

    async fn close_session(
        &self,
        id: &str,
        auth: &crate::mcp::AuthContext,
    ) -> Result<bool, String> {
        if let Some(store) = self.persistent_store(auth) {
            return store
                .close_session(id)
                .await
                .map_err(|error| error.to_string());
        }
        let mut sessions = self.sessions.lock().expect("sessions");
        sessions.retain(|_, session| session.created.elapsed() < SESSION_TTL);
        Ok(sessions.get(id).is_some_and(|session| {
            session.tenant_id == auth.tenant_id && session.token_id == auth.token_id
        }) && sessions.remove(id).is_some())
    }

    fn persistent_store(&self, auth: &crate::mcp::AuthContext) -> Option<PostgresMcpStore> {
        self.postgres.as_ref().map(|pool| {
            PostgresMcpStore::new(pool.clone(), auth.tenant_id.clone(), auth.token_id.clone())
        })
    }
}

pub fn router(state: McpGatewayState) -> Router {
    Router::new()
        .route("/mcp", get(handle_mcp_get).post(handle_mcp))
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
        .insert(header::ALLOW, HeaderValue::from_static("GET, POST, DELETE"));
    response
}

async fn handle_mcp_get(State(state): State<McpGatewayState>, headers: HeaderMap) -> Response {
    if header_text(&headers, "mcp-protocol-version").as_deref() == Some(MODERN_VERSION) {
        return method_not_allowed().await;
    }
    if let Some(origin) = header_text(&headers, "origin")
        && !valid_origin(&origin)
    {
        return plain_error(StatusCode::FORBIDDEN, "origin is not allowed");
    }
    let Some(session) = header_text(&headers, "mcp-session-id") else {
        return plain_error(StatusCode::BAD_REQUEST, "missing MCP-Session-Id");
    };
    let Some(token) = bearer_token(&headers) else {
        return unauthorized(&state, "missing bearer token", false);
    };
    let auth = match resolve_mcp_auth(&state, token).await {
        Ok(Some((_, auth))) => auth,
        Ok(None) | Err(McpAuthError::Invalid(_)) => {
            return unauthorized(&state, "invalid token", true);
        }
        Err(McpAuthError::Internal(error)) => {
            return json_error(StatusCode::INTERNAL_SERVER_ERROR, None, -32603, error);
        }
    };
    match state.has_session(&session, &auth).await {
        Ok(true) => {}
        Ok(false) => {
            return plain_error(StatusCode::NOT_FOUND, "unknown or expired MCP session");
        }
        Err(error) => {
            return json_error(StatusCode::INTERNAL_SERVER_ERROR, None, -32603, error);
        }
    }
    let requested = json!({
        "toolsListChanged": true,
        "resourcesListChanged": true,
        "promptsListChanged": true
    });
    let after = last_event_sequence(&headers);
    let events = match state.persistent_store(&auth) {
        Some(store) => {
            let subscription = match store
                .create_subscription(&session, &requested, SESSION_TTL.as_secs() as i32)
                .await
            {
                Ok(subscription) => subscription,
                Err(error) => {
                    return json_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        None,
                        -32603,
                        error.to_string(),
                    );
                }
            };
            let after = if after > 0 {
                after
            } else {
                match store.latest_notification_sequence().await {
                    Ok(sequence) => sequence,
                    Err(error) => {
                        return json_error(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            None,
                            -32603,
                            error.to_string(),
                        );
                    }
                }
            };
            persistent_catalog_stream(store, subscription, after, notification_max_lag(), None)
                .boxed()
        }
        None => catalog_stream(auth.tenant_id.clone(), requested, None).boxed(),
    };
    sse_stream(
        stream::once(async {
            Ok::<Event, Infallible>(Event::default().comment("hpa MCP legacy stream connected"))
        })
        .chain(events),
        HeaderMap::from_iter([(
            HeaderName::from_static("mcp-session-id"),
            HeaderValue::from_str(&session).unwrap_or_else(|_| HeaderValue::from_static("session")),
        )]),
    )
}

fn sse_stream<S>(events: S, mut headers: HeaderMap) -> Response
where
    S: futures_core::Stream<Item = Result<Event, Infallible>> + Send + 'static,
{
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    let events = events.map(|event| event.map(|event| event.retry(Duration::from_secs(2))));
    let mut response = Sse::new(events)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        )
        .into_response();
    response.headers_mut().extend(headers);
    response
}

fn catalog_stream(
    tenant_id: String,
    requested: Value,
    subscription_ref: Option<Value>,
) -> impl futures_core::Stream<Item = Result<Event, Infallible>> + Send + 'static {
    let receiver = catalog_changes().subscribe();
    stream::unfold(
        (receiver, tenant_id, requested, subscription_ref),
        |(mut receiver, tenant_id, requested, subscription_ref)| async move {
            loop {
                match receiver.recv().await {
                    Ok(change) if change.tenant_id == tenant_id => {
                        let enabled = match change.kind.as_str() {
                            "tools" => requested
                                .get("toolsListChanged")
                                .and_then(Value::as_bool)
                                .unwrap_or(false),
                            "resources" => requested
                                .get("resourcesListChanged")
                                .and_then(Value::as_bool)
                                .unwrap_or(false),
                            "prompts" => requested
                                .get("promptsListChanged")
                                .and_then(Value::as_bool)
                                .unwrap_or(false),
                            _ => false,
                        };
                        if !enabled {
                            continue;
                        }
                        let method = match change.kind.as_str() {
                            "tools" => "notifications/tools/list_changed",
                            "resources" => "notifications/resources/list_changed",
                            "prompts" => "notifications/prompts/list_changed",
                            _ => continue,
                        };
                        let mut meta = json!({
                            "io.modelcontextprotocol/protocolVersion": MODERN_VERSION
                        });
                        if let Some(subscription_ref) = subscription_ref.as_ref() {
                            meta["io.modelcontextprotocol/subscriptionId"] =
                                subscription_ref.clone();
                        }
                        let event = Event::default().event("message").data(
                            json!({
                                "jsonrpc": "2.0",
                                "method": method,
                                "params": {
                                    "_meta": meta
                                }
                            })
                            .to_string(),
                        );
                        return Some((
                            Ok(event),
                            (receiver, tenant_id, requested, subscription_ref),
                        ));
                    }
                    Ok(_) => continue,
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(
                            tenant = %tenant_id,
                            skipped,
                            "MCP in-memory notification consumer lagged; closing stream for reconnect"
                        );
                        return None;
                    }
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            }
        },
    )
}

fn last_event_sequence(headers: &HeaderMap) -> i64 {
    header_text(headers, "last-event-id")
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value >= 0)
        .unwrap_or(0)
}

fn notification_max_lag() -> i64 {
    std::env::var("HPA_MCP_NOTIFICATION_MAX_LAG")
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| (1..=100_000).contains(value))
        .unwrap_or(DEFAULT_NOTIFICATION_MAX_LAG)
}

fn persistent_catalog_stream(
    store: PostgresMcpStore,
    subscription_id: String,
    after_sequence: i64,
    max_lag: i64,
    subscription_ref: Option<Value>,
) -> impl futures_core::Stream<Item = Result<Event, Infallible>> + Send + 'static {
    stream::unfold(
        (
            store,
            subscription_id,
            after_sequence,
            max_lag,
            subscription_ref,
            VecDeque::<McpNotification>::new(),
        ),
        |(store, subscription_id, after_sequence, max_lag, subscription_ref, mut buffered)| async move {
            loop {
                if let Some(notification) = buffered.pop_front() {
                    let next_sequence = notification.sequence;
                    let event =
                        persistent_notification_event(&notification, subscription_ref.as_ref());
                    return Some((
                        Ok(event),
                        (
                            store,
                            subscription_id,
                            next_sequence,
                            max_lag,
                            subscription_ref,
                            buffered,
                        ),
                    ));
                }
                let cap = max_lag.saturating_add(1).min(10_000) as i32;
                match store
                    .pending_notification_count(&subscription_id, after_sequence, cap)
                    .await
                {
                    Ok(pending) if pending > max_lag => {
                        tracing::warn!(
                            %subscription_id,
                            pending,
                            max_lag,
                            "MCP notification consumer exceeded backlog limit; closing stream for replay"
                        );
                        return None;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        tracing::error!(
                            %error,
                            %subscription_id,
                            "persistent MCP notification backlog check failed"
                        );
                        return None;
                    }
                }
                match store
                    .poll_subscription(&subscription_id, after_sequence, 20)
                    .await
                {
                    Ok(notifications) if notifications.is_empty() => {
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                    Ok(notifications) => {
                        buffered.extend(notifications);
                    }
                    Err(error) => {
                        tracing::error!(
                            %error,
                            %subscription_id,
                            "persistent MCP notification stream closed"
                        );
                        return None;
                    }
                }
            }
        },
    )
}

fn persistent_notification_event(
    notification: &McpNotification,
    subscription_ref: Option<&Value>,
) -> Event {
    let method = match notification.kind.as_str() {
        "tools" => "notifications/tools/list_changed",
        "resources" => "notifications/resources/list_changed",
        "prompts" => "notifications/prompts/list_changed",
        "tasks" => "notifications/tasks/status_changed",
        other => {
            tracing::warn!(kind = other, "unknown MCP notification kind");
            "notifications/unknown"
        }
    };
    let mut params = notification.payload_json.clone();
    let mut meta = json!({"io.modelcontextprotocol/protocolVersion": MODERN_VERSION});
    if let Some(subscription_ref) = subscription_ref {
        meta["io.modelcontextprotocol/subscriptionId"] = subscription_ref.clone();
    }
    if let Some(object) = params.as_object_mut() {
        object.insert("_meta".to_string(), meta);
    } else {
        params = json!({
            "value": notification.payload_json,
            "_meta": meta
        });
    }
    Event::default()
        .id(notification.sequence.to_string())
        .event("message")
        .data(
            json!({
                "jsonrpc": "2.0",
                "method": method,
                "params": params
            })
            .to_string(),
        )
}

async fn handle_delete(State(state): State<McpGatewayState>, headers: HeaderMap) -> Response {
    if let Some(origin) = header_text(&headers, "origin")
        && !valid_origin(&origin)
    {
        return plain_error(StatusCode::FORBIDDEN, "origin is not allowed");
    }
    let Some(session) = header_text(&headers, "mcp-session-id") else {
        return plain_error(StatusCode::BAD_REQUEST, "missing MCP-Session-Id");
    };
    let Some(token) = bearer_token(&headers) else {
        return unauthorized(&state, "missing bearer token", false);
    };
    match resolve_mcp_auth(&state, token).await {
        Ok(Some((_, auth))) => match state.close_session(&session, &auth).await {
            Ok(true) => StatusCode::NO_CONTENT.into_response(),
            Ok(false) => plain_error(StatusCode::NOT_FOUND, "unknown or expired MCP session"),
            Err(error) => json_error(StatusCode::INTERNAL_SERVER_ERROR, None, -32603, error),
        },
        Ok(None) | Err(McpAuthError::Invalid(_)) => unauthorized(&state, "invalid token", true),
        Err(McpAuthError::Internal(error)) => {
            json_error(StatusCode::INTERNAL_SERVER_ERROR, None, -32603, error)
        }
    }
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
    if access.tenant_id != "local" && state.enterprise.is_none() {
        return Err("token was issued for another tenant".to_string());
    }
    Ok(Some(crate::mcp::AuthContext {
        token_id: format!("oauth:{}:{}", access.client_id, access.jti),
        tenant_id: access.tenant_id,
        scopes: access.scopes,
        label: format!("oauth:{}", access.client_id),
    }))
}

enum McpAuthError {
    Invalid(String),
    Internal(String),
}

async fn resolve_mcp_auth(
    state: &McpGatewayState,
    token: &str,
) -> Result<Option<(McpService, crate::mcp::AuthContext)>, McpAuthError> {
    match authenticate_token(&state.service.skills.db, token, &now_rfc3339()).await {
        Ok(Some(auth)) => return Ok(Some((state.service.clone(), auth))),
        Ok(None) => {}
        Err(error) => return Err(McpAuthError::Internal(error.to_string())),
    }
    if let Some(enterprise) = state.enterprise.as_ref() {
        let tenants = enterprise
            .active_tenants()
            .await
            .map_err(|error| McpAuthError::Internal(error.message))?;
        for tenant_id in tenants {
            let api_state = enterprise
                .tenant_api_state(&tenant_id)
                .await
                .map_err(|error| McpAuthError::Internal(error.message))?;
            let service =
                McpService::new(api_state.agents.skills.clone(), api_state.runtime.clone());
            match authenticate_token(&service.skills.db, token, &now_rfc3339()).await {
                Ok(Some(auth)) => return Ok(Some((service, auth))),
                Ok(None) => {}
                Err(error) => return Err(McpAuthError::Internal(error.to_string())),
            }
        }
    }
    let auth = match oauth_auth(state, token).await {
        Ok(Some(auth)) => auth,
        Ok(None) => return Ok(None),
        Err(error) => return Err(McpAuthError::Invalid(error)),
    };
    if let Some(enterprise) = state.enterprise.as_ref() {
        let active = enterprise
            .tenant_active(&auth.tenant_id)
            .await
            .map_err(|error| McpAuthError::Internal(error.message))?;
        if !active {
            return Err(McpAuthError::Invalid(
                "token tenant is not active".to_string(),
            ));
        }
        let api_state = enterprise
            .tenant_api_state(&auth.tenant_id)
            .await
            .map_err(|error| McpAuthError::Internal(error.message))?;
        let service = McpService::new(api_state.agents.skills.clone(), api_state.runtime.clone());
        return Ok(Some((service, auth)));
    }
    if auth.tenant_id == "local" {
        Ok(Some((state.service.clone(), auth)))
    } else {
        Err(McpAuthError::Invalid(
            "token was issued for another tenant".to_string(),
        ))
    }
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
        return json_error_data(
            StatusCode::BAD_REQUEST,
            Some(request_id),
            -32022,
            format!("unsupported protocol version `{version}`"),
            Some(json!({
                "requested": version,
                "supported": SUPPORTED_VERSIONS
            })),
        );
    }
    let modern_requested = modern_header.as_deref() == Some(MODERN_VERSION);
    if modern_requested {
        let Some(meta) = request
            .get("params")
            .and_then(|params| params.get("_meta"))
            .and_then(Value::as_object)
        else {
            return json_error(
                StatusCode::BAD_REQUEST,
                Some(request_id),
                -32602,
                "request `_meta` is required",
            );
        };
        let Some(meta_version) = meta
            .get("io.modelcontextprotocol/protocolVersion")
            .and_then(Value::as_str)
        else {
            return json_error(
                StatusCode::BAD_REQUEST,
                Some(request_id),
                -32602,
                "protocolVersion is required in `_meta`",
            );
        };
        if meta_version != MODERN_VERSION {
            return json_error_data(
                StatusCode::BAD_REQUEST,
                Some(request_id),
                -32020,
                "MCP-Protocol-Version header does not match _meta.protocolVersion",
                Some(json!({
                    "header": MODERN_VERSION,
                    "meta": meta_version
                })),
            );
        }
        if !meta
            .get("io.modelcontextprotocol/clientCapabilities")
            .is_some_and(Value::is_object)
        {
            return json_error(
                StatusCode::BAD_REQUEST,
                Some(request_id),
                -32602,
                "clientCapabilities is required in `_meta`",
            );
        }
        if MODERN_REMOVED_METHODS.contains(&rpc_method.as_str()) {
            return json_error(
                StatusCode::NOT_FOUND,
                Some(request_id),
                -32601,
                format!("method `{rpc_method}` is not available in the modern protocol"),
            );
        }
    }
    let era = if rpc_method == "initialize" {
        if modern_header.as_deref() == Some(MODERN_VERSION) {
            return json_error(
                StatusCode::NOT_FOUND,
                Some(request_id),
                -32601,
                "initialize is a legacy method; modern clients use server/discover",
            );
        }
        Era::Legacy
    } else {
        detect_era(&request, None)
    };
    if era == Era::Modern {
        if !modern_requested {
            return json_error(
                StatusCode::BAD_REQUEST,
                Some(request_id),
                -32020,
                "MCP-Protocol-Version header does not match _meta.protocolVersion",
            );
        }
        if let Some(error) = validate_modern_headers(&headers, &request, &rpc_method) {
            return error;
        }
    } else if modern_header.as_deref() == Some(MODERN_VERSION) {
        return json_error(
            StatusCode::BAD_REQUEST,
            Some(request_id),
            -32602,
            "modern protocol header requires modern request metadata",
        );
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
    let (service, auth) = match resolve_mcp_auth(&state, token).await {
        Ok(Some(resolved)) => resolved,
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
        Err(McpAuthError::Invalid(error)) => {
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
        Err(McpAuthError::Internal(error)) => {
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                Some(request_id),
                -32603,
                error,
            );
        }
    };
    if era == Era::Legacy && rpc_method != "initialize" {
        let Some(session) = header_text(&headers, "mcp-session-id") else {
            return json_error(
                StatusCode::BAD_REQUEST,
                Some(request_id),
                -32000,
                "MCP-Session-Id header is required for legacy requests",
            );
        };
        match state.has_session(&session, &auth).await {
            Ok(true) => {}
            Ok(false) => {
                return json_error(
                    StatusCode::NOT_FOUND,
                    Some(request_id),
                    -32000,
                    "unknown, expired, or cross-token MCP session",
                );
            }
            Err(error) => {
                return json_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Some(request_id),
                    -32603,
                    error,
                );
            }
        }
    }
    if let Err(retry_after) = state.check_rate(&auth.token_id) {
        audit(
            &service.skills.db,
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
    if matches!(rpc_method.as_str(), "tasks/get" | "tasks/update") {
        match persistent_task_result(&state, &auth, era, &rpc_method, &request).await {
            Ok(Some(result)) => {
                let mut response = (
                    StatusCode::OK,
                    Json(json!({
                        "jsonrpc": "2.0",
                        "id": request_id,
                        "result": result
                    })),
                )
                    .into_response();
                if era == Era::Modern {
                    response.headers_mut().insert(
                        "mcp-protocol-version",
                        HeaderValue::from_static(MODERN_VERSION),
                    );
                }
                return response;
            }
            Ok(None) => {}
            Err(error) => {
                return json_error(
                    StatusCode::BAD_REQUEST,
                    Some(request_id),
                    error.code,
                    error.message,
                );
            }
        }
    }
    if rpc_method == "subscriptions/listen" {
        if era != Era::Modern {
            return json_error(
                StatusCode::BAD_REQUEST,
                Some(request_id),
                -32000,
                "subscriptions/listen requires 2026-07-28",
            );
        }
        if !auth.has_scope(SCOPE_READ) {
            return json_error(
                StatusCode::FORBIDDEN,
                Some(request_id),
                -32003,
                "token is missing scope `mcp:read`",
            );
        }
        let requested = request
            .get("params")
            .and_then(|params| params.get("subscriptions"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        if requested.get("resourceSubscriptions").is_some_and(|value| {
            value.as_array().is_some_and(|items| !items.is_empty())
                || value.as_object().is_some_and(|items| !items.is_empty())
        }) {
            return json_error(
                StatusCode::BAD_REQUEST,
                Some(request_id),
                -32602,
                "resourceSubscriptions are not supported; use resourcesListChanged",
            );
        }
        let subscription_ref = request_id.clone();
        let (_subscription_id, events) = match state.persistent_store(&auth) {
            Some(store) => {
                let session = match store
                    .create_session(Some(&auth.label), SESSION_TTL.as_secs() as i32)
                    .await
                {
                    Ok(session) => session,
                    Err(error) => {
                        return json_error(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Some(request_id),
                            -32603,
                            error.to_string(),
                        );
                    }
                };
                let subscription = match store
                    .create_subscription(&session.id, &requested, SESSION_TTL.as_secs() as i32)
                    .await
                {
                    Ok(subscription) => subscription,
                    Err(error) => {
                        return json_error(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Some(request_id),
                            -32603,
                            error.to_string(),
                        );
                    }
                };
                let after = match store.latest_notification_sequence().await {
                    Ok(sequence) => sequence,
                    Err(error) => {
                        return json_error(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Some(request_id),
                            -32603,
                            error.to_string(),
                        );
                    }
                };
                (
                    subscription.clone(),
                    persistent_catalog_stream(
                        store,
                        subscription,
                        after,
                        notification_max_lag(),
                        Some(subscription_ref.clone()),
                    )
                    .boxed(),
                )
            }
            None => (
                format!("mcpsub_{}", Uuid::now_v7()),
                catalog_stream(
                    auth.tenant_id.clone(),
                    requested.clone(),
                    Some(subscription_ref.clone()),
                )
                .boxed(),
            ),
        };
        let initial = json!({
            "jsonrpc": "2.0",
            "method": "notifications/subscriptions/acknowledged",
            "params": {
                "notifications": requested,
                "_meta": {
                    "io.modelcontextprotocol/subscriptionId": subscription_ref
                }
            }
        });
        let mut headers = HeaderMap::new();
        headers.insert(
            "mcp-protocol-version",
            HeaderValue::from_static(MODERN_VERSION),
        );
        let initial = stream::once(async move {
            Ok::<Event, Infallible>(Event::default().event("message").data(initial.to_string()))
        });
        return sse_stream(initial.chain(events), headers);
    }
    let is_notification = request.get("id").is_none();
    let tool_name_style = match header_text(&headers, "x-hpa-tool-name-style").as_deref() {
        Some("slug") => ToolNameStyle::Slug,
        _ => ToolNameStyle::Canonical,
    };
    let mcp_span = tracing::info_span!(
        "mcp.request",
        mcp.method.name = %rpc_method,
        tenant.id = %auth.tenant_id,
        mcp.session.id = tracing::field::Empty
    );
    if let Some(session) = header_text(&headers, "mcp-session-id") {
        mcp_span.record("mcp.session.id", session);
    }
    let outcome = handle_request_styled(&service, &auth, era, &request, tool_name_style)
        .instrument(mcp_span)
        .await;
    let mut response_headers = HeaderMap::new();
    if era == Era::Modern {
        response_headers.insert(
            "mcp-protocol-version",
            HeaderValue::from_static(MODERN_VERSION),
        );
    }
    let session = if let Some(session) = outcome.new_session {
        let id = match state.open_session(&auth).await {
            Ok(id) => id,
            Err(error) => {
                return json_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Some(request_id),
                    -32603,
                    error,
                );
            }
        };
        let _ = session;
        response_headers.insert(
            "mcp-session-id",
            HeaderValue::from_str(&id).unwrap_or_else(|_| HeaderValue::from_static("session")),
        );
        Some(id)
    } else {
        header_text(&headers, "mcp-session-id")
    };
    if let Some(ref session) = session
        && era == Era::Legacy
    {
        response_headers.insert(
            "mcp-session-id",
            HeaderValue::from_str(session).unwrap_or_else(|_| HeaderValue::from_static("session")),
        );
    }
    if let Err(error) = persist_task_if_needed(
        &state,
        &auth,
        &rpc_method,
        &request,
        session.as_deref(),
        outcome.response.as_ref(),
    )
    .await
    {
        return json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            Some(request_id),
            -32603,
            error,
        );
    }
    if is_notification {
        let mut response = StatusCode::ACCEPTED.into_response();
        response.headers_mut().extend(response_headers);
        return response;
    }
    if era == Era::Modern
        && rpc_method == "tools/call"
        && let Some(progress_token) = request
            .get("params")
            .and_then(|params| params.get("_meta"))
            .and_then(|meta| meta.get("progressToken"))
    {
        let response = outcome
            .response
            .unwrap_or_else(|| json!({"jsonrpc": "2.0", "id": request_id, "result": {}}));
        let progress = |value: i64| {
            json!({
                "jsonrpc": "2.0",
                "method": "notifications/progress",
                "params": {
                    "progressToken": progress_token,
                    "progress": value,
                    "total": 100
                }
            })
        };
        let frames = vec![progress(0), progress(50), progress(100), response];
        let events = stream::iter(frames.into_iter().map(|frame| {
            Ok::<Event, Infallible>(Event::default().event("message").data(frame.to_string()))
        }));
        let mut headers = HeaderMap::new();
        headers.insert(
            "mcp-protocol-version",
            HeaderValue::from_static(MODERN_VERSION),
        );
        return sse_stream(events, headers);
    }
    let response = outcome
        .response
        .unwrap_or_else(|| json!({"jsonrpc": "2.0", "id": request_id, "result": {}}));
    let status = if era == Era::Modern {
        match response
            .get("error")
            .and_then(|error| error.get("code"))
            .and_then(Value::as_i64)
        {
            Some(-32601) => StatusCode::NOT_FOUND,
            Some(-32020 | -32021) => StatusCode::BAD_REQUEST,
            _ => StatusCode::OK,
        }
    } else {
        StatusCode::OK
    };
    let mut response = (status, Json(response)).into_response();
    response.headers_mut().extend(response_headers);
    response
}

async fn persist_task_if_needed(
    state: &McpGatewayState,
    auth: &crate::mcp::AuthContext,
    method: &str,
    request: &Value,
    session_id: Option<&str>,
    response: Option<&Value>,
) -> Result<(), String> {
    if method != "tools/call" {
        return Ok(());
    }
    let Some(store) = state.persistent_store(auth) else {
        return Ok(());
    };
    let params = request.get("params").unwrap_or(&Value::Null);
    let tool_name = params.get("name").and_then(Value::as_str).unwrap_or("");
    if !matches!(tool_name, "workflow.execute" | "workflow_execute") {
        return Ok(());
    }
    let result = response
        .and_then(|response| response.get("result"))
        .unwrap_or(&Value::Null);
    let structured = result.get("structuredContent").unwrap_or(&Value::Null);
    let Some(task_id) = structured
        .get("taskId")
        .or_else(|| {
            result
                .get("_meta")
                .and_then(|meta| meta.get("io.modelcontextprotocol/task"))
                .and_then(|task| task.get("taskId"))
        })
        .and_then(Value::as_str)
    else {
        return Ok(());
    };
    let status = if result.get("isError").and_then(Value::as_bool) == Some(true) {
        "failed"
    } else {
        match result.get("resultType").and_then(Value::as_str) {
            Some("input_required") => "input_required",
            _ => match structured.get("status").and_then(Value::as_str) {
                Some("running" | "queued" | "leased") => "working",
                Some("waiting_approval") => "input_required",
                Some("succeeded") => "completed",
                Some("failed") => "failed",
                Some("cancelled") => "cancelled",
                _ => "working",
            },
        }
    };
    let execution_id = structured
        .get("executionId")
        .and_then(Value::as_str)
        .unwrap_or(task_id);
    let error = (status == "failed").then(|| {
        json!({
            "message": result
                .get("content")
                .and_then(Value::as_array)
                .and_then(|items| items.first())
                .and_then(|item| item.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("workflow.execute failed")
        })
    });
    store
        .upsert_task(&TaskUpsert {
            id: task_id.to_string(),
            session_id: session_id.map(str::to_string),
            method: method.to_string(),
            status: status.to_string(),
            input: params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({})),
            output: (!structured.is_null()).then(|| structured.clone()),
            error,
            approval: None,
            execution_id: Some(execution_id.to_string()),
            owner_node_id: Some(state.node_id.clone()),
            expires_in_seconds: Some(86_400),
        })
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
}

async fn persistent_task_result(
    state: &McpGatewayState,
    auth: &crate::mcp::AuthContext,
    era: Era,
    method: &str,
    request: &Value,
) -> Result<Option<Value>, RpcError> {
    let Some(store) = state.persistent_store(auth) else {
        return Ok(None);
    };
    let params = request.get("params").unwrap_or(&Value::Null);
    let task_id = params
        .get("taskId")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::new(-32602, format!("{method} requires taskId")))?;
    let existing = store
        .get_task_optional(task_id)
        .await
        .map_err(|error| RpcError::new(-32603, error.to_string()))?;
    let existing = existing.ok_or_else(|| {
        RpcError::new(
            -32602,
            format!("task `{task_id}` was not found or has expired"),
        )
    })?;
    match method {
        "tasks/get" => {
            if !auth.has_scope(SCOPE_READ) {
                return Err(RpcError::new(-32003, "token is missing scope `mcp:read`"));
            }
            Ok(Some(finish(era, persistent_task_value(&existing))))
        }
        "tasks/update" => {
            if !auth.has_scope(SCOPE_EXECUTE) {
                return Err(RpcError::new(
                    -32003,
                    "token is missing scope `mcp:execute`",
                ));
            }
            let update = params
                .get("update")
                .and_then(Value::as_object)
                .ok_or_else(|| RpcError::new(-32602, "tasks/update requires an update object"))?;
            if update.get("status").and_then(Value::as_str) != Some("cancelled") {
                return Err(RpcError::new(
                    -32602,
                    "only `{\"status\":\"cancelled\"}` is supported",
                ));
            }
            let task = store
                .request_task_cancel(task_id)
                .await
                .map_err(persistent_task_error)?;
            Ok(Some(finish(era, persistent_task_value(&task))))
        }
        _ => Ok(None),
    }
}

fn persistent_task_value(task: &PersistentTask) -> Value {
    json!({
        "taskId": task.id,
        "executionId": task.execution_id,
        "status": task.status,
        "input": task.input_json,
        "output": task.output_json,
        "error": task.error_json,
        "waiting": task.approval_json,
        "createdAt": task.created_at.to_string(),
        "updatedAt": task.updated_at.to_string(),
        "expiresAt": task.expires_at.map(|value| value.to_string()),
        "_meta": {
            "io.modelcontextprotocol/task": {
                "taskId": task.id,
                "ownerNodeId": task.owner_node_id
            }
        }
    })
}

fn persistent_task_error(error: McpStoreError) -> RpcError {
    match error {
        McpStoreError::TaskNotFound(_) | McpStoreError::InvalidTaskTransition { .. } => {
            RpcError::new(-32602, error.to_string())
        }
        other => RpcError::new(-32603, other.to_string()),
    }
}

fn validate_modern_headers(headers: &HeaderMap, request: &Value, method: &str) -> Option<Response> {
    let request_id = request.get("id").cloned().unwrap_or(Value::Null);
    let header_method = header_text(headers, "mcp-method");
    if header_method.as_deref() != Some(method) {
        return Some(json_error(
            StatusCode::BAD_REQUEST,
            Some(request_id),
            -32020,
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
                -32020,
                "Mcp-Name header does not match the request name",
            ));
        }
    }
    if method == "tools/call"
        && let Some(params) = request.get("params")
        && params.get("name").and_then(Value::as_str) == Some("test_custom_headers")
        && let Some(body_value) = params
            .get("arguments")
            .and_then(|arguments| arguments.get("value"))
            .and_then(Value::as_str)
    {
        let header = header_text(headers, "mcp-param-value");
        let decoded = match header {
            Some(raw) => {
                if let Some(encoded) = raw
                    .strip_prefix("=?base64?")
                    .and_then(|value| value.strip_suffix("?="))
                {
                    match BASE64.decode(encoded) {
                        Ok(bytes) => String::from_utf8(bytes).ok(),
                        Err(_) => {
                            return Some(json_error(
                                StatusCode::BAD_REQUEST,
                                Some(request_id),
                                -32020,
                                "Mcp-Param-Value header is not valid base64",
                            ));
                        }
                    }
                } else {
                    Some(raw)
                }
            }
            None => None,
        };
        if decoded.as_deref() != Some(body_value) {
            return Some(json_error(
                StatusCode::BAD_REQUEST,
                Some(request_id),
                -32020,
                "Mcp-Param-Value header does not match the argument value",
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
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
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
    json_error_data(status, id, code, message, None)
}

fn json_error_data(
    status: StatusCode,
    id: Option<Value>,
    code: i64,
    message: impl Into<String>,
    data: Option<Value>,
) -> Response {
    let mut error = json!({"code": code, "message": message.into()});
    if let Some(data) = data {
        error["data"] = data;
    }
    let body = json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "error": error,
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
