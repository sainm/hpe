from __future__ import annotations

import asyncio
import json
import os
import re
import secrets
import sys
import time
from collections.abc import AsyncIterator
from contextlib import asynccontextmanager
from typing import Any

from langgraph.checkpoint.memory import InMemorySaver
from langgraph.checkpoint.postgres.aio import AsyncPostgresSaver
from langgraph.checkpoint.sqlite.aio import AsyncSqliteSaver
from psycopg import AsyncConnection, sql
from psycopg.rows import dict_row
from pydantic import TypeAdapter, ValidationError

from .contracts import (
    CancelSessionRequest,
    PlannerDecision,
    ResumeSessionRequest,
    StartSessionRequest,
    ToolResult,
    WorkerEvent,
    WorkerRequest,
)
from .graph import AgentGraph, AgentRunOutcome
from .planner import LiteLlmPlanner, Planner, ScriptedPlanner, deterministic_planner
from .telemetry import WorkerTelemetry, init_telemetry, session_span
from .tools import HttpToolExecutor, ScriptedToolExecutor, ToolExecutor

MAX_REQUEST_BYTES = 1024 * 1024
CHECKPOINT_SCHEMA = re.compile(r"^[a-z_][a-z0-9_]{0,62}$")


class ProtocolError(RuntimeError):
    pass


def _planner() -> Planner:
    if os.getenv("HPA_AI_PLANNER", "litellm") == "deterministic":
        output = json.loads(os.getenv("HPA_DETERMINISTIC_OUTPUT", '{"ok":true}'))
        if os.getenv("HPA_DETERMINISTIC_RESUME") == "1":
            return ScriptedPlanner([PlannerDecision.finish(output)])
        tool_name = os.getenv("HPA_DETERMINISTIC_TOOL", "workflow.execute")
        arguments = json.loads(os.getenv("HPA_DETERMINISTIC_ARGUMENTS", "{}"))
        return deterministic_planner(
            tool_name,
            arguments,
            output,
            approval_required=os.getenv("HPA_DETERMINISTIC_APPROVAL") == "1",
        )
    models = [model.strip() for model in os.getenv("HPA_AI_MODELS", "").split(",") if model.strip()]
    if not models:
        raise ProtocolError("HPA_AI_MODELS is required unless HPA_AI_PLANNER=deterministic")
    return LiteLlmPlanner(
        models,
        base_url=os.getenv("HPA_LITELLM_BASE_URL"),
    )


def _tools() -> ToolExecutor:
    if os.getenv("HPA_AI_PLANNER", "litellm") == "deterministic":
        raw_results = json.loads(os.getenv("HPA_DETERMINISTIC_TOOL_RESULTS", "{}"))
        results = {name: ToolResult.model_validate(result) for name, result in raw_results.items()}
        return ScriptedToolExecutor(
            results,
            delay_seconds=float(os.getenv("HPA_DETERMINISTIC_TOOL_DELAY_MS", "0")) / 1000,
        )
    endpoint = os.getenv("HPA_TOOL_ENDPOINT")
    if not endpoint:
        raise ProtocolError("HPA_TOOL_ENDPOINT is required for model-driven tool calls")
    return HttpToolExecutor(endpoint)


