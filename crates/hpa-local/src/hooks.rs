//! Public webhook ingress.
//!
//! Hooks are machine endpoints: no session, no CSRF. They are guarded by an
//! HMAC signature over `{timestamp}.{body}` with a fixed replay window, and an
//! optional `X-HPA-Event-Id` for idempotent delivery.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use ring::hmac;
use serde_json::{Value, json};

use crate::api::ApiError;
use crate::enterprise::EnterpriseState;
use crate::scheduler::{FireRequest, TriggerError};
use hpa_core::ir::Trigger;
use hpa_core::trigger::{TriggerSpec, parse_trigger};

const REPLAY_WINDOW_SECONDS: i64 = 300;
const MAX_BODY_BYTES: usize = 1_048_576;
const DEFAULT_LIMIT_PER_MINUTE: usize = 120;

struct Window {
    started: Instant,
    count: usize,
}

impl Default for Window {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            count: 0,
        }
    }
}

fn rate_limiter() -> &'static Mutex<HashMap<String, Window>> {
    static LIMITER: OnceLock<Mutex<HashMap<String, Window>>> = OnceLock::new();
    LIMITER.get_or_init(|| Mutex::new(HashMap::new()))
}

fn check_rate(key: &str) -> Result<(), u64> {
    let limit = std::env::var("HPA_HOOK_RATE_LIMIT")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_LIMIT_PER_MINUTE);
    let mut windows = rate_limiter().lock().expect("hook rate limiter");
    let window = windows.entry(key.to_string()).or_default();
    if window.started.elapsed() >= Duration::from_secs(60) {
        *window = Window {
            started: Instant::now(),
            count: 0,
        };
    }
    if window.count >= limit {
        return Err(60u64
            .saturating_sub(window.started.elapsed().as_secs())
            .max(1));
    }
    window.count += 1;
    Ok(())
}

pub fn router() -> Router<EnterpriseState> {
    Router::new()
        .route("/hooks/{workflow_id}/{trigger_id}", post(hook_local))
        .route(
            "/hooks/{tenant_id}/{workflow_id}/{trigger_id}",
            post(hook_tenant),
        )
}

