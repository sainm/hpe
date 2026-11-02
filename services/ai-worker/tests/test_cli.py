from __future__ import annotations

import asyncio
import json
import os
from pathlib import Path


def tool_specs(*, approval: bool = False) -> list[dict[str, object]]:
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
                    "risk": "write" if approval else "read",
                    "approvalRequired": approval,
                    "idempotencyKeyTemplate": "{sessionId}/{step}/{callId}",
                },
            },
        }
    ]


def worker_executable() -> str:
    return str(Path(".venv/bin/hpa-ai-worker").resolve())


async def read_json_line(process: asyncio.subprocess.Process) -> dict[str, object]:
    assert process.stdout is not None
    async with asyncio.timeout(5):
        line = await process.stdout.readline()
    assert line, "worker closed stdout unexpectedly"
    return json.loads(line)


async def test_worker_authenticates_and_emits_bounded_events() -> None:
    environment = {
        **os.environ,
        "HPA_AI_WORKER_TOKEN": "worker-token",
        "HPA_AI_PLANNER": "deterministic",
        "HPA_DETERMINISTIC_TOOL": "workflow.execute",
        "HPA_DETERMINISTIC_ARGUMENTS": '{"workflowId":"wf_1"}',
        "HPA_DETERMINISTIC_OUTPUT": '{"ok":true}',
        "HPA_DETERMINISTIC_TOOL_RESULTS": json.dumps(
            {
                "workflow.execute": {
                    "callId": "tc_1",
                    "status": "succeeded",
                    "result": {"executionId": "exec_1"},
                }
            }
        ),
    }
    process = await asyncio.create_subprocess_exec(
        worker_executable(),
        stdin=asyncio.subprocess.PIPE,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
        env=environment,
    )
    assert process.stdin is not None

    invalid = {
        "type": "start",
        "sessionId": "session-invalid",
        "requestToken": "wrong-token",
        "tenantId": "tenant-1",
        "agentVersionId": "agent-1",
        "input": {},
        "tools": tool_specs(),
    }
    process.stdin.write((json.dumps(invalid) + "\n").encode())
    await process.stdin.drain()
    rejected = await read_json_line(process)
    assert rejected["type"] == "error"
    assert rejected["code"] == "INVALID_REQUEST"
    assert "wrong-token" not in json.dumps(rejected)

    request = {
        "type": "start",
        "sessionId": "session-1",
        "requestToken": "worker-token",
        "tenantId": "tenant-1",
        "agentVersionId": "agent-1",
        "input": {"message": "run"},
        "tools": tool_specs(),
        "budget": {
            "maxSteps": 4,
            "maxToolCalls": 2,
            "timeoutSeconds": 10,
            "maxTokens": 1000,
            "maxCostUsd": 0.1,
        },
    }
    process.stdin.write((json.dumps(request) + "\n").encode())
    await process.stdin.drain()

    events: list[dict[str, object]] = []
    while True:
        event = await read_json_line(process)
        events.append(event)
        if event["type"] == "session.result":
            break
    assert [event["type"] for event in events] == [
        "session.started",
        "step",
        "tool.call",
        "tool.result",
        "step",
        "session.completed",
        "usage",
        "session.result",
    ]
    assert events[-1]["data"] == {"output": {"ok": True}}
    assert "worker-token" not in json.dumps(events)

    process.stdin.close()
    await process.stdin.wait_closed()
    assert await asyncio.wait_for(process.wait(), timeout=5) == 0


async def test_sqlite_checkpoint_resumes_after_process_restart(tmp_path) -> None:
    checkpoint = str(tmp_path / "agent-checkpoint.sqlite")
    common_environment = {
        **os.environ,
        "HPA_AI_WORKER_TOKEN": "worker-token",
        "HPA_AI_PLANNER": "deterministic",
        "HPA_DETERMINISTIC_TOOL": "workflow.execute",
        "HPA_DETERMINISTIC_ARGUMENTS": '{"workflowId":"wf_1"}',
        "HPA_DETERMINISTIC_OUTPUT": '{"ok":true}',
        "HPA_DETERMINISTIC_APPROVAL": "1",
        "HPA_DETERMINISTIC_TOOL_RESULTS": json.dumps(
            {
                "workflow.execute": {
                    "callId": "tc_1",
                    "status": "succeeded",
                    "result": {"executionId": "exec_1"},
                }
            }
        ),
        "HPA_AI_CHECKPOINT_PATH": checkpoint,
    }
    first = await asyncio.create_subprocess_exec(
        worker_executable(),
        stdin=asyncio.subprocess.PIPE,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
        env=common_environment,
    )
    assert first.stdin is not None
    start = {
        "type": "start",
        "sessionId": "session-restart",
        "requestToken": "worker-token",
        "tenantId": "tenant-1",
        "agentVersionId": "agent-1",
        "input": {"message": "run"},
        "tools": tool_specs(approval=True),
        "budget": {
            "maxSteps": 4,
            "maxToolCalls": 2,
            "timeoutSeconds": 30,
            "maxTokens": 1000,
            "maxCostUsd": 0.1,
        },
    }
    first.stdin.write((json.dumps(start) + "\n").encode())
    await first.stdin.drain()
    approval_event: dict[str, object] | None = None
    while approval_event is None:
        event = await read_json_line(first)
        if event["type"] == "approval.required":
            approval_event = event
    first.terminate()
    await asyncio.wait_for(first.wait(), timeout=5)
    assert await asyncio.to_thread(Path(checkpoint).is_file)

    second_environment = {
        **common_environment,
        "HPA_DETERMINISTIC_RESUME": "1",
    }
    second = await asyncio.create_subprocess_exec(
        worker_executable(),
        stdin=asyncio.subprocess.PIPE,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
        env=second_environment,
    )
    assert second.stdin is not None
    resume = {
        "type": "resume",
        "sessionId": "session-restart",
        "requestToken": "worker-token",
        "approval": {"approved": True, "approver": "human-1"},
    }
    second.stdin.write((json.dumps(resume) + "\n").encode())
    await second.stdin.drain()
    events: list[dict[str, object]] = []
    while not any(event["type"] == "session.result" for event in events):
        events.append(await read_json_line(second))
    assert any(event["type"] == "tool.result" for event in events)
    assert events[-1]["data"] == {"output": {"ok": True}}
    second.stdin.close()
    await second.stdin.wait_closed()
    assert await asyncio.wait_for(second.wait(), timeout=5) == 0
