from __future__ import annotations

from langgraph.checkpoint.memory import InMemorySaver
from pydantic import SecretStr

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


def tool_specs(*, approval: bool = False, risk: str = "read") -> list[dict[str, object]]:
    return [
        {
            "type": "function",
            "function": {
                "name": "workflow.execute",
                "description": "Agent tool `workflow.execute`",
                "parameters": {
                    "type": "object",
                    "properties": {"workflowId": {"type": "string"}},
                    "required": ["workflowId"],
                },
                "x-hpa": {
                    "risk": risk,
                    "approvalRequired": approval,
                    "idempotencyKeyTemplate": "{sessionId}/{step}/{callId}",
                },
            },
        }
    ]


def request(*, approval: bool = False, max_tool_calls: int = 2) -> StartSessionRequest:
    del approval
    return StartSessionRequest(
        type="start",
        sessionId="session-1",
        requestToken=SecretStr("worker-token"),
        tenantId="tenant-1",
        agentVersionId="agent-version-1",
        input={"message": "run"},
        budget=Budget(maxSteps=4, maxToolCalls=max_tool_calls, timeoutSeconds=30),
    )


def planner(*, approval: bool = False, finish_first: bool = False) -> ScriptedPlanner:
    if finish_first:
        return ScriptedPlanner([PlannerDecision.finish({"ok": True})])
    return ScriptedPlanner(
        [
            PlannerDecision(
                action="tool",
                toolCall=ToolCall(
                    callId="tc_1",
                    tool="workflow.execute",
                    arguments={"workflowId": "wf_1"},
                    idempotencyKey="session/step-1/tc_1",
                    approvalRequired=approval,
                    risk="write" if approval else "read",
                ),
            ),
            PlannerDecision.finish({"ok": True}),
        ]
    )


def tools() -> ScriptedToolExecutor:
    return ScriptedToolExecutor(
        {
            "workflow.execute": ToolResult(
                callId="tc_1",
                status="succeeded",
                result={"executionId": "exec_1"},
            )
        }
    )


async def test_tool_round_trip_completes() -> None:
    executor = tools()
    graph = AgentGraph(
        planner(),
        executor,
        tool_specs=tool_specs(),
        checkpointer=InMemorySaver(),
    )
    outcome = await graph.start(request())
    assert outcome.status == "completed"
    assert outcome.output == {"ok": True}
    assert outcome.usage["steps"] == 2
    assert outcome.usage["toolCalls"] == 1
    assert [event["type"] for event in outcome.events] == [
        "step",
        "tool.call",
        "tool.result",
        "step",
        "session.completed",
    ]
    assert executor.calls[0].approval_required is False


async def test_approval_interrupts_and_resumes() -> None:
    executor = tools()
    graph = AgentGraph(
        planner(approval=True),
        executor,
        tool_specs=tool_specs(approval=True, risk="write"),
        checkpointer=InMemorySaver(),
    )
    paused = await graph.start(request(approval=True))
    assert paused.status == "approval_required"
    assert paused.events[-1]["type"] == "approval.required"
    assert executor.calls == []

    completed = await graph.resume("session-1", {"approved": True, "approver": "human-1"})
    assert completed.status == "completed"
    assert len(executor.calls) == 1


async def test_tool_budget_fails_closed() -> None:
    executor = tools()
    graph = AgentGraph(
        planner(),
        executor,
        tool_specs=tool_specs(),
        checkpointer=InMemorySaver(),
    )
    outcome = await graph.start(request(max_tool_calls=0))
    assert outcome.status == "failed"
    assert outcome.error == {"code": "BudgetExceeded", "message": "maxToolCalls"}
    assert executor.calls == []


async def test_planner_cannot_select_undeclared_tool() -> None:
    executor = tools()
    graph = AgentGraph(
        planner(),
        executor,
        tool_specs=[
            {
                "type": "function",
                "function": {
                    "name": "workflow.list",
                    "description": "Different tool",
                    "parameters": {"type": "object", "properties": {}},
                    "x-hpa": {
                        "risk": "read",
                        "approvalRequired": False,
                    },
                },
            }
        ],
        checkpointer=InMemorySaver(),
    )
    outcome = await graph.start(request())
    assert outcome.status == "failed"
    assert outcome.error == {
        "code": "PlannerError",
        "message": "planner selected undeclared tool workflow.execute",
    }
    assert executor.calls == []


async def test_rust_tool_metadata_overrides_model_risk_and_approval() -> None:
    executor = tools()
    graph = AgentGraph(
        planner(approval=False),
        executor,
        tool_specs=tool_specs(approval=True, risk="external"),
        checkpointer=InMemorySaver(),
    )
    paused = await graph.start(request())
    assert paused.status == "approval_required"
    assert paused.events[-1]["data"]["risk"] == "external"
    assert paused.events[-1]["data"]["callId"] == "tc_1"
    assert executor.calls == []


async def test_tool_without_trusted_metadata_fails_closed() -> None:
    executor = tools()
    specs = tool_specs()
    function = specs[0]["function"]
    assert isinstance(function, dict)
    function.pop("x-hpa")
    graph = AgentGraph(
        planner(),
        executor,
        tool_specs=specs,
        checkpointer=InMemorySaver(),
    )
    outcome = await graph.start(request())
    assert outcome.status == "failed"
    assert outcome.error == {
        "code": "PlannerError",
        "message": "tool workflow.execute has no trusted x-hpa metadata",
    }
    assert executor.calls == []
