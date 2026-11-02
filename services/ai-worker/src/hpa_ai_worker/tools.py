from __future__ import annotations

import asyncio
import json
from typing import Protocol
from urllib.error import HTTPError, URLError
from urllib.parse import urlparse
from urllib.request import (
    HTTPRedirectHandler,
    Request,
    build_opener,
)

from opentelemetry.propagate import inject
from pydantic import ValidationError

from .contracts import ToolCall, ToolResult

MAX_RESPONSE_BYTES = 2 * 1024 * 1024


class ToolExecutionError(RuntimeError):
    pass


class ToolExecutor(Protocol):
    async def invoke(
        self,
        call: ToolCall,
        tool_token: str | None,
        session_id: str,
        tenant_id: str,
    ) -> ToolResult: ...


class _RejectRedirects(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):  # type: ignore[no-untyped-def]
        raise ToolExecutionError("tool endpoint redirects are not allowed")


class HttpToolExecutor:
    """HTTP JSON adapter to a Rust-owned ToolService endpoint."""

    def __init__(self, endpoint: str, timeout_seconds: float = 30.0) -> None:
        parsed = urlparse(endpoint)
        if parsed.scheme not in {"http", "https"} or not parsed.hostname:
            raise ValueError("tool endpoint must be an absolute HTTP(S) URL")
        if parsed.scheme != "https" and parsed.hostname not in {"127.0.0.1", "::1", "localhost"}:
            raise ValueError("non-loopback tool endpoints must use HTTPS")
        self.endpoint = endpoint
        self.timeout_seconds = timeout_seconds

    async def invoke(
        self,
        call: ToolCall,
        tool_token: str | None,
        session_id: str,
        tenant_id: str,
    ) -> ToolResult:
        return await asyncio.to_thread(self._invoke, call, tool_token, session_id, tenant_id)

    def _invoke(
        self,
        call: ToolCall,
        tool_token: str | None,
        session_id: str,
        tenant_id: str,
    ) -> ToolResult:
        body = json.dumps(
            {
                "sessionId": session_id,
                "tenantId": tenant_id,
                "callId": call.call_id,
                "tool": call.tool,
                "arguments": call.arguments,
                "idempotencyKey": call.idempotency_key,
                "risk": call.risk,
            },
            separators=(",", ":"),
        ).encode()
        headers = {"content-type": "application/json", "accept": "application/json"}
        headers["x-hpa-tenant-id"] = tenant_id
        inject(headers)
        if tool_token:
            headers["authorization"] = f"Bearer {tool_token}"
        request = Request(  # noqa: S310 - endpoint scheme was validated above.
            self.endpoint,
            data=body,
            headers=headers,
            method="POST",
        )
        opener = build_opener(_RejectRedirects())
        try:
            with opener.open(request, timeout=self.timeout_seconds) as response:
                payload = response.read(MAX_RESPONSE_BYTES + 1)
                if len(payload) > MAX_RESPONSE_BYTES:
                    return ToolResult(
                        callId=call.call_id,
                        status="failed",
                        error="tool response exceeded 2 MiB",
                    )
                decoded = json.loads(payload.decode())
                result = ToolResult.model_validate(decoded)
                if result.call_id != call.call_id:
                    return ToolResult(
                        callId=call.call_id,
                        status="failed",
                        error="tool response callId does not match the request",
                    )
                return result
        except ValidationError:
            return ToolResult(
                callId=call.call_id,
                status="failed",
                error="tool endpoint returned an invalid ToolResult",
            )
        except HTTPError as error:
            detail = error.read(4096).decode(errors="replace")
            return ToolResult(
                callId=call.call_id,
                status="denied" if error.code in {401, 403} else "failed",
                error=f"tool endpoint returned HTTP {error.code}: {detail}",
                retryable=error.code >= 500 or error.code == 429,
            )
        except TimeoutError:
            return ToolResult(
                callId=call.call_id,
                status="timeout",
                error="tool endpoint timed out",
                retryable=True,
            )
        except (URLError, ToolExecutionError, json.JSONDecodeError) as error:
            return ToolResult(
                callId=call.call_id,
                status="failed",
                error=str(error),
                retryable=isinstance(error, URLError),
            )


class ScriptedToolExecutor:
    def __init__(
        self,
        results: dict[str, ToolResult],
        delay_seconds: float = 0.0,
    ) -> None:
        self.results = results
        self.delay_seconds = delay_seconds
        self.calls: list[ToolCall] = []

    async def invoke(
        self,
        call: ToolCall,
        tool_token: str | None,
        session_id: str,
        tenant_id: str,
    ) -> ToolResult:
        del tool_token, session_id, tenant_id
        if self.delay_seconds:
            await asyncio.sleep(self.delay_seconds)
        self.calls.append(call)
        return self.results.get(
            call.tool,
            ToolResult(
                callId=call.call_id,
                status="failed",
                error=f"no scripted result for {call.tool}",
            ),
        )
