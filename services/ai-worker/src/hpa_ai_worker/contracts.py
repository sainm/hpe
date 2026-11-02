from __future__ import annotations

from typing import Annotated, Any, Literal

from pydantic import BaseModel, ConfigDict, Field, SecretStr, StringConstraints

Identifier = Annotated[
    str,
    StringConstraints(min_length=1, max_length=128, pattern=r"^[A-Za-z0-9_.:-]+$"),
]


class StrictModel(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, populate_by_name=True)


class Budget(StrictModel):
    max_steps: int = Field(default=8, alias="maxSteps", ge=1, le=64)
    max_tool_calls: int = Field(default=4, alias="maxToolCalls", ge=0, le=32)
    timeout_seconds: int = Field(default=60, alias="timeoutSeconds", ge=1, le=3600)
    max_tokens: int = Field(default=32_000, alias="maxTokens", ge=1, le=1_000_000)
    max_cost_usd: float = Field(default=1.0, alias="maxCostUsd", ge=0, le=1_000)


class StartSessionRequest(StrictModel):
    type: Literal["start"]
    session_id: Identifier = Field(alias="sessionId")
    request_token: SecretStr = Field(alias="requestToken")
    tenant_id: Identifier = Field(alias="tenantId")
    agent_version_id: Identifier = Field(alias="agentVersionId")
    input: dict[str, Any]
    instructions: str | None = Field(default=None, max_length=32_000)
    tools: list[dict[str, Any]] = Field(default_factory=list, max_length=128)
    budget: Budget = Field(default_factory=Budget)
    tool_token: SecretStr | None = Field(default=None, alias="toolToken")
    model: str | None = Field(default=None, max_length=256)
    trace_id: str | None = Field(default=None, alias="traceId", max_length=128)
    traceparent: str | None = Field(default=None, max_length=128)


class ResumeSessionRequest(StrictModel):
    type: Literal["resume"]
    session_id: Identifier = Field(alias="sessionId")
    request_token: SecretStr = Field(alias="requestToken")
    approval: dict[str, Any]
    budget: Budget | None = None
    tool_token: SecretStr | None = Field(default=None, alias="toolToken")
    traceparent: str | None = Field(default=None, max_length=128)


class CancelSessionRequest(StrictModel):
    type: Literal["cancel"]
    session_id: Identifier = Field(alias="sessionId")
    request_token: SecretStr = Field(alias="requestToken")
    reason: str = Field(default="cancelled", max_length=512)


WorkerRequest = StartSessionRequest | ResumeSessionRequest | CancelSessionRequest


class ToolCall(StrictModel):
    call_id: Identifier = Field(alias="callId")
    tool: Identifier
    arguments: dict[str, Any]
    risk: Literal["read", "write", "external"] = "read"
    approval_required: bool = Field(default=False, alias="approvalRequired")
    idempotency_key: str = Field(alias="idempotencyKey", min_length=1, max_length=256)


class ToolResult(StrictModel):
    call_id: Identifier = Field(alias="callId")
    status: Literal["succeeded", "failed", "denied", "timeout"]
    result: Any = None
    error: str | None = None
    retryable: bool = False


class PlannerUsage(StrictModel):
    prompt_tokens: int = Field(default=0, alias="promptTokens", ge=0)
    completion_tokens: int = Field(default=0, alias="completionTokens", ge=0)
    cost_usd: float = Field(default=0.0, alias="costUsd", ge=0)


class PlannerDecision(StrictModel):
    action: Literal["tool", "finish"]
    tool_call: ToolCall | None = Field(default=None, alias="toolCall")
    output: dict[str, Any] | None = None
    usage: PlannerUsage = Field(default_factory=PlannerUsage)

    @classmethod
    def finish(cls, output: dict[str, Any], usage: PlannerUsage | None = None) -> PlannerDecision:
        return cls(action="finish", output=output, usage=usage or PlannerUsage())


class WorkerEvent(StrictModel):
    type: str
    session_id: Identifier = Field(alias="sessionId")
    trace_id: str | None = Field(default=None, alias="traceId")
    data: dict[str, Any] = Field(default_factory=dict)