async fn hook_local(
    state: State<EnterpriseState>,
    Path((workflow_id, trigger_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    handle(
        state,
        "local".to_string(),
        workflow_id,
        trigger_id,
        headers,
        body,
    )
    .await
}

async fn hook_tenant(
    state: State<EnterpriseState>,
    Path((tenant_id, workflow_id, trigger_id)): Path<(String, String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    handle(state, tenant_id, workflow_id, trigger_id, headers, body).await
}

async fn handle(
    State(enterprise): State<EnterpriseState>,
    tenant_id: String,
    workflow_id: String,
    trigger_id: String,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if body.len() > MAX_BODY_BYTES {
        return ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "HOOK_BODY_TOO_LARGE",
            format!("webhook body must be at most {MAX_BODY_BYTES} bytes"),
        )
        .into_response();
    }
    match enterprise.tenant_active(&tenant_id).await {
        Ok(true) => {}
        Ok(false) => {
            return ApiError::not_found(format!("tenant `{tenant_id}` is not active"))
                .into_response();
        }
        Err(error) => return error.into_response(),
    }
    if let Err(retry_after) = check_rate(&format!("{tenant_id}/{workflow_id}/{trigger_id}")) {
        let mut response = ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "HOOK_RATE_LIMITED",
            "webhook rate limit exceeded",
        )
        .into_response();
        if let Ok(value) = header::HeaderValue::from_str(&retry_after.to_string()) {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }
        return response;
    }
    let api_state = match enterprise.tenant_api_state(&tenant_id).await {
        Ok(state) => state,
        Err(error) => return error.into_response(),
    };
    let row = match api_state.db.get_trigger(&workflow_id, &trigger_id).await {
        Ok(Some(row)) => row,
        Ok(None) => {
            return ApiError::not_found(format!(
                "trigger `{trigger_id}` of workflow `{workflow_id}` was not found"
            ))
            .into_response();
        }
        Err(error) => return ApiError::internal(error.to_string()).into_response(),
    };
    if row.kind != "webhook" {
        return ApiError::bad_request(
            "HOOK_KIND_INVALID",
            format!(
                "trigger `{trigger_id}` is a `{}` trigger, not a webhook",
                row.kind
            ),
        )
        .into_response();
    }
    if row.enabled == 0 {
        return ApiError::conflict(
            "HOOK_DISABLED",
            format!("trigger `{trigger_id}` is disabled"),
        )
        .into_response();
    }
    let config: Value = serde_json::from_str(&row.config_json).unwrap_or_else(|_| json!({}));
    let secret = match parse_trigger(&Trigger {
        id: trigger_id.clone(),
        trigger_type: hpa_core::ir::TriggerType::Webhook,
        config,
    }) {
        Ok(TriggerSpec::Webhook(spec)) => spec.secret,
        Ok(_) => None,
        Err(message) => {
            return ApiError::internal(format!("stored webhook config is invalid: {message}"))
                .into_response();
        }
    };
    let Some(secret) = secret else {
        return ApiError::new(
            StatusCode::UNAUTHORIZED,
            "HOOK_NO_SECRET",
            "this webhook trigger has no secret; add `config.secret` and publish again",
        )
        .into_response();
    };
    let Some(timestamp) = header_text(&headers, "x-hpa-timestamp") else {
        return ApiError::new(
            StatusCode::UNAUTHORIZED,
            "HOOK_SIGNATURE_REQUIRED",
            "X-HPA-Timestamp header is required",
        )
        .into_response();
    };
    let Some(signature) = header_text(&headers, "x-hpa-signature") else {
        return ApiError::new(
            StatusCode::UNAUTHORIZED,
            "HOOK_SIGNATURE_REQUIRED",
            "X-HPA-Signature header is required",
        )
        .into_response();
    };
    let Ok(timestamp_value) = timestamp.parse::<i64>() else {
        return ApiError::new(
            StatusCode::UNAUTHORIZED,
            "HOOK_TIMESTAMP_INVALID",
            "X-HPA-Timestamp must be unix seconds",
        )
        .into_response();
    };
    let now = crate::scheduler::now_ms().div_euclid(1000);
    if (now - timestamp_value).abs() > REPLAY_WINDOW_SECONDS {
        return ApiError::new(
            StatusCode::UNAUTHORIZED,
            "HOOK_TIMESTAMP_STALE",
            format!("X-HPA-Timestamp must be within {REPLAY_WINDOW_SECONDS}s of now"),
        )
        .into_response();
    }
    let payload = format!("{timestamp}.");
    let mut message = payload.into_bytes();
    message.extend_from_slice(&body);
    if !verify_signature(&secret, &message, &signature) {
        return ApiError::new(
            StatusCode::UNAUTHORIZED,
            "HOOK_SIGNATURE_INVALID",
            "signature does not match the trigger secret",
        )
        .into_response();
    }
    let input = match serde_json::from_slice::<Value>(&body) {
        Ok(value) => value,
        Err(_) => json!({
            "body": String::from_utf8_lossy(&body),
            "contentType": header_text(&headers, "content-type").unwrap_or_default(),
        }),
    };
    let event_id = header_text(&headers, "x-hpa-event-id");
    let request = FireRequest {
        workflow_id: workflow_id.clone(),
        trigger_id: trigger_id.clone(),
        input,
        event_id,
        scheduled_at: None,
        origin: "webhook".to_string(),
    };
    let host = api_state.trigger_host();
    match crate::scheduler::fire_now(&host, request).await {
        Ok(outcome) => (
            StatusCode::ACCEPTED,
            Json(json!({
                "executionId": outcome.execution_id,
                "workflowId": outcome.workflow_id,
                "workflowVersion": outcome.workflow_version,
                "triggerId": outcome.trigger_id,
            })),
        )
            .into_response(),
        Err(TriggerError::Duplicate { event_id }) => ApiError::conflict(
            "HOOK_DUPLICATE",
            format!("event `{event_id}` was already delivered"),
        )
        .into_response(),
        Err(TriggerError::Disabled { .. }) => {
            ApiError::conflict("HOOK_DISABLED", "trigger is disabled").into_response()
        }
        Err(error) => ApiError::internal(error.to_string()).into_response(),
    }
}

fn verify_signature(secret: &str, message: &[u8], provided: &str) -> bool {
    let provided = provided.strip_prefix("sha256=").unwrap_or(provided).trim();
    if provided.len() != 64 || !provided.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return false;
    }
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    let tag = hmac::sign(&key, message);
    let expected = hex(tag.as_ref());
    constant_time_eq(
        expected.as_bytes(),
        provided.to_ascii_lowercase().as_bytes(),
    )
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn header_text(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}
