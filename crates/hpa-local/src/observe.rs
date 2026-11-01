//! Operational endpoints: Prometheus metrics and readiness.

use std::collections::BTreeMap;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;

use crate::api::ApiError;
use crate::enterprise::EnterpriseState;
use crate::metrics::{metrics, now_ms};

pub fn router() -> Router<EnterpriseState> {
    Router::new()
        .route("/metrics", get(metrics_endpoint))
        .route("/api/ready", get(ready_endpoint))
        .route("/ready", get(ready_endpoint))
}

async fn metrics_endpoint(State(state): State<EnterpriseState>, headers: HeaderMap) -> Response {
    if state.auth_required()
        && let Err(error) = state.oauth_admin(&headers).await
    {
        return error.into_response();
    }
    if let Err(error) = collect_tenant_gauges(&state).await {
        // Scrape must not fail because one tenant is unavailable.
        tracing::warn!(error = %error.message, "cannot collect tenant gauges");
    }
    let body = metrics().render();
    let mut response = body.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}

async fn collect_tenant_gauges(state: &EnterpriseState) -> Result<(), ApiError> {
    for tenant in state.active_tenants().await? {
        let Ok(api) = state.tenant_api_state(&tenant).await else {
            continue;
        };
        let Ok(stats) = api.db.stats().await else {
            continue;
        };
        metrics().set(
            "hpa_workflows_total",
            &[("tenant", &tenant)],
            stats.workflows as f64,
        );
        metrics().set(
            "hpa_executions_running",
            &[("tenant", &tenant)],
            stats.running as f64,
        );
        metrics().set(
            "hpa_executions_waiting_approval",
            &[("tenant", &tenant)],
            stats.waiting_approval as f64,
        );
        metrics().set(
            "hpa_executions_succeeded_total",
            &[("tenant", &tenant)],
            stats.succeeded as f64,
        );
        metrics().set(
            "hpa_executions_failed_total",
            &[("tenant", &tenant)],
            stats.failed as f64,
        );
        let Ok(triggers) = api.db.list_triggers().await else {
            continue;
        };
        let mut enabled: BTreeMap<String, i64> = BTreeMap::new();
        for trigger in triggers.iter().filter(|trigger| trigger.enabled != 0) {
            *enabled.entry(trigger.kind.clone()).or_insert(0) += 1;
        }
        for kind in ["schedule", "webhook", "event", "mcp"] {
            metrics().set(
                "hpa_triggers_enabled",
                &[("tenant", &tenant), ("kind", kind)],
                enabled.get(kind).copied().unwrap_or(0) as f64,
            );
        }
    }
    Ok(())
}

async fn ready_endpoint(State(state): State<EnterpriseState>) -> Response {
    let mut checks: BTreeMap<&'static str, &'static str> = BTreeMap::new();
    let mut ready = true;

    match state.control_ping().await {
        Ok(()) => checks.insert("controlDb", "ok"),
        Err(_) => {
            ready = false;
            checks.insert("controlDb", "error")
        }
    };

    match state.tenant_ping("local").await {
        Ok(()) => checks.insert("tenantDb", "ok"),
        Err(_) => {
            ready = false;
            checks.insert("tenantDb", "error")
        }
    };

    let fresh = state.oauth().jwks().await.map(|jwks| {
        jwks.get("keys")
            .and_then(|keys| keys.as_array())
            .is_some_and(|keys| !keys.is_empty())
    });
    match fresh {
        Ok(true) => checks.insert("signingKey", "ok"),
        _ => {
            ready = false;
            checks.insert("signingKey", "missing")
        }
    };

    let last_tick = metrics().last_scheduler_tick();
    let tick_age = if last_tick == 0 {
        None
    } else {
        Some(now_ms() - last_tick)
    };
    match tick_age {
        Some(age) if age <= 15_000 => checks.insert("scheduler", "ok"),
        Some(_) => {
            ready = false;
            checks.insert("scheduler", "stale")
        }
        None => {
            ready = false;
            checks.insert("scheduler", "starting")
        }
    };

    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let mut response = (
        status,
        Json(json!({
            "ready": ready,
            "checks": checks,
            "schedulerTickAgeMs": tick_age,
            "uptimeSeconds": metrics().uptime_seconds().round() as i64,
        })),
    )
        .into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}
