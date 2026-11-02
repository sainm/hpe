# Python LangGraph Worker

> 状态：**P13-04 IN_PROGRESS**。Python 3.13 + LangGraph + LiteLLM 的显式 `python` planner 已接入 Rust Agent Runtime；Rust ToolService、会话能力、工具幂等账本、审批恢复、跨请求取消、控制面强停后继续、SQLite/PostgreSQL checkpoint、只读 rootfs/cgroup/无网络容器演练、sub-agent handoff 和受控 OpenAI-compatible 测试已落地。真实云模型、受控模型网络的容器网关、mTLS 和多副本会话路由尚未完成。

## 1. 边界

Python Worker 是独立可选进程：

```text
Rust Agent Runtime (`planner.kind = "python"`)
  -> authenticated JSONL stdin/stdout
  -> Python 3.13 process
       -> LangGraph state graph
       -> LiteLLM / OpenAI-compatible model gateway
       -> HTTP ToolService (`/api/internal/agent-tool-calls`)
       -> Rust risk/approval/idempotency checks
       -> Rust-owned Tool execution + audit
```

- Python 不读取 `control.db`、PostgreSQL Canonical 数据或租户 secret store。
- Rust 继续负责租户、RBAC、审批权威状态、工具执行、外部副作用和审计。
- Worker 请求必须携带进程级 `requestToken`；比较使用 constant-time compare。
- Tool token 只保存在进程内 session map，不进入 LangGraph state/checkpoint。
- JSONL 每行最大 1 MiB；未知字段、非法 ID、超预算和错误 token 都 fail-closed。

## 2. 请求与事件

启动 Worker：

```bash
cd services/ai-worker
uv sync --frozen --default-index https://pypi.tuna.tsinghua.edu.cn/simple
export HPA_AI_WORKER_TOKEN='process-secret'
export HPA_AI_MODELS='openai/your-model'
export HPA_LITELLM_BASE_URL='https://litellm.internal/v1'
export HPA_MODEL_API_KEY='provider-key-from-secret-store'
export HPA_TOOL_ENDPOINT='https://control.internal/internal/agent-tools'
uv run --frozen hpa-ai-worker
```

请求类型：

- `start`：session/tenant/AgentVersion/input/budget，可选 model 与 tool token。
- `resume`：同一 session + 人工审批结果，可选重新注入 tool token。
- `cancel`：协作取消当前 asyncio task。

事件包括 `session.started`、`step`、`tool.call`、`tool.result`、`approval.required`、`usage`、`session.completed`、`session.cancelled`、`session.result` 和结构化 `error`。Event 带单调 sequence；断线重连允许至少一次重复，消费者按 session + sequence 去重。

## 3. LangGraph

- `plan -> tool -> plan -> finish` 状态图。
- 模型 planner 输出结构化 `PlannerDecision`，Schema 未知字段拒绝。
- `x-hpa.risk` 与 `x-hpa.approvalRequired` 只能来自 Rust 生成的工具清单；模型自报值会被覆盖，元数据缺失或非法时 fail-closed。
- 高风险 tool 在图中调用 LangGraph `interrupt()`；只有 Rust 审批记录、调用绑定能力和审批恢复值同时匹配才允许执行。
- 重复工具计划达到 3 次自动失败，避免无限循环。
- 预算镜像检查 steps、tool calls、wall-clock、tokens 和 cost；Rust 仍是最终权威。
- `HPA_AI_CHECKPOINT_PATH` 使用 `AsyncSqliteSaver`；控制面或 Worker 被终止后，会话进入 `interrupted`，`continue` 使用同一 thread ID 恢复。已提交的工具结果由 Rust `agent_tool_calls` 账本查询，不盲目重复外部副作用。
- 未设置 checkpoint path 时使用内存 saver，仅适合确定性 smoke，不具备进程恢复。

设置 `HPA_AI_CHECKPOINT_POSTGRES_URL` 时优先使用官方 `AsyncPostgresSaver`：

```bash
export HPA_AI_CHECKPOINT_POSTGRES_URL='postgresql://...'
export HPA_AI_CHECKPOINT_SCHEMA='hpa_agent_cp_<tenant-hash>'
```

- Rust 根据 tenantId 生成稳定的 `hpa_agent_cp_<20 hex>` schema 名，并注入 Worker；schema 名严格匹配 `^[a-z_][a-z0-9_]{0,62}$`。
- checkpoint 表只写入独立 schema，不与 Rust `hpa_control` / `hpa_tenant` 混写。
- 首次建表使用 PostgreSQL advisory lock 串行化；多个 Worker 同时启动不会并发迁移同一 schema。
- PostgreSQL 不可用或 schema 无权限时启动失败，不回退到 SQLite，避免同一 session 静默使用不同事实存储。
- 生产应给 Worker 使用仅能访问本租户 checkpoint schema 的独立数据库角色；共享超级用户连接只能用于本地受控测试。

