//! In-process metrics with a Prometheus text exposition.
//!
//! Everything here is process-local: one `hpa serve` process owns its counters.
//! Multi-node deployments aggregate at the scraper, not inside the runtime.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{MatchedPath, Request};
use axum::middleware::Next;
use axum::response::Response;

/// Latency buckets in seconds, shared by every histogram.
const BUCKETS: [f64; 13] = [
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0,
];

struct Histogram {
    counts: [u64; BUCKETS.len()],
    sum: f64,
    count: u64,
}

impl Default for Histogram {
    fn default() -> Self {
        Self {
            counts: [0; BUCKETS.len()],
            sum: 0.0,
            count: 0,
        }
    }
}

pub struct Metrics {
    counters: Mutex<BTreeMap<(String, String), f64>>,
    gauges: Mutex<BTreeMap<(String, String), f64>>,
    histograms: Mutex<BTreeMap<(String, String), Histogram>>,
    last_tick_ms: AtomicI64,
    started_at: i64,
}

pub fn metrics() -> &'static Metrics {
    static METRICS: OnceLock<Metrics> = OnceLock::new();
    METRICS.get_or_init(Metrics::new)
}

impl Metrics {
    fn new() -> Self {
        Self {
            counters: Mutex::new(BTreeMap::new()),
            gauges: Mutex::new(BTreeMap::new()),
            histograms: Mutex::new(BTreeMap::new()),
            last_tick_ms: AtomicI64::new(0),
            started_at: now_ms(),
        }
    }

    pub fn inc(&self, name: &str, labels: &[(&str, &str)]) {
        self.inc_by(name, labels, 1.0);
    }

    pub fn inc_by(&self, name: &str, labels: &[(&str, &str)], value: f64) {
        let key = (name.to_string(), label_key(labels));
        let mut counters = self.counters.lock().expect("metrics counters");
        *counters.entry(key).or_insert(0.0) += value;
    }

    pub fn set(&self, name: &str, labels: &[(&str, &str)], value: f64) {
        let key = (name.to_string(), label_key(labels));
        let mut gauges = self.gauges.lock().expect("metrics gauges");
        gauges.insert(key, value);
    }

    pub fn observe(&self, name: &str, labels: &[(&str, &str)], value: f64) {
        let key = (name.to_string(), label_key(labels));
        let mut histograms = self.histograms.lock().expect("metrics histograms");
        let histogram = histograms.entry(key).or_default();
        histogram.sum += value;
        histogram.count += 1;
        for (index, bound) in BUCKETS.iter().enumerate() {
            if value <= *bound {
                histogram.counts[index] += 1;
            }
        }
    }

    pub fn mark_scheduler_tick(&self, at_ms: i64) {
        self.last_tick_ms.store(at_ms, Ordering::Relaxed);
    }

    pub fn last_scheduler_tick(&self) -> i64 {
        self.last_tick_ms.load(Ordering::Relaxed)
    }

    pub fn uptime_seconds(&self) -> f64 {
        (now_ms() - self.started_at) as f64 / 1000.0
    }

    /// Prometheus text exposition (version 0.0.4).
    pub fn render(&self) -> String {
        self.set("hpa_process_uptime_seconds", &[], self.uptime_seconds());
        self.set(
            "hpa_build_info",
            &[("version", env!("CARGO_PKG_VERSION"))],
            1.0,
        );
        let last_tick = self.last_tick_ms.load(Ordering::Relaxed);
        if last_tick > 0 {
            self.set(
                "hpa_scheduler_last_tick_timestamp_seconds",
                &[],
                last_tick as f64 / 1000.0,
            );
        }
        let mut out = String::with_capacity(4096);
        {
            let counters = self.counters.lock().expect("metrics counters");
            render_samples(&mut out, &counters, "counter");
        }
        {
            let gauges = self.gauges.lock().expect("metrics gauges");
            render_samples(&mut out, &gauges, "gauge");
        }
        {
            let histograms = self.histograms.lock().expect("metrics histograms");
            let mut current = "";
            for ((name, labels), histogram) in histograms.iter() {
                if current != name {
                    if !current.is_empty() {
                        out.push('\n');
                    }
                    current = name;
                    if let Some(help) = help_for(name) {
                        out.push_str(&format!("# HELP {name} {help}\n"));
                    }
                    out.push_str(&format!("# TYPE {name} histogram\n"));
                }
                for (index, bound) in BUCKETS.iter().enumerate() {
                    let merged = merge_labels(labels, &format!("le=\"{bound}\""));
                    out.push_str(&format!(
                        "{name}_bucket{merged} {}\n",
                        histogram.counts[index]
                    ));
                }
                let merged = merge_labels(labels, "le=\"+Inf\"");
                out.push_str(&format!("{name}_bucket{merged} {}\n", histogram.count));
                out.push_str(&format!(
                    "{name}_sum{} {}\n",
                    braces(labels),
                    trim_float(histogram.sum)
                ));
                out.push_str(&format!(
                    "{name}_count{} {}\n",
                    braces(labels),
                    histogram.count
                ));
            }
        }
        out
    }
}

fn render_samples(out: &mut String, samples: &BTreeMap<(String, String), f64>, kind: &str) {
    let mut current = "";
    for ((name, labels), value) in samples {
        if current != name {
            if !current.is_empty() {
                out.push('\n');
            }
            current = name;
            if let Some(help) = help_for(name) {
                out.push_str(&format!("# HELP {name} {help}\n"));
            }
            out.push_str(&format!("# TYPE {name} {kind}\n"));
        }
        out.push_str(&format!(
            "{name}{} {}\n",
            braces(labels),
            trim_float(*value)
        ));
    }
    if !current.is_empty() {
        out.push('\n');
    }
}

