from __future__ import annotations

import os
from collections.abc import Iterator, Mapping
from contextlib import contextmanager

from opentelemetry import metrics, trace
from opentelemetry._logs import SeverityNumber
from opentelemetry.context import Context
from opentelemetry.exporter.otlp.proto.http._log_exporter import OTLPLogExporter
from opentelemetry.exporter.otlp.proto.http.metric_exporter import OTLPMetricExporter
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
from opentelemetry.propagate import set_global_textmap
from opentelemetry.sdk._logs import LoggerProvider
from opentelemetry.sdk._logs.export import BatchLogRecordProcessor
from opentelemetry.sdk.metrics import MeterProvider
from opentelemetry.sdk.metrics.export import PeriodicExportingMetricReader
from opentelemetry.sdk.resources import Resource
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor
from opentelemetry.sdk.trace.sampling import ParentBased, TraceIdRatioBased
from opentelemetry.trace import Span, SpanKind, Tracer
from opentelemetry.trace.propagation.tracecontext import TraceContextTextMapPropagator

TRACER_NAME = "hpa-ai-worker"


class WorkerTelemetry:
    def __init__(
        self,
        provider: TracerProvider,
        meter_provider: MeterProvider,
        logger_provider: LoggerProvider,
        propagator: TraceContextTextMapPropagator,
    ) -> None:
        self.provider = provider
        self.meter_provider = meter_provider
        self.logger_provider = logger_provider
        self.propagator = propagator
        meter = meter_provider.get_meter(TRACER_NAME)
        self.sessions = meter.create_counter(
            "hpa_ai_worker_sessions_total",
            description="AI worker sessions completed by status",
        )
        self.session_seconds = meter.create_histogram(
            "hpa_ai_worker_session_seconds",
            unit="s",
            description="AI worker session wall-clock duration",
        )
        self.tool_calls = meter.create_counter(
            "hpa_ai_worker_tool_calls_total",
            description="AI worker tool calls",
        )
        self.tokens = meter.create_counter(
            "hpa_ai_worker_tokens_total",
            description="AI worker model tokens by direction",
        )
        self.logger = logger_provider.get_logger(TRACER_NAME)

    def tracer(self) -> Tracer:
        return self.provider.get_tracer(TRACER_NAME)

    def extract(self, traceparent: str | None) -> Context | None:
        if not traceparent:
            return None
        context = self.propagator.extract({"traceparent": traceparent})
        return context if trace.get_current_span(context).get_span_context().is_valid else None

    def inject(self, carrier: dict[str, str]) -> None:
        self.propagator.inject(carrier)

    def shutdown(self) -> None:
        self.provider.shutdown()
        self.meter_provider.shutdown()
        self.logger_provider.shutdown()

    def record_session_outcome(
        self,
        status: str,
        duration_seconds: float,
        usage: Mapping[str, int | float],
    ) -> None:
        attributes = {"status": status}
        self.sessions.add(1, attributes)
        self.session_seconds.record(max(0.0, duration_seconds), attributes)
        self.tool_calls.add(
            int(usage.get("toolCalls", 0)),
            {"status": status},
        )
        self.tokens.add(
            int(usage.get("promptTokens", 0)),
            {"direction": "input"},
        )
        self.tokens.add(
            int(usage.get("completionTokens", 0)),
            {"direction": "output"},
        )
        self.log(
            SeverityNumber.INFO if status == "completed" else SeverityNumber.WARN,
            "agent session finished",
            {
                "event.name": "agent.session.finished",
                "agent.status": status,
                "duration_ms": int(max(0.0, duration_seconds) * 1000),
                "tool_calls": int(usage.get("toolCalls", 0)),
                "prompt_tokens": int(usage.get("promptTokens", 0)),
                "completion_tokens": int(usage.get("completionTokens", 0)),
            },
        )

    def log(
        self,
        severity: SeverityNumber,
        body: str,
        attributes: Mapping[str, str | int | float | bool],
    ) -> None:
        self.logger.emit(
            severity_number=severity,
            severity_text=severity.name,
            body=body,
            attributes=dict(attributes),
        )


