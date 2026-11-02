from __future__ import annotations

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any

from hpa_ai_worker.contracts import ToolCall
from hpa_ai_worker.tools import HttpToolExecutor


class ToolHandler(BaseHTTPRequestHandler):
    request_body: dict[str, Any] = {}
    request_headers: dict[str, str] = {}

    def do_POST(self) -> None:  # noqa: N802
        length = int(self.headers.get("content-length", "0"))
        type(self).request_body = json.loads(self.rfile.read(length))
        type(self).request_headers = {key.lower(): value for key, value in self.headers.items()}
        body = json.dumps(
            {
                "callId": self.request_body["callId"],
                "status": "succeeded",
                "result": {"executionId": "exec_1"},
            }
        ).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args: object) -> None:
        del format, args


async def test_http_tool_executor_binds_session_tenant_and_token() -> None:
    server = ThreadingHTTPServer(("127.0.0.1", 0), ToolHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        executor = HttpToolExecutor(
            f"http://127.0.0.1:{server.server_address[1]}/internal/agent-tool-calls"
        )
        call = ToolCall(
            callId="tc_1",
            tool="workflow.execute",
            arguments={"workflowId": "wf_1"},
            risk="external",
            approvalRequired=True,
            idempotencyKey="session-1/tc_1",
        )
        result = await executor.invoke(call, "tool-token", "session-1", "tenant-1")
        assert result.status == "succeeded"
        assert result.result == {"executionId": "exec_1"}
        assert ToolHandler.request_body == {
            "sessionId": "session-1",
            "tenantId": "tenant-1",
            "callId": "tc_1",
            "tool": "workflow.execute",
            "arguments": {"workflowId": "wf_1"},
            "idempotencyKey": "session-1/tc_1",
            "risk": "external",
        }
        assert ToolHandler.request_headers["authorization"] == "Bearer tool-token"
        assert ToolHandler.request_headers["x-hpa-tenant-id"] == "tenant-1"
    finally:
        server.shutdown()
        server.server_close()
