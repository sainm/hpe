from __future__ import annotations

from dataclasses import dataclass, field
from time import monotonic

from .contracts import Budget, PlannerUsage, ToolCall


class BudgetExceeded(RuntimeError):
    """Raised when a Rust-authoritative budget mirror is exhausted."""


@dataclass
class BudgetTracker:
    budget: Budget
    started: float = field(default_factory=monotonic)
    steps: int = 0
    tool_calls: int = 0
    prompt_tokens: int = 0
    completion_tokens: int = 0
    cost_usd: float = 0.0

    def check_deadline(self) -> None:
        if monotonic() - self.started >= self.budget.timeout_seconds:
            raise BudgetExceeded("timeout")

    def consume_step(self, usage: PlannerUsage | None = None) -> None:
        self.check_deadline()
        if self.steps >= self.budget.max_steps:
            raise BudgetExceeded("maxSteps")
        self.steps += 1
        if usage is not None:
            self.prompt_tokens += usage.prompt_tokens
            self.completion_tokens += usage.completion_tokens
            self.cost_usd += usage.cost_usd
        if self.prompt_tokens + self.completion_tokens > self.budget.max_tokens:
            raise BudgetExceeded("maxTokens")
        if self.cost_usd > self.budget.max_cost_usd:
            raise BudgetExceeded("maxCostUsd")

    def authorize_tool(self, call: ToolCall) -> None:
        self.check_deadline()
        if self.tool_calls >= self.budget.max_tool_calls:
            raise BudgetExceeded("maxToolCalls")
        if not call.idempotency_key:
            raise BudgetExceeded("missingIdempotencyKey")
        self.tool_calls += 1

    def usage(self) -> dict[str, int | float]:
        return {
            "steps": self.steps,
            "toolCalls": self.tool_calls,
            "promptTokens": self.prompt_tokens,
            "completionTokens": self.completion_tokens,
            "costUsd": round(self.cost_usd, 8),
        }
