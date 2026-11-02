use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::http::{HeaderMap, HeaderName, HeaderValue};
use opentelemetry::global;
use opentelemetry::propagation::{Extractor, Injector};
use opentelemetry::trace::{SpanContext, TraceContextExt, TracerProvider as _};
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::{WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::{
    BatchConfigBuilder as LogBatchConfigBuilder, BatchLogProcessor, SdkLoggerProvider,
};
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::{
    BatchConfigBuilder as TraceBatchConfigBuilder, BatchSpanProcessor, Sampler, SdkTracerProvider,
};
use thiserror::Error;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[derive(Debug, Error)]
pub enum TelemetryError {
    #[error("OTLP exporter configuration failed: {0}")]
    Exporter(String),
    #[error("tracing subscriber initialization failed: {0}")]
    Subscriber(String),
}

pub struct TelemetryGuard {
    tracer_provider: Option<SdkTracerProvider>,
    meter_provider: Option<SdkMeterProvider>,
    logger_provider: Option<SdkLoggerProvider>,
}

static OTEL_ENABLED: AtomicBool = AtomicBool::new(false);

pub fn enabled() -> bool {
    OTEL_ENABLED.load(Ordering::Relaxed)
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Some(provider) = self.tracer_provider.take() {
            let _ = provider.shutdown();
        }
        if let Some(provider) = self.meter_provider.take() {
            let _ = provider.shutdown();
        }
        if let Some(provider) = self.logger_provider.take() {
            let _ = provider.shutdown();
        }
    }
}

pub fn init(service_name: &str) -> Result<TelemetryGuard, TelemetryError> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "hpa=info,tower_http=info".into());
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_writer(std::io::stderr);
    let enabled = std::env::var("HPA_OTEL_ENABLED")
        .map(|value| matches!(value.as_str(), "1" | "true"))
        .unwrap_or(false)
        || std::env::var("HPA_OTEL_ENDPOINT").is_ok()
        || std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").is_ok();
    global::set_text_map_propagator(TraceContextPropagator::new());
    if !enabled {
        OTEL_ENABLED.store(false, Ordering::Relaxed);
        tracing_subscriber::registry()
            .with(filter)
            .with(fmt_layer)
            .try_init()
            .map_err(|error| TelemetryError::Subscriber(error.to_string()))?;
        return Ok(TelemetryGuard {
            tracer_provider: None,
            meter_provider: None,
            logger_provider: None,
        });
    }
    OTEL_ENABLED.store(true, Ordering::Relaxed);

    let base_endpoint = std::env::var("HPA_OTEL_ENDPOINT")
        .or_else(|_| std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT"))
        .unwrap_or_else(|_| "http://127.0.0.1:4318".to_string());
    let headers = otlp_headers();
    let trace_endpoint = signal_endpoint("HPA_OTEL_TRACES_ENDPOINT", &base_endpoint, "traces");
    let mut span_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(trace_endpoint)
        .with_timeout(Duration::from_secs(3));
    if !headers.is_empty() {
        span_exporter = span_exporter.with_headers(headers.clone());
    }
    let span_exporter = span_exporter
        .build()
        .map_err(|error| TelemetryError::Exporter(error.to_string()))?;
    let queue_size = env_usize("HPA_OTEL_QUEUE_SIZE", 2048).clamp(64, 65_536);
    let batch_size = env_usize("HPA_OTEL_BATCH_SIZE", 256).clamp(1, queue_size);
    let span_processor = BatchSpanProcessor::builder(span_exporter)
        .with_batch_config(trace_batch_config(queue_size, batch_size))
        .build();
    let sampler_ratio = std::env::var("HPA_OTEL_SAMPLE_RATIO")
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(1.0)
        .clamp(0.0, 1.0);
    let resource = Resource::builder()
        .with_service_name(service_name.to_string())
        .with_attribute(opentelemetry::KeyValue::new(
            "service.version",
            env!("CARGO_PKG_VERSION"),
        ))
        .with_attribute(opentelemetry::KeyValue::new(
            "deployment.environment.name",
            std::env::var("HPA_DEPLOYMENT_ENV").unwrap_or_else(|_| "local".to_string()),
        ))
        .build();
    let tracer_provider = SdkTracerProvider::builder()
        .with_resource(resource.clone())
        .with_sampler(Sampler::parent_based(Sampler::TraceIdRatioBased(
            sampler_ratio,
        )))
        .with_span_processor(span_processor)
        .build();
    global::set_tracer_provider(tracer_provider.clone());

    let metrics_endpoint = signal_endpoint("HPA_OTEL_METRICS_ENDPOINT", &base_endpoint, "metrics");
    let mut metric_exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_http()
        .with_endpoint(metrics_endpoint)
        .with_timeout(Duration::from_secs(3));
    if !headers.is_empty() {
        metric_exporter = metric_exporter.with_headers(headers.clone());
    }
    let metric_exporter = metric_exporter
        .build()
        .map_err(|error| TelemetryError::Exporter(error.to_string()))?;
    let metric_interval = env_usize("HPA_OTEL_METRIC_INTERVAL_SECONDS", 5).clamp(1, 60);
    let metric_reader = PeriodicReader::builder(metric_exporter)
        .with_interval(Duration::from_secs(metric_interval as u64))
        .build();
    let meter_provider = SdkMeterProvider::builder()
        .with_resource(resource.clone())
        .with_reader(metric_reader)
        .build();
    global::set_meter_provider(meter_provider.clone());

    let logs_endpoint = signal_endpoint("HPA_OTEL_LOGS_ENDPOINT", &base_endpoint, "logs");
    let mut log_exporter = opentelemetry_otlp::LogExporter::builder()
        .with_http()
        .with_endpoint(logs_endpoint)
        .with_timeout(Duration::from_secs(3));
    if !headers.is_empty() {
        log_exporter = log_exporter.with_headers(headers);
    }
    let log_exporter = log_exporter
        .build()
        .map_err(|error| TelemetryError::Exporter(error.to_string()))?;
    let log_processor = BatchLogProcessor::builder(log_exporter)
        .with_batch_config(log_batch_config(queue_size, batch_size))
        .build();
    let logger_provider = SdkLoggerProvider::builder()
        .with_resource(resource)
        .with_log_processor(log_processor)
        .build();
    let tracer = tracer_provider.tracer("hpa-runtime");
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .with(tracing_opentelemetry::layer().with_tracer(tracer))
        .with(OpenTelemetryTracingBridge::new(&logger_provider))
        .try_init()
        .map_err(|error| TelemetryError::Subscriber(error.to_string()))?;
    Ok(TelemetryGuard {
        tracer_provider: Some(tracer_provider),
        meter_provider: Some(meter_provider),
        logger_provider: Some(logger_provider),
    })
}