生产可预建 schema，使 Worker 角色不需要数据库级 `CREATE`：

```sql
CREATE SCHEMA hpa_agent_cp_0123456789abcdef0123
  AUTHORIZATION hpa_agent_worker;
GRANT USAGE, CREATE ON SCHEMA hpa_agent_cp_0123456789abcdef0123
  TO hpa_agent_worker;
```

Worker 只会对已存在 schema 执行官方 migration；schema 不存在且角色无创建权限时 fail-closed。

`uv.lock` 当前固定清华 PyPI URL 与每个 artifact 的 SHA-256。国内镜像只改变下载来源；切换镜像必须重新 lock 并审查 diff，不能关闭 hash 校验。

## 4. Rust 客户端与路由

`hpa_local::ai_worker_client::AiWorkerProcess`：

- 使用 argv 数组启动独立进程，不经过 shell。
- 注入进程 token，pipes stdin/stdout，`kill_on_drop`。
- `start` 传递 Agent instructions、Rust 工具清单、剩余预算、model override、tenant/session 和短期 tool capability。
- 提供 `start`、`resume`、`cancel`、`next_event(timeout)` 和 graceful shutdown；恢复请求重新注入剩余预算与调用绑定能力。
- `planner.kind = "python"` 是显式选择；未配置 `HPA_AI_WORKER_BIN` 时只有该 planner 失败，deterministic/OpenAI-compatible 路径不受影响。
- 每个 session 使用独立 Python 子进程和 checkpoint 文件；活动进程记录在 Rust 内存取消表中，跨 HTTP 请求的 cancel 会向 stdin 写入取消命令。
- 测试验证 Rust → Python → ToolService → Workflow 完整回合、模型伪造低风险被覆盖、人工审批、跨请求取消、控制面强停后 continue 和进程级 resume/cancel。

`hpa serve` 不会自动改变既有 AgentVersion 的 planner。只有在 AgentVersion 中显式设置 `{"kind":"python","model":"..."}` 才启动 Worker；Agent Center 已提供 Python/LangGraph 模板，现有审批/会话/事件 API 同时服务三种 planner。

## 5. ToolService

Python 不执行 Skill、Workflow、Connector 或 MCP 工具。`HttpToolExecutor` 调用：

```http
POST /api/internal/agent-tool-calls
Authorization: Bearer hpa_tool_...
x-hpa-tenant-id: tenant-1
Content-Type: application/json

{
  "tenantId": "tenant-1",
  "sessionId": "agentsess_...",
  "callId": "call_...",
  "tool": "workflow.execute",
  "arguments": {"workflowId": "wf_1", "version": "1.0.0"},
  "idempotencyKey": "session/step/call",
  "risk": "external"
}
```

Rust 独立重验 tenant/session capability、AgentVersion 工具声明、权威风险、审批绑定和稳定工具账本。审批能力同时绑定 `approvalId + callId + canonical request hash`；模型不能把 `external/high` 降为 `read` 后执行。响应使用平台 `ToolResult`：

```json
{"callId":"call_...","status":"succeeded","result":{},"retryable":false}
```

账本键稳定包含 session、callId 与规范化请求哈希。相同请求返回首次结果；同 callId 不同参数、in-progress 重入或审批能力错配均拒绝。账本避免重复内部执行记录，但不宣称外部副作用 Exactly Once。

`agent.handoff` 也是 Rust 特殊工具，不进入 Python 本地执行器：

- 只允许 AgentVersion `subAgents` 中的 agentId。
- 子 Agent 使用父会话剩余 tokens/cost/handoff/depth，并为当前 handoff 预留 1 step 和 1 tool call 后再继承，不能重新获得完整预算。
- 父会话记录 childSessionId、handoff 次数，并在下次 usage 回传时聚合子工具次数、token 和成本。
- 子 Agent 等待审批时父 handoff 失败并取消遗留子会话，父 Agent 不代替子 Agent 批准。
- 非法/未声明/超深 handoff 在 Rust ToolService 拒绝，并使用同一工具幂等账本。

## 6. LiteLLM

`LiteLlmPlanner` 只使用 LiteLLM 的 OpenAI-compatible `acompletion`，返回平台 `PlannerDecision` 和 usage；provider 类型不进入 AgentVersion/Session 契约。

