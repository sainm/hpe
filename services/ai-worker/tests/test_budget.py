import pytest

from hpa_ai_worker.budget import BudgetExceeded, BudgetTracker
from hpa_ai_worker.contracts import Budget, PlannerUsage, ToolCall


def tool_call() -> ToolCall:
    return ToolCall(
        callId="tc_1",
        tool="workflow.execute",
        arguments={},
        idempotencyKey="request/step-1/tc_1",
    )


def test_tokens_and_cost_cannot_exceed_budget() -> None:
    tracker = BudgetTracker(Budget(maxTokens=10, maxCostUsd=0.01))
    with pytest.raises(BudgetExceeded, match="maxTokens"):
        tracker.consume_step(PlannerUsage(promptTokens=8, completionTokens=3))

    tracker = BudgetTracker(Budget(maxCostUsd=0.01))
    with pytest.raises(BudgetExceeded, match="maxCostUsd"):
        tracker.consume_step(PlannerUsage(costUsd=0.02))


def test_tool_calls_have_a_hard_limit() -> None:
    tracker = BudgetTracker(Budget(maxToolCalls=0))
    with pytest.raises(BudgetExceeded, match="maxToolCalls"):
        tracker.authorize_tool(tool_call())
