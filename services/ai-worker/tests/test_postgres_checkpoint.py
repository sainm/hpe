from __future__ import annotations

import asyncio
import os
import uuid

import pytest
from langgraph.checkpoint.postgres.aio import AsyncPostgresSaver
from psycopg import AsyncConnection, sql

from hpa_ai_worker.cli import ProtocolError, _checkpointer
from hpa_ai_worker.contracts import (
    Budget,
    PlannerDecision,
    StartSessionRequest,
    ToolCall,
    ToolResult,
)
from hpa_ai_worker.graph import AgentGraph
from hpa_ai_worker.planner import ScriptedPlanner
from hpa_ai_worker.tools import ScriptedToolExecutor

POSTGRES_URL = os.getenv("HPA_POSTGRES_TEST_URL")


def tool_specs() -> list[dict[str, object]]:
    return [
        {
            "type": "function",
            "function": {
                "name": "workflow.execute",
                "parameters": {"type": "object", "properties": {}},
                "x-hpa": {"risk": "external", "approvalRequired": True},
            },
        }
    ]


def request() -> StartSessionRequest:
    return StartSessionRequest(
        type="start",
        sessionId=f"session-{uuid.uuid4().hex}",
        requestToken="worker-token",
        tenantId="tenant-1",
        agentVersionId="agent-version-1",
        input={"message": "run"},
        budget=Budget(maxSteps=4, maxToolCalls=2, timeoutSeconds=30),
    )


async def drop_schema(url: str, schema: str) -> None:
    async with await AsyncConnection.connect(url, autocommit=True) as connection:
        await connection.execute(
            sql.SQL("DROP SCHEMA IF EXISTS {} CASCADE").format(sql.Identifier(schema))
        )


@pytest.mark.skipif(not POSTGRES_URL, reason="HPA_POSTGRES_TEST_URL is not set")
async def test_postgres_checkpoint_persists_approval_across_connections() -> None:
    assert POSTGRES_URL is not None
    schema = f"hpa_worker_test_{uuid.uuid4().hex[:20]}"
    start = request()
    executor = ScriptedToolExecutor(
        {
            "workflow.execute": ToolResult(
                callId="tc_1",
                status="succeeded",
                result={"executionId": "exec_1"},
            )
        }
    )
    try:
        async with _checkpointer(None, POSTGRES_URL, schema) as saver:
            assert isinstance(saver, AsyncPostgresSaver)
            graph = AgentGraph(
                ScriptedPlanner(
                    [
                        PlannerDecision(
                            action="tool",
                            toolCall=ToolCall(
                                callId="tc_1",
                                tool="workflow.execute",
                                arguments={},
                                risk="read",
                                approvalRequired=False,
                                idempotencyKey="session/step/call",
                            ),
                        )
                    ]
                ),
                executor,
                tool_specs=tool_specs(),
                checkpointer=saver,
            )
            paused = await graph.start(start)
            assert paused.status == "approval_required"

        async with _checkpointer(None, POSTGRES_URL, schema) as saver:
            graph = AgentGraph(
                ScriptedPlanner([PlannerDecision.finish({"ok": True})]),
                executor,
                tool_specs=tool_specs(),
                checkpointer=saver,
            )
            completed = await graph.resume(
                start.session_id,
                {"approved": True, "approver": "test"},
            )
            assert completed.status == "completed"
            assert completed.output == {"ok": True}
            assert len(executor.calls) == 1
            assert executor.calls[0].risk == "external"

        async with await AsyncConnection.connect(POSTGRES_URL, autocommit=True) as connection:
            checkpoint_table = await (
                await connection.execute(
                    "SELECT to_regclass(%s)",
                    (f"{schema}.checkpoints",),
                )
            ).fetchone()
            assert checkpoint_table == (f"{schema}.checkpoints",)
    finally:
        await drop_schema(POSTGRES_URL, schema)


@pytest.mark.skipif(not POSTGRES_URL, reason="HPA_POSTGRES_TEST_URL is not set")
async def test_postgres_checkpoint_initialization_is_concurrent_safe() -> None:
    assert POSTGRES_URL is not None
    schema = f"hpa_worker_concurrent_{uuid.uuid4().hex[:18]}"

    async def initialize() -> None:
        async with _checkpointer(None, POSTGRES_URL, schema):
            await asyncio.sleep(0.05)

    try:
        await asyncio.gather(initialize(), initialize())
        async with await AsyncConnection.connect(POSTGRES_URL, autocommit=True) as connection:
            exists = await (
                await connection.execute(
                    "SELECT to_regclass(%s)",
                    (f"{schema}.checkpoint_migrations",),
                )
            ).fetchone()
            assert exists == (f"{schema}.checkpoint_migrations",)
    finally:
        await drop_schema(POSTGRES_URL, schema)


async def test_postgres_checkpoint_requires_safe_schema() -> None:
    with pytest.raises(ProtocolError):
        async with _checkpointer(None, "postgresql://ignored", None):
            pass
    with pytest.raises(ProtocolError):
        async with _checkpointer(
            None,
            "postgresql://ignored",
            'unsafe"; DROP SCHEMA public CASCADE; --',
        ):
            pass
