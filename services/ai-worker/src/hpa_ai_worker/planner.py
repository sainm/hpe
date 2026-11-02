from __future__ import annotations

import json
from collections import deque
from typing import Any, Protocol

from .contracts import PlannerDecision, PlannerUsage, ToolCall


class PlannerError(RuntimeError):
    pass


class Planner(Protocol):
    async def decide(
        self,
        *,
        input_data: dict[str, Any],
        history: list[dict[str, Any]],
        tools: list[dict[str, Any]],
        model: str | None,
    ) -> PlannerDecision: ...


class ScriptedPlanner:
    def __init__(self, decisions: list[PlannerDecision]) -> None:
        self.decisions = deque(decisions)

    async def decide(
        self,
        *,
        input_data: dict[str, Any],
        history: list[dict[str, Any]],
        tools: list[dict[str, Any]],
        model: str | None,
    ) -> PlannerDecision:
        del input_data, history, tools, model
        if not self.decisions:
            raise PlannerError("scripted planner exhausted")
        return self.decisions.popleft()


class LiteLlmPlanner:
    """Provider-neutral planner using LiteLLM's OpenAI-compatible router."""

    def __init__(
        self,
        models: list[str],
        *,
        base_url: str | None = None,
        api_key_env: str = "HPA_MODEL_API_KEY",
        timeout_seconds: float = 30.0,
    ) -> None:
        if not models:
            raise ValueError("at least one model is required")
        self.models = models
        self.base_url = base_url
        self.api_key_env = api_key_env
        self.timeout_seconds = timeout_seconds

    async def decide(
        self,
        *,
        input_data: dict[str, Any],
        history: list[dict[str, Any]],
        tools: list[dict[str, Any]],
        model: str | None,
    ) -> PlannerDecision:
        import os

        import litellm

        messages = [
            {
                "role": "system",
                "content": (
                    "You are an HPA Agent planner. Return one JSON object with either "
                    '{"action":"tool","toolCall":{...}} or '
                    '{"action":"finish","output":{...}}. '
                    "Never claim a tool ran unless a tool result is present."
                ),
            },
            {
                "role": "user",
                "content": json.dumps(
                    {"input": input_data, "history": history, "tools": tools},
                    separators=(",", ":"),
                ),
            },
        ]
        last_error: Exception | None = None
        models = [model] if model is not None else self.models
        for candidate in models:
            try:
                response = await litellm.acompletion(
                    model=candidate,
                    messages=messages,
                    api_base=self.base_url,
                    api_key=os.getenv(self.api_key_env),
                    timeout=self.timeout_seconds,
                    response_format={"type": "json_object"},
                )
                content = response.choices[0].message.content
                if not isinstance(content, str):
                    raise PlannerError("model returned non-text content")
                decision = PlannerDecision.model_validate_json(content)
                usage = getattr(response, "usage", None)
                prompt_tokens = int(getattr(usage, "prompt_tokens", 0) or 0)
                completion_tokens = int(getattr(usage, "completion_tokens", 0) or 0)
                try:
                    cost = float(litellm.completion_cost(completion_response=response) or 0)
                except Exception:
                    cost = 0.0
                return decision.model_copy(
                    update={
                        "usage": PlannerUsage(
                            promptTokens=prompt_tokens,
                            completionTokens=completion_tokens,
                            costUsd=cost,
                        )
                    }
                )
            except Exception as error:
                last_error = error
        raise PlannerError(f"all model fallbacks failed: {type(last_error).__name__}: {last_error}")


def deterministic_planner(
    tool_name: str,
    arguments: dict[str, Any],
    output: dict[str, Any],
    *,
    approval_required: bool = False,
) -> ScriptedPlanner:
    return ScriptedPlanner(
        [
            PlannerDecision(
                action="tool",
                toolCall=ToolCall(
                    callId="tc_1",
                    tool=tool_name,
                    arguments=arguments,
                    idempotencyKey="agent/session/step-1/tc_1",
                    approvalRequired=approval_required,
                    risk="write" if approval_required else "read",
                ),
            ),
            PlannerDecision.finish(output),
        ]
    )