class Worker:
    def __init__(
        self,
        graph: AgentGraph,
        request_token: str,
        telemetry: WorkerTelemetry,
    ) -> None:
        self.graph = graph
        self.request_token = request_token
        self.telemetry = telemetry
        self.tasks: dict[str, asyncio.Task[None]] = {}
        self.emitted_sequences: dict[str, int] = {}
        self.output_lock = asyncio.Lock()

    async def run(self) -> None:
        while True:
            line = await asyncio.to_thread(sys.stdin.readline)
            if line == "":
                break
            if len(line.encode()) > MAX_REQUEST_BYTES:
                await self._emit_error(None, "REQUEST_TOO_LARGE", "request exceeded 1 MiB")
                continue
            try:
                request = WorkerRequestAdapter.validate_json(line)
                self._authenticate(request)
                if isinstance(request, CancelSessionRequest):
                    await self._cancel(request)
                else:
                    session_id = request.session_id
                    if session_id in self.tasks and not self.tasks[session_id].done():
                        raise ProtocolError(f"session {session_id} already has an active request")
                    self.tasks[session_id] = asyncio.create_task(
                        self._run_request(request),
                        name=f"hpa-ai:{session_id}",
                    )
            except (ValidationError, ProtocolError, json.JSONDecodeError) as error:
                request_id = None
                try:
                    raw = json.loads(line)
                    request_id = raw.get("sessionId")
                except json.JSONDecodeError:
                    pass
                await self._emit_error(request_id, "INVALID_REQUEST", str(error))
        if self.tasks:
            await asyncio.gather(*tuple(self.tasks.values()), return_exceptions=True)

    def _authenticate(self, request: WorkerRequest) -> None:
        provided = request.request_token.get_secret_value()
        if not secrets.compare_digest(provided, self.request_token):
            raise ProtocolError("invalid request token")

    async def _run_request(
        self,
        request: StartSessionRequest | ResumeSessionRequest,
    ) -> None:
        session_id = request.session_id
        started = time.monotonic()
        recorded = False
        try:
            parent = self.telemetry.extract(request.traceparent)
            attributes: dict[str, str | int] = {"session.id": session_id}
            if isinstance(request, StartSessionRequest):
                attributes["tenant.id"] = request.tenant_id
                attributes["agent.version.id"] = request.agent_version_id
            with session_span(
                self.telemetry,
                "agent.session",
                parent=parent,
                attributes=attributes,
            ) as span:
                trace_id = (
                    request.trace_id if isinstance(request, StartSessionRequest) else None
                ) or _span_trace_id(span)
                if isinstance(request, StartSessionRequest):
                    await self._emit(
                        WorkerEvent(
                            type="session.started",
                            sessionId=session_id,
                            traceId=trace_id,
                            data={"agentVersionId": request.agent_version_id},
                        )
                    )
                    outcome = await self.graph.start(request)
                else:
                    outcome = await self.graph.resume(
                        session_id,
                        request.approval,
                        request.tool_token.get_secret_value() if request.tool_token else None,
                        (
                            request.budget.model_dump(by_alias=True)
                            if request.budget is not None
                            else None
                        ),
                    )
                span.set_attribute("agent.status", outcome.status)
                self.telemetry.record_session_outcome(
                    outcome.status,
                    time.monotonic() - started,
                    outcome.usage,
                )
                recorded = True
                await self._emit_outcome(outcome)
        except asyncio.CancelledError:
            if not recorded:
                self.telemetry.record_session_outcome(
                    "cancelled",
                    time.monotonic() - started,
                    {},
                )
            await self._emit(
                WorkerEvent(
                    type="session.cancelled",
                    sessionId=session_id,
                    data={"reason": "cancelled by control plane"},
                )
            )
        except Exception as error:
            if not recorded:
                self.telemetry.record_session_outcome(
                    "failed",
                    time.monotonic() - started,
                    {},
                )
            await self._emit_error(
                session_id,
                "WORKER_ERROR",
                f"{type(error).__name__}: {error}",
            )
        finally:
            self.tasks.pop(session_id, None)

    async def _cancel(self, request: CancelSessionRequest) -> None:
        task = self.tasks.get(request.session_id)
        if task is not None and not task.done():
            task.cancel()
        await self._emit(
            WorkerEvent(
                type="session.cancel.requested",
                sessionId=request.session_id,
                data={"reason": request.reason},
            )
        )

    async def _emit_outcome(self, outcome: AgentRunOutcome) -> None:
        last = self.emitted_sequences.get(outcome.session_id, 0)
        for event in outcome.events:
            sequence = int(event.get("sequence", 0))
            if sequence <= last:
                continue
            last = max(last, sequence)
            await self._emit(
                WorkerEvent(
                    type=str(event["type"]),
                    sessionId=outcome.session_id,
                    data={
                        **dict(event.get("data", {})),
                        "sequence": sequence,
                    },
                )
            )
        self.emitted_sequences[outcome.session_id] = last
        await self._emit(
            WorkerEvent(
                type="usage",
                sessionId=outcome.session_id,
                data=outcome.usage,
            )
        )
        if outcome.status == "completed":
            await self._emit(
                WorkerEvent(
                    type="session.result",
                    sessionId=outcome.session_id,
                    data={"output": outcome.output or {}},
                )
            )
        elif outcome.status == "failed":
            await self._emit_error(
                outcome.session_id,
                "AGENT_FAILED",
                json.dumps(outcome.error or {}),
            )

    async def _emit_error(
        self,
        session_id: str | None,
        code: str,
        message: str,
    ) -> None:
        payload = {
            "type": "error",
            "code": code,
            "message": message[:4096],
        }
        if session_id:
            payload["sessionId"] = session_id
        async with self.output_lock:
            sys.stdout.write(json.dumps(payload, separators=(",", ":")) + "\n")
            sys.stdout.flush()

    async def _emit(self, event: WorkerEvent) -> None:
        async with self.output_lock:
            sys.stdout.write(event.model_dump_json(by_alias=True, exclude_none=True) + "\n")
            sys.stdout.flush()


