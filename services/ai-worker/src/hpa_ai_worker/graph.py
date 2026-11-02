from __future__ import annotations

import hashlib
import json
import time
from collections.abc import Mapping
from typing import Any, Literal, TypedDict, cast

from langchain_core.runnables import RunnableConfig
from langgraph.checkpoint.base import BaseCheckpointSaver
from langgraph.graph import END, START, StateGraph
from langgraph.types import Command, interrupt
from opentelemetry import trace
from pydantic import BaseModel, ConfigDict, Field

from .budget import BudgetExceeded
from .contracts import StartSessionRequest, ToolResult
from .planner import Planner, PlannerError
from .tools import ToolExecutor


class AgentGraphState(TypedDict, total=False):
    session_id: str
    tenant_id: str
    input: dict[str, Any]
    instructions: str | None
    tools: list[dict[str, Any]]
    model: str | None
    history: list[dict[str, Any]]
    events: list[dict[str, Any]]
    budget: dict[str, Any]
    usage: dict[str, int | float]
    deadline_epoch: float
    pending_tool: dict[str, Any] | None
    final_output: dict[str, Any] | None
    error: dict[str, Any] | None
    last_call_hash: str
    repeated_calls: int


class AgentRunOutcome(BaseModel):
    model_config = ConfigDict(extra="forbid", populate_by_name=True)
    session_id: str
    status: Literal["completed", "approval_required", "failed"]
    output: dict[str, Any] | None = None
    error: dict[str, Any] | None = None
    usage: dict[str, int | float] = Field(default_factory=dict)
    events: list[dict[str, Any]] = Field(default_factory=list)