fn otlp_headers() -> HashMap<String, String> {
    let mut headers = HashMap::new();
    let Ok(raw_headers) = std::env::var("HPA_OTEL_HEADERS") else {
        return headers;
    };
    for item in raw_headers.split(',') {
        let Some((name, value)) = item.split_once('=') else {
            continue;
        };
        if !name.trim().is_empty() {
            headers.insert(name.trim().to_string(), value.trim().to_string());
        }
    }
    headers
}

fn signal_endpoint(variable: &str, base: &str, signal: &str) -> String {
    if let Ok(endpoint) = std::env::var(variable) {
        return endpoint;
    }
    let base = base
        .trim_end_matches('/')
        .trim_end_matches("/v1/traces")
        .trim_end_matches("/v1/metrics")
        .trim_end_matches("/v1/logs");
    format!("{base}/v1/{signal}")
}

fn trace_batch_config(
    queue_size: usize,
    batch_size: usize,
) -> opentelemetry_sdk::trace::BatchConfig {
    TraceBatchConfigBuilder::default()
        .with_max_queue_size(queue_size)
        .with_max_export_batch_size(batch_size)
        .with_scheduled_delay(Duration::from_secs(1))
        .build()
}

fn log_batch_config(queue_size: usize, batch_size: usize) -> opentelemetry_sdk::logs::BatchConfig {
    LogBatchConfigBuilder::default()
        .with_max_queue_size(queue_size)
        .with_max_export_batch_size(batch_size)
        .with_scheduled_delay(Duration::from_secs(1))
        .build()
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

struct HeaderCarrier<'a>(&'a HeaderMap);

impl Extractor for HeaderCarrier<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        HeaderName::from_bytes(key.as_bytes())
            .ok()
            .and_then(|name| self.0.get(name))
            .and_then(|value| value.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(HeaderName::as_str).collect()
    }
}

struct MutableHeaderCarrier<'a>(&'a mut HeaderMap);

impl Injector for MutableHeaderCarrier<'_> {
    fn set(&mut self, key: &str, value: String) {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(key.as_bytes()),
            HeaderValue::from_str(&value),
        ) {
            self.0.insert(name, value);
        }
    }
}

pub fn extract_context(headers: &HeaderMap) -> opentelemetry::Context {
    global::get_text_map_propagator(|propagator| propagator.extract(&HeaderCarrier(headers)))
}

pub fn current_context() -> opentelemetry::Context {
    opentelemetry::Context::current()
}

pub fn inject_current_context(headers: &mut HeaderMap) {
    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&current_context(), &mut MutableHeaderCarrier(headers));
    });
}

pub fn current_trace_id() -> Option<String> {
    let context = current_context();
    let span = context.span();
    let span_context = span.span_context();
    span_context
        .is_valid()
        .then(|| span_context.trace_id().to_string())
}

pub fn trace_context(traceparent: &str) -> opentelemetry::Context {
    let mut headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(traceparent) {
        headers.insert("traceparent", value);
    }
    extract_context(&headers)
}

pub fn valid_traceparent(value: &str) -> bool {
    let context = trace_context(value);
    context.span().span_context().is_remote() && context.span().span_context().is_valid()
}

pub fn span_context() -> Option<SpanContext> {
    let context = current_context();
    let span = context.span();
    let context = span.span_context();
    context.is_valid().then(|| context.clone())
}
