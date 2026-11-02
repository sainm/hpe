from __future__ import annotations

import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from opentelemetry import trace
from opentelemetry.proto.collector.logs.v1.logs_service_pb2 import (
    ExportLogsServiceRequest,
    ExportLogsServiceResponse,
)
from opentelemetry.proto.collector.metrics.v1.metrics_service_pb2 import (
    ExportMetricsServiceRequest,
    ExportMetricsServiceResponse,
)
from opentelemetry.proto.collector.trace.v1.trace_service_pb2 import (
    ExportTraceServiceRequest,
    ExportTraceServiceResponse,
)

from hpa_ai_worker.telemetry import init_telemetry, session_span


class _Collector(BaseHTTPRequestHandler):
    trace_requests: list[ExportTraceServiceRequest] = []
    metric_requests: list[ExportMetricsServiceRequest] = []
    log_requests: list[ExportLogsServiceRequest] = []
    paths: list[str] = []

    def do_POST(self) -> None:  # noqa: N802
        self.__class__.paths.append(self.path)
        body = self.rfile.read(int(self.headers.get("content-length", "0")))
        if self.path == "/v1/traces":
            request = ExportTraceServiceRequest()
            request.ParseFromString(body)
            self.__class__.trace_requests.append(request)
            response = ExportTraceServiceResponse().SerializeToString()
        elif self.path == "/v1/metrics":
            request = ExportMetricsServiceRequest()
            request.ParseFromString(body)
            self.__class__.metric_requests.append(request)
            response = ExportMetricsServiceResponse().SerializeToString()
        elif self.path == "/v1/logs":
            request = ExportLogsServiceRequest()
            request.ParseFromString(body)
            self.__class__.log_requests.append(request)
            response = ExportLogsServiceResponse().SerializeToString()
        else:
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header("content-type", "application/x-protobuf")
        self.send_header("content-length", str(len(response)))
        self.end_headers()
        self.wfile.write(response)

    def log_message(self, format: str, *args: object) -> None:
        del format, args


def test_python_worker_exports_span_with_rust_parent_context(monkeypatch) -> None:
    _Collector.paths = []
    _Collector.trace_requests = []
    _Collector.metric_requests = []
    _Collector.log_requests = []
    server = ThreadingHTTPServer(("127.0.0.1", 0), _Collector)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    monkeypatch.setenv("HPA_OTEL_ENABLED", "1")
    monkeypatch.setenv(
        "HPA_OTEL_ENDPOINT",
        f"http://127.0.0.1:{server.server_port}",
    )
    monkeypatch.setenv("HPA_OTEL_QUEUE_SIZE", "64")
    monkeypatch.setenv("HPA_OTEL_METRIC_INTERVAL_SECONDS", "1")
    telemetry = init_telemetry()
    trace_id = "4bf92f3577b34da6a3ce929d0e0e4736"
    parent = telemetry.extract(f"00-{trace_id}-00f067aa0ba902b7-01")
    try:
        with (
            session_span(
                telemetry,
                "agent.session",
                parent=parent,
                attributes={"session.id": "session-otel"},
            ),
            trace.get_tracer("hpa-ai-worker").start_as_current_span("agent.plan"),
        ):
            pass
        telemetry.record_session_outcome(
            "completed",
            1.25,
            {
                "toolCalls": 2,
                "promptTokens": 10,
                "completionTokens": 5,
            },
        )
    finally:
        telemetry.shutdown()
        server.shutdown()
        thread.join(timeout=5)

    assert _Collector.paths
    assert "/v1/traces" in _Collector.paths
    assert "/v1/metrics" in _Collector.paths
    assert "/v1/logs" in _Collector.paths
    spans = [
        span
        for request in _Collector.trace_requests
        for resource in request.resource_spans
        for scope in resource.scope_spans
        for span in scope.spans
    ]
    assert any(span.name == "agent.session" and span.trace_id.hex() == trace_id for span in spans)
    assert any(span.name == "agent.plan" and span.trace_id.hex() == trace_id for span in spans)
    service_names = [
        attribute.value.string_value
        for request in _Collector.trace_requests
        for resource in request.resource_spans
        for attribute in resource.resource.attributes
        if attribute.key == "service.name"
    ]
    assert "hpa-ai-worker" in service_names
    metric_names = [
        metric.name
        for request in _Collector.metric_requests
        for resource in request.resource_metrics
        for scope in resource.scope_metrics
        for metric in scope.metrics
    ]
    assert "hpa_ai_worker_sessions_total" in metric_names
    assert "hpa_ai_worker_session_seconds" in metric_names
    log_bodies = [
        record.body.string_value
        for request in _Collector.log_requests
        for resource in request.resource_logs
        for scope in resource.scope_logs
        for record in scope.log_records
        if record.body.string_value
    ]
    assert "agent session finished" in log_bodies