class AgentGraph:
    def __init__(
        self,
        planner: Planner,
        tools: ToolExecutor,
        *,
        tool_specs: list[dict[str, Any]] | None = None,
        checkpointer: BaseCheckpointSaver[Any] | None = None,
    ) -> None:
        self.planner = planner
        self.tools = tools
        self.tool_specs = tool_specs or []
        self._tool_tokens: dict[str, str] = {}
        builder = StateGraph(AgentGraphState)
        builder.add_node("plan", self._plan)
        builder.add_node("tool", self._tool)
        builder.add_edge(START, "plan")
        builder.add_conditional_edges(
            "plan",
            self._route_after_plan,
            {"tool": "tool", "finish": END},
        )
        builder.add_conditional_edges(
            "tool",
            self._route_after_tool,
            {"plan": "plan", "finish": END},
        )
        self.graph = builder.compile(checkpointer=checkpointer)

    async def start(self, request: StartSessionRequest) -> AgentRunOutcome:
        if request.tool_token is not None:
            self._tool_tokens[request.session_id] = request.tool_token.get_secret_value()
        budget = request.budget.model_dump(by_alias=True)
        initial: AgentGraphState = {
            "session_id": request.session_id,
            "tenant_id": request.tenant_id,
            "input": request.input,
            "instructions": request.instructions,
            "tools": request.tools or self.tool_specs,
            "model": request.model,
            "history": [],
            "events": [],
            "budget": budget,
            "usage": {
                "steps": 0,
                "toolCalls": 0,
                "promptTokens": 0,
                "completionTokens": 0,
                "costUsd": 0.0,
            },
            "deadline_epoch": time.time() + request.budget.timeout_seconds,
            "pending_tool": None,
            "final_output": None,
            "error": None,
            "last_call_hash": "",
            "repeated_calls": 0,
        }
        result = cast(
            dict[str, Any],
            await self.graph.ainvoke(initial, self._config(request.session_id)),
        )
        outcome = self._outcome(request.session_id, result)
        if outcome.status != "approval_required":
            self._tool_tokens.pop(request.session_id, None)
        return outcome

    async def resume(
        self,
        session_id: str,
        approval: dict[str, Any],
        tool_token: str | None = None,
        budget: dict[str, Any] | None = None,
    ) -> AgentRunOutcome:
        if tool_token:
            self._tool_tokens[session_id] = tool_token
        if budget is not None:
            await self.graph.aupdate_state(
                self._config(session_id),
                {
                    "budget": budget,
                    "deadline_epoch": time.time() + float(budget["timeoutSeconds"]),
                },
            )
        command: Command[Any] = Command(resume=approval)
        result = cast(
            dict[str, Any],
            await self.graph.ainvoke(command, self._config(session_id)),
        )
        outcome = self._outcome(session_id, result)
        if outcome.status != "approval_required":
            self._tool_tokens.pop(session_id, None)
        return outcome

    async def _plan(self, state: AgentGraphState) -> AgentGraphState:
        try:
            self._check_deadline(state)
            usage = dict(state["usage"])
            budget = state["budget"]
            if int(usage["steps"]) >= int(budget["maxSteps"]):
                raise BudgetExceeded("maxSteps")
            with trace.get_tracer("hpa-ai-worker").start_as_current_span(
                "agent.plan",
                attributes={"agent.step": int(usage["steps"]) + 1},
            ) as planner_span:
                decision = await self.planner.decide(
                    input_data={
                        "input": state["input"],
                        "instructions": state.get("instructions"),
                    },
                    history=state.get("history", []),
                    tools=state.get("tools", []),
                    model=state.get("model"),
                )
                planner_span.set_attribute("agent.action", decision.action)
            response_usage = decision.usage
            usage["steps"] = int(usage["steps"]) + 1
            usage["promptTokens"] = int(usage["promptTokens"]) + response_usage.prompt_tokens
            usage["completionTokens"] = (
                int(usage["completionTokens"]) + response_usage.completion_tokens
            )
            usage["costUsd"] = float(usage["costUsd"]) + response_usage.cost_usd
            if int(usage["promptTokens"]) + int(usage["completionTokens"]) > int(
                budget["maxTokens"]
            ):
                raise BudgetExceeded("maxTokens")
            if float(usage["costUsd"]) > float(budget["maxCostUsd"]):
                raise BudgetExceeded("maxCostUsd")
            events = self._event(
                state,
                "step",
                {
                    "step": usage["steps"],
                    "action": decision.action,
                    "usage": response_usage.model_dump(by_alias=True),
                },
            )
            history = [
                *state.get("history", []),
                {
                    "kind": "decision",
                    "decision": decision.model_dump(by_alias=True, exclude_none=True),
                },
            ]
            if decision.action == "finish":
                events = self._event(
                    {"events": events},
                    "session.completed",
                    {"output": decision.output or {}},
                )
                return {
                    "usage": usage,
                    "events": events,
                    "history": history,
                    "pending_tool": None,
                    "final_output": decision.output or {},
                }
            if decision.tool_call is None:
                raise PlannerError("tool decision did not include toolCall")
            model_tool_call = decision.tool_call
            tool_spec = next(
                (
                    spec
                    for spec in state.get("tools", [])
                    if spec.get("function", {}).get("name") == model_tool_call.tool
                ),
                None,
            )
            if tool_spec is None:
                raise PlannerError(f"planner selected undeclared tool {model_tool_call.tool}")
            function = tool_spec.get("function")
            tool_meta = function.get("x-hpa") if isinstance(function, dict) else None
            if not isinstance(tool_meta, dict):
                raise PlannerError(f"tool {model_tool_call.tool} has no trusted x-hpa metadata")
            risk = tool_meta.get("risk")
            approval_required = tool_meta.get("approvalRequired")
            if not isinstance(risk, str) or risk not in {"read", "write", "external"}:
                raise PlannerError(f"tool {model_tool_call.tool} has invalid risk metadata")
            if not isinstance(approval_required, bool):
                raise PlannerError(f"tool {model_tool_call.tool} has invalid approval metadata")
            decision = decision.model_copy(
                update={
                    "tool_call": model_tool_call.model_copy(
                        update={
                            "risk": risk,
                            "approval_required": approval_required,
                        }
                    )
                }
            )
            trusted_tool_call = decision.tool_call
            if trusted_tool_call is None:
                raise PlannerError("trusted tool decision lost its toolCall")
            call_hash = hashlib.sha256(
                json.dumps(
                    trusted_tool_call.model_dump(by_alias=True),
                    sort_keys=True,
                    separators=(",", ":"),
                ).encode()
            ).hexdigest()
            repeats = (
                state.get("repeated_calls", 0) + 1
                if call_hash == state.get("last_call_hash")
                else 1
            )
            if repeats >= 3:
                raise PlannerError("repeated tool plan detected")
            events = self._event(
                {"events": events},
                "tool.call",
                trusted_tool_call.model_dump(by_alias=True),
            )
            return {
                "usage": usage,
                "events": events,
                "history": history,
                "pending_tool": trusted_tool_call.model_dump(by_alias=True),
                "last_call_hash": call_hash,
                "repeated_calls": repeats,
            }
        except (BudgetExceeded, PlannerError) as error:
            return self._failed(state, type(error).__name__, str(error))

    async def _tool(self, state: AgentGraphState) -> AgentGraphState:
        pending = state.get("pending_tool")
        if not pending:
            return self._failed(state, "InvalidState", "tool node received no pending tool")
        from .contracts import ToolCall

        call = ToolCall.model_validate(pending)
        tool_span = trace.get_tracer("hpa-ai-worker").start_span(
            "agent.tool_call",
            attributes={
                "agent.tool.name": call.tool,
                "agent.tool.call_id": call.call_id,
                "agent.tool.risk": call.risk,
            },
        )
        with trace.use_span(tool_span, end_on_exit=True):
            if call.approval_required:
                approval = interrupt(
                    {
                        "kind": "approval.required",
                        "callId": call.call_id,
                        "tool": call.tool,
                        "arguments": call.arguments,
                        "risk": call.risk,
                    }
                )
                if not isinstance(approval, dict) or approval.get("approved") is not True:
                    result = ToolResult(
                        callId=call.call_id,
                        status="denied",
                        error="approval denied",
                    )
                    tool_span.set_attribute("agent.tool.status", result.status)
                    return self._record_tool_result(state, call, result)
            try:
                self._check_deadline(state)
                budget = state["budget"]
                usage = dict(state["usage"])
                if int(usage["toolCalls"]) >= int(budget["maxToolCalls"]):
                    raise BudgetExceeded("maxToolCalls")
                usage["toolCalls"] = int(usage["toolCalls"]) + 1
                result = await self.tools.invoke(
                    call,
                    self._tool_tokens.get(state["session_id"]),
                    state["session_id"],
                    state["tenant_id"],
                )
                tool_span.set_attribute("agent.tool.status", result.status)
                next_state: AgentGraphState = state.copy()
                next_state["usage"] = usage
                return self._record_tool_result(next_state, call, result)
            except BudgetExceeded as error:
                tool_span.set_attribute("agent.tool.status", "budget_exceeded")
                return self._failed(state, type(error).__name__, str(error))

    def _record_tool_result(
        self,
        state: Mapping[str, Any],
        call: Any,
        result: ToolResult,
    ) -> AgentGraphState:
        events = self._event(
            state,
            "tool.result",
            result.model_dump(by_alias=True, exclude_none=True),
        )
        history = [
            *state.get("history", []),
            {
                "kind": "toolResult",
                "callId": call.call_id,
                "tool": call.tool,
                "result": result.model_dump(by_alias=True, exclude_none=True),
            },
        ]
        return {
            "events": events,
            "history": history,
            "pending_tool": None,
            "usage": state.get("usage", {}),
        }

    def _failed(
        self,
        state: AgentGraphState,
        code: str,
        message: str,
    ) -> AgentGraphState:
        error = {"code": code, "message": message}
        events = self._event(state, "error", error)
        return {"error": error, "events": events, "pending_tool": None}

    @staticmethod
    def _route_after_plan(state: AgentGraphState) -> str:
        return "finish" if state.get("final_output") is not None or state.get("error") else "tool"

    @staticmethod
    def _route_after_tool(state: AgentGraphState) -> str:
        return "finish" if state.get("error") else "plan"

    @staticmethod
    def _event(
        state: Mapping[str, Any],
        event_type: str,
        data: dict[str, Any],
    ) -> list[dict[str, Any]]:
        events = list(state.get("events", []))
        events.append(
            {
                "sequence": len(events) + 1,
                "type": event_type,
                "data": data,
            }
        )
        return events

    @staticmethod
    def _check_deadline(state: AgentGraphState) -> None:
        if time.time() >= float(state["deadline_epoch"]):
            raise BudgetExceeded("timeout")

    @staticmethod
    def _config(session_id: str) -> RunnableConfig:
        return {"configurable": {"thread_id": session_id}}

    @staticmethod
    def _outcome(session_id: str, result: dict[str, Any]) -> AgentRunOutcome:
        interrupts = result.get("__interrupt__")
        if interrupts:
            first = interrupts[0]
            value = getattr(first, "value", first)
            return AgentRunOutcome(
                session_id=session_id,
                status="approval_required",
                usage=result.get("usage", {}),
                events=[
                    *result.get("events", []),
                    {
                        "sequence": len(result.get("events", [])) + 1,
                        "type": "approval.required",
                        "data": value if isinstance(value, dict) else {"detail": str(value)},
                    },
                ],
            )
        if result.get("error"):
            return AgentRunOutcome(
                session_id=session_id,
                status="failed",
                error=result["error"],
                usage=result.get("usage", {}),
                events=result.get("events", []),
            )
        return AgentRunOutcome(
            session_id=session_id,
            status="completed",
            output=result.get("final_output") or {},
            usage=result.get("usage", {}),
            events=result.get("events", []),
        )