fn braces(labels: &str) -> String {
    if labels.is_empty() {
        String::new()
    } else {
        format!("{{{labels}}}")
    }
}

fn merge_labels(labels: &str, extra: &str) -> String {
    if labels.is_empty() {
        format!("{{{extra}}}")
    } else {
        format!("{{{labels},{extra}}}")
    }
}

fn label_key(labels: &[(&str, &str)]) -> String {
    let mut sorted = labels.to_vec();
    sorted.sort_by(|left, right| left.0.cmp(right.0));
    sorted
        .iter()
        .map(|(key, value)| format!("{key}=\"{}\"", escape(value)))
        .collect::<Vec<_>>()
        .join(",")
}

fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn trim_float(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{value:.6}")
    }
}

fn help_for(name: &str) -> Option<&'static str> {
    let help = match name {
        "hpa_http_requests_total" => "HTTP requests handled, by route and status",
        "hpa_http_request_seconds" => "HTTP request latency in seconds",
        "hpa_workflow_executions_total" => "Workflow executions finished, by status",
        "hpa_workflow_execution_seconds" => "Wall-clock duration of finished workflow executions",
        "hpa_node_executions_total" => "Node steps executed, by node type and status",
        "hpa_node_seconds" => "Node step latency in seconds, by node type",
        "hpa_scheduler_tick_seconds" => "Duration of one scheduler pass",
        "hpa_scheduler_ticks_total" => "Scheduler passes executed",
        "hpa_scheduler_last_tick_timestamp_seconds" => "Unix time of the last scheduler pass",
        "hpa_scheduler_fires_total" => "Executions started by the scheduler",
        "hpa_trigger_fires_total" => "Trigger fires, by kind and result",
        "hpa_mcp_requests_total" => "MCP gateway requests, by method and outcome",
        "hpa_mcp_request_seconds" => "MCP gateway request latency in seconds",
        "hpa_workflows_total" => "Workflows known to the tenant",
        "hpa_executions_running" => "Executions currently running",
        "hpa_executions_waiting_approval" => "Executions waiting for approval",
        "hpa_executions_succeeded_total" => "Succeeded executions stored in the tenant",
        "hpa_executions_failed_total" => "Failed executions stored in the tenant",
        "hpa_triggers_enabled" => "Enabled triggers, by kind",
        "hpa_process_uptime_seconds" => "Process uptime in seconds",
        "hpa_build_info" => "Build information",
        _ => return None,
    };
    Some(help)
}

/// Marker inserted by the tenant gate so the inner router can label routes
/// with their public `/api` prefix.
#[derive(Clone)]
pub struct TenantApiSurface;

/// Middleware that records request count and latency per matched route.
pub async fn track_http(request: Request, next: Next) -> Response {
    let started = std::time::Instant::now();
    let path = request.uri().path().to_string();
    let matched = request
        .extensions()
        .get::<MatchedPath>()
        .map(|path| path.as_str().to_string());
    // `/api/{*rest}` is the tenant gate: the tenant router records the real
    // route, so counting the wildcard as well would double count.
    let tenant_api = request.extensions().get::<TenantApiSurface>().is_some();
    let route = match matched {
        Some(route) if route == "/api/{*rest}" => {
            return next.run(request).await;
        }
        Some(route) if route.starts_with("/api") => route,
        Some(route) if tenant_api || path.starts_with("/api/") => format!("/api{route}"),
        Some(route) => route,
        None => "unmatched".to_string(),
    };
    let method = request.method().as_str().to_string();
    let response = next.run(request).await;
    let status = response.status().as_u16().to_string();
    metrics().inc(
        "hpa_http_requests_total",
        &[("route", &route), ("method", &method), ("status", &status)],
    );
    metrics().observe(
        "hpa_http_request_seconds",
        &[("route", &route)],
        started.elapsed().as_secs_f64(),
    );
    response
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_and_histograms_render_prometheus_text() {
        let metrics = Metrics::new();
        metrics.inc("hpa_workflow_executions_total", &[("status", "succeeded")]);
        metrics.inc_by(
            "hpa_workflow_executions_total",
            &[("status", "succeeded")],
            2.0,
        );
        metrics.set("hpa_executions_running", &[("tenant", "local")], 3.0);
        metrics.observe("hpa_http_request_seconds", &[("route", "/api/x")], 0.02);
        let text = metrics.render();

        assert!(text.contains("# TYPE hpa_workflow_executions_total counter"));
        assert!(text.contains("hpa_workflow_executions_total{status=\"succeeded\"} 3"));
        assert!(text.contains("# TYPE hpa_executions_running gauge"));
        assert!(text.contains("hpa_executions_running{tenant=\"local\"} 3"));
        assert!(text.contains("# TYPE hpa_http_request_seconds histogram"));
        assert!(text.contains("hpa_http_request_seconds_bucket{route=\"/api/x\",le=\"0.005\"} 0"));
        assert!(text.contains("hpa_http_request_seconds_bucket{route=\"/api/x\",le=\"0.025\"} 1"));
        assert!(text.contains("hpa_http_request_seconds_bucket{route=\"/api/x\",le=\"+Inf\"} 1"));
        assert!(text.contains("hpa_http_request_seconds_count{route=\"/api/x\"} 1"));
        assert!(text.contains("# TYPE hpa_build_info gauge"));
    }

    #[test]
    fn label_values_are_escaped() {
        let metrics = Metrics::new();
        metrics.inc("hpa_mcp_requests_total", &[("method", "tools/\"call\"")]);
        let text = metrics.render();
        assert!(text.contains("method=\"tools/\\\"call\\\"\""), "{text}");
    }
}