WorkerRequestAdapter = TypeAdapter(WorkerRequest)


async def _run() -> None:
    request_token = os.getenv("HPA_AI_WORKER_TOKEN")
    if not request_token:
        raise ProtocolError("HPA_AI_WORKER_TOKEN is required")
    checkpoint_path = os.getenv("HPA_AI_CHECKPOINT_PATH")
    checkpoint_postgres_url = os.getenv("HPA_AI_CHECKPOINT_POSTGRES_URL")
    checkpoint_schema = os.getenv("HPA_AI_CHECKPOINT_SCHEMA")
    telemetry = init_telemetry()
    try:
        async with _checkpointer(
            checkpoint_path,
            checkpoint_postgres_url,
            checkpoint_schema,
        ) as saver:
            graph = AgentGraph(
                _planner(),
                _tools(),
                checkpointer=saver,
            )
            await Worker(graph, request_token, telemetry).run()
    finally:
        telemetry.shutdown()


@asynccontextmanager
async def _checkpointer(
    path: str | None,
    postgres_url: str | None = None,
    schema: str | None = None,
) -> AsyncIterator[InMemorySaver | AsyncSqliteSaver | AsyncPostgresSaver]:
    if postgres_url:
        if not schema:
            raise ProtocolError(
                "HPA_AI_CHECKPOINT_SCHEMA is required with HPA_AI_CHECKPOINT_POSTGRES_URL"
            )
        if not CHECKPOINT_SCHEMA.fullmatch(schema):
            raise ProtocolError("HPA_AI_CHECKPOINT_SCHEMA must match ^[a-z_][a-z0-9_]{0,62}$")
        async with await AsyncConnection.connect(
            postgres_url,
            autocommit=True,
            prepare_threshold=0,
            row_factory=dict_row,
        ) as connection:
            lock_key = f"hpa-ai-worker-checkpoint:{schema}"
            async with _postgres_advisory_lock(postgres_url, lock_key):
                schema_exists = await (
                    await connection.execute(
                        "SELECT EXISTS (SELECT 1 FROM pg_namespace WHERE nspname = %s)",
                        (schema,),
                    )
                ).fetchone()
                if schema_exists != {"exists": True}:
                    await connection.execute(
                        sql.SQL("CREATE SCHEMA {}").format(sql.Identifier(schema))
                    )
                await connection.execute(
                    "SELECT set_config('search_path', %s, false)",
                    (schema,),
                )
                postgres_saver = AsyncPostgresSaver(connection)
                await postgres_saver.setup()
            yield postgres_saver
        return
    if path:
        async with AsyncSqliteSaver.from_conn_string(path) as sqlite_saver:
            yield sqlite_saver
    else:
        yield InMemorySaver()


@asynccontextmanager
async def _postgres_advisory_lock(url: str, lock_key: str) -> AsyncIterator[None]:
    lock_connection: AsyncConnection[Any] | None = None
    for _ in range(600):
        connection = await AsyncConnection.connect(
            url,
            autocommit=True,
            prepare_threshold=0,
        )
        try:
            acquired = await (
                await connection.execute(
                    "SELECT pg_try_advisory_lock(hashtext(%s))",
                    (lock_key,),
                )
            ).fetchone()
        except Exception:
            await connection.close()
            raise
        if acquired == (True,):
            lock_connection = connection
            break
        await connection.close()
        await asyncio.sleep(0.1)
    if lock_connection is None:
        raise ProtocolError(f"timed out acquiring PostgreSQL checkpoint lock `{lock_key}`")
    try:
        yield
    finally:
        try:
            await lock_connection.execute(
                "SELECT pg_advisory_unlock(hashtext(%s))",
                (lock_key,),
            )
        finally:
            await lock_connection.close()


def main() -> None:
    try:
        asyncio.run(_run())
    except (KeyboardInterrupt, BrokenPipeError):
        pass
    except Exception as error:
        sys.stderr.write(f"hpa-ai-worker: {type(error).__name__}: {error}\n")
        raise SystemExit(1) from error


def _span_trace_id(span: object) -> str | None:
    get_span_context = getattr(span, "get_span_context", None)
    if get_span_context is None:
        return None
    context = get_span_context()
    return format(context.trace_id, "032x") if context.is_valid else None


if __name__ == "__main__":
    main()