- 模型 fallback 列表来自 `HPA_AI_MODELS`。
- API key 从 `HPA_MODEL_API_KEY` 读取，不写日志/异常正文。
- 测试使用本地 OpenAI-compatible server，确认 LiteLLM 发出 `/chat/completions` 并解析 usage。
- 真实 Claude/OpenAI/其他 provider 调用需要外部凭证，当前状态为 `BLOCKED_EXTERNAL`。

## 7. 容器隔离

`services/ai-worker/Containerfile` 使用国内镜像与清华 PyPI 构建非 root 只读镜像：

基镜像固定为
`docker.m.daocloud.io/library/python:3.13-slim@sha256:7b58e4266b99c869d2225c050345f7c7fec255d9e3baeca19ea7de9411411c91`。

```bash
podman build --network=host --pull=missing \
  -f services/ai-worker/Containerfile \
  -t localhost/hpa-ai-worker:phase13 services/ai-worker
HPA_CONTAINER_RUNTIME=podman \
  node scripts/phase13-ai-worker-container-smoke.mjs
```

当前演练验证：

- UID/GID 10001，非 root。
- `--read-only` rootfs；`/tmp` 为 `noexec,nosuid` tmpfs。
- `--network=none`，容器内只有 loopback，外网连接失败。
- `--memory=512m`、`--cpus=1`、`--pids-limit=64`。
- `--cap-drop=ALL` 与 `no-new-privileges`.
- 同一隔离配置完成 deterministic LangGraph `session → tool → result` 回合。

当前 Podman 构建内容 ID 为
`sha256:31be586e8569f71935fc24d832e3e8ca5038b436d03247d3f1d10e03705369b5`。
该记录是本地构建证据；正式发布仍需 Registry digest、签名/来源证明和独立 SBOM 归档。

本机 Podman 5.8.5 AppleHV machine 出现过 `machine start` 返回成功后 socket 立即停止的问题。演练脚本不会自动启动或假设 VM 可用；运行前必须先确认 `podman version` 返回 server 版本，否则以容器运行时不可用失败退出。

该演练不使用真实模型网络。LiteLLM/模型网关容器化时必须增加仅允许 Rust Model/Tool gateway 的受控网络路径、DNS/egress policy 和 mTLS 证据；直接给容器开放公网不能视为生产隔离完成。

## 8. 验证

```bash
cd services/ai-worker
HPA_POSTGRES_TEST_URL=postgresql://localhost:5432/hpa_p13_test \
  uv run --frozen pytest -q
uv run --frozen mypy src
uv run --frozen ruff check .
uv run --frozen ruff format --check .
uv run --frozen pip-audit --local --progress-spinner off
uv export --frozen --format cyclonedx1.5 --output-file ../../target/sbom/hpa-ai-worker.cdx.json

cd ../..
HPA_AI_WORKER_BIN="$PWD/services/ai-worker/.venv/bin/hpa-ai-worker" \
HPA_POSTGRES_TEST_URL=postgresql://localhost:5432/hpa_p13_test \
  cargo test -p hpa-local --test ai_worker_process -- --nocapture
HPA_AI_WORKER_BIN="$PWD/services/ai-worker/.venv/bin/hpa-ai-worker" \
HPA_POSTGRES_TEST_URL=postgresql://localhost:5432/hpa_p13_test \
  cargo test -p hpa-local --test agents python -- --nocapture
```

当前测试覆盖 19 项 Python 测试、模型 fallback、SQLite/PostgreSQL checkpoint、并发初始化、租户 schema 校验、Rust 进程契约、模型→Python→Rust ToolService、审批覆盖、风险降级拒绝、幂等重放、跨请求取消、控制面强停恢复和 hardened container smoke。

`pip-audit` 对 117 个已安装/锁定依赖的审计为 0 已知漏洞。LiteLLM 1.106.6 声明
`filelock<4.0`，但其旧版本存在已知漏洞；项目在 `[tool.uv].override-dependencies`
中固定 `filelock==4.2.1`，并用完整 LiteLLM 受控调用回归确认兼容。该 override 在升级
LiteLLM 时必须重新审查并移除以恢复上游约束。

## 9. 尚未完成

- 无真实企业模型凭证的模型调用；`BLOCKED_EXTERNAL`。
- 多副本 Agent session 路由和 AI Worker mTLS。
- 受控模型网关网络、DNS/egress policy 和容器 mTLS 尚未完成；无网络 deterministic 容器演练不能替代真实模型网络隔离。
- 生产 ToolService 账本仍未整体切换到 PostgreSQL；SQLite API 与 PostgreSQL schema 已同时定义，待 P13-01 控制面切换。
