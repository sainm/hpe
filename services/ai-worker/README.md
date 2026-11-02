# HPA AI Worker

Optional Python 3.13 process for LangGraph agent sessions and LiteLLM model access.

The Rust runtime remains authoritative for tenancy, RBAC, approvals, budgets, and tool execution.
This process receives a bounded JSONL session contract over stdin/stdout and never reads the control
plane database directly.

```bash
uv sync --frozen --default-index https://pypi.tuna.tsinghua.edu.cn/simple
HPA_AI_WORKER_TOKEN=local-test uv run hpa-ai-worker
```

`uv.lock` 固定 Python 3.13、清华 PyPI 下载 URL 和 SHA-256 哈希。镜像只改变下载来源，
不改变包名、版本或完整性校验；CI 使用同一镜像。若在企业网络切换镜像，应重新生成并审查
`uv.lock`，不能跳过哈希校验。

Send one JSON object per line:

```json
{"type":"start","sessionId":"agent_01","requestToken":"local-test","tenantId":"local","agentVersionId":"av_01","input":{"message":"Look up order 42"},"budget":{"maxSteps":4,"maxToolCalls":2,"timeoutSeconds":30,"maxTokens":10000,"maxCostUsd":0.25}}
```

The worker emits `session.started`, `step`, `tool.call`, `tool.result`, `approval.required`,
`usage`, `checkpoint`, `session.completed`, or structured `error` events. Unknown or unauthenticated
requests fail closed. A real model call requires LiteLLM/OpenAI-compatible configuration and
credentials; without them the deterministic test planner can still exercise the graph.