def init_telemetry(service_name: str = "hpa-ai-worker") -> WorkerTelemetry:
    propagator = TraceContextTextMapPropagator()
    set_global_textmap(propagator)
    endpoint = os.getenv("HPA_OTEL_ENDPOINT") or os.getenv("OTEL_EXPORTER_OTLP_ENDPOINT")
    enabled = os.getenv("HPA_OTEL_ENABLED", "").lower() in {"1", "true"} or bool(endpoint)
    ratio = float(os.getenv("HPA_OTEL_SAMPLE_RATIO", "1"))
    resource = Resource.create(
        {
            "service.name": service_name,
            "service.version": "0.1.0",
            "deployment.environment.name": os.getenv("HPA_DEPLOYMENT_ENV", "local"),
        }
    )
    provider = TracerProvider(
        resource=resource,
        sampler=ParentBased(TraceIdRatioBased(max(0.0, min(1.0, ratio)))),
    )
    meter_provider = MeterProvider(resource=resource)
    logger_provider = LoggerProvider(resource=resource)
    if enabled:
        base = _base_endpoint(endpoint or "http://127.0.0.1:4318")
        trace_endpoint = _signal_endpoint("HPA_OTEL_TRACES_ENDPOINT", base, "traces")
        headers = _parse_headers(os.getenv("HPA_OTEL_HEADERS", ""))
        exporter = OTLPSpanExporter(endpoint=trace_endpoint, headers=headers)
        queue_size = max(
            64,
            min(65536, int(os.getenv("HPA_OTEL_QUEUE_SIZE", "2048"))),
        )
        processor = BatchSpanProcessor(
            exporter,
            max_queue_size=queue_size,
            schedule_delay_millis=1000,
            export_timeout_millis=3000,
            max_export_batch_size=min(256, queue_size),
        )
        provider.add_span_processor(processor)
        metrics_endpoint = _signal_endpoint("HPA_OTEL_METRICS_ENDPOINT", base, "metrics")
        metric_exporter = OTLPMetricExporter(
            endpoint=metrics_endpoint,
            headers=headers or None,
            timeout=3,
        )
        metric_interval = max(
            1,
            min(
                60,
                int(os.getenv("HPA_OTEL_METRIC_INTERVAL_SECONDS", "5")),
            ),
        )
        metric_reader = PeriodicExportingMetricReader(
            metric_exporter,
            export_interval_millis=metric_interval * 1000,
            export_timeout_millis=3000,
        )
        meter_provider = MeterProvider(
            metric_readers=[metric_reader],
            resource=resource,
        )
        logs_endpoint = _signal_endpoint("HPA_OTEL_LOGS_ENDPOINT", base, "logs")
        log_exporter = OTLPLogExporter(
            endpoint=logs_endpoint,
            headers=headers or None,
            timeout=3,
        )
        log_processor = BatchLogRecordProcessor(
            log_exporter,
            schedule_delay_millis=1000,
            max_queue_size=queue_size,
            max_export_batch_size=min(256, queue_size),
            export_timeout_millis=3000,
        )
        logger_provider.add_log_record_processor(log_processor)
    trace.set_tracer_provider(provider)
    metrics.set_meter_provider(meter_provider)
    return WorkerTelemetry(provider, meter_provider, logger_provider, propagator)


def _base_endpoint(endpoint: str) -> str:
    value = endpoint.rstrip("/")
    for suffix in ("/v1/traces", "/v1/metrics", "/v1/logs"):
        if value.endswith(suffix):
            return value[: -len(suffix)]
    return value


def _signal_endpoint(variable: str, base: str, signal: str) -> str:
    return os.getenv(variable) or f"{base}/v1/{signal}"


def _parse_headers(raw: str) -> dict[str, str]:
    headers: dict[str, str] = {}
    for item in raw.split(","):
        if "=" not in item:
            continue
        key, value = item.split("=", 1)
        if key.strip():
            headers[key.strip()] = value.strip()
    return headers


@contextmanager
def session_span(
    telemetry: WorkerTelemetry,
    name: str,
    *,
    parent: Context | None,
    attributes: Mapping[str, str | int],
) -> Iterator[Span]:
    with telemetry.tracer().start_as_current_span(
        name,
        context=parent,
        kind=SpanKind.SERVER,
        attributes=dict(attributes),
    ) as span:
        yield span


def traceparent_from_current(telemetry: WorkerTelemetry) -> str | None:
    carrier: dict[str, str] = {}
    telemetry.inject(carrier)
    return carrier.get("traceparent")
