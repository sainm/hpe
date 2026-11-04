# Phase 26：MCP 2026-07-28 tasks extension conformance

> 状态：**本地验收完成**（2026-11-04）。SEP-2663 tasks extension 的 10 个官方场景 0 failed（9 个有检查场景 44/44 通过，`tasks-status-notifications` 被套件跳过）；2026-07-28 完整 requirements 总报告 **193 passed / 0 failed**（37 个计分 server 场景、10 个 tasks extension 场景与 pending 场景全部无失败）。

## 1. 范围

- `server/discover` 在 `capabilities.extensions["io.modelcontextprotocol/tasks"]` 声明扩展，不再使用 v1 风格的 `capabilities.tasks`。
- 工具 fixture：`greet`（同步）、`slow_compute`（可选任务）、`failing_job`（必需任务）、`protocol_error_job`、`confirm_delete`、`multi_input`、`test_tool_with_task`。
- `tools/call` 返回扁平 `CreateTaskResult`：`resultType:"task"`、`taskId`、`status`、`createdAt`、`lastUpdatedAt`、`ttlMs`、`pollIntervalMs`，不含 `result`/`error`/`inputRequests`/`requestState`。
- `tasks/get` 返回 `DetailedTask`：working 任务只带状态与元数据；completed 内联 `result`；failed 内联 `error`；input_required 返回 `inputRequests`；不出现 v1 `related-task` `_meta`。
- `tasks/update` 与 `tasks/cancel` 返回空 `{resultType:"complete"}` ack，不携带任务信封；取消后的状态通过下一次 `tasks/get` 观察，终态取消幂等。
- MRTR 组合：`test_tool_with_task` 首轮返回 `InputRequiredResult`，重试轮直接升级为 `CreateTaskResult`，最终任务结果包含 MRTR 收集的 `user_name`。
- `confirm_delete` 单输入、`multi_input` 双输入：`tasks/update` 支持部分满足，未回答的 key 保留；全部回答后进入终态。
- 未声明扩展时：`tasks/get`、`tasks/update`、`tasks/cancel` 返回 `-32021`，错误数据包含 `requiredCapabilities.extensions`；`slow_compute` 回退同步执行，不创建任务。
- 单请求 opt-in：在 `_meta.clientCapabilities.extensions` 声明扩展时，即使会话未声明也创建任务。
- tasks 方法的 `Mcp-Name` 必须匹配 `params.taskId`；`tasks/result` 与 `tasks/list` 返回 `-32601`。

## 2. 行为契约

- 所有 tasks fixture 仅在 `HPA_MCP_CONFORMANCE=1` 的临时 Runtime 注册；任务状态存放在进程内，供无状态请求轮询，不写 SQLite/PostgreSQL。
- 除该模式外，常规工具目录与执行任务 API 不变；生产 `tasks/get/update` 仍映射到 execution handle，legacy `{update:{status:"cancelled"}}` 路径保留。
- 现代请求的 tasks 方法要求扩展协商；内部管理客户端、MCP Client 测试与 UI 在 `_meta.clientCapabilities` 中声明扩展。
- 任务状态机：`working` → `completed` / `failed` / `cancelled`；`input_required` → `working`/`completed`/`cancelled`；终态不可回退。
- `ttlMs`/`pollIntervalMs` 使用整数毫秒；任务在其 `ttlMs` 内保持可读。
- conformance 脚本将 `HPA_MCP_RATE_LIMIT` 提高到 100000，仅用于官方套件的突发请求；产品默认仍为 120/min。

## 3. 验收

```bash
cargo build -p hpa-local
pnpm test:mcp-conformance:2026-tasks
HPA_CONFORMANCE_REQUIREMENTS=2026-07-28 pnpm test:mcp-conformance
```

- tasks 场景 10 个：`tasks-lifecycle` 9/9、`tasks-capability-negotiation` 5/5、`tasks-wire-fields` 4/4、`tasks-request-state-removal` 3/3、`tasks-mrtr-input` 4/4、`tasks-request-headers` 5/5、`tasks-dispatch-and-envelope` 9/9、`tasks-required-task-error` 3/3、`tasks-mrtr-composition` 2/2，合计 44/44；`tasks-status-notifications` 由套件标记为可选并跳过，0 failed。
- 完整 2026-07-28 requirements：**193 passed / 0 failed**；37 个计分 server 场景全部通过，10 个 tasks extension 场景全部通过，pending 场景不失败。
- 2025-11-25 核心矩阵 9/9、`server-stateless` 28/28、内容/Header 18/18、MRTR 14 场景/40 检查保持通过。
- Rust 全量测试 252 passed / 0 failed / 1 ignored，Clippy `-D warnings` 与 fmt 通过。
- 真实 PostgreSQL 18.4：31 个 PG 测试文件（32 用例）与 `mcp_multinode` 跨节点强停/恢复用例通过；备份演练恢复到临时库后 21 个 migration、71 个业务关系、68 张 FORCE RLS 表一致。
- Web 12 Vitest、typecheck/build；扩展 65 Vitest + 7 个 Extension Host 检查、build；文档 183 文件 886 链接、品牌/IR/契约/镜像/OTel/供应链门禁通过。

## 4. 非目标

- 不把 conformance fixture 暴露到常规 Runtime，也不把进程内任务存储用于生产持久任务。
- 不实现 `notifications/tasks` 的 subscriptions/listen 推送；官方场景当前跳过，通知仍为可选。
- 不改变真实外部 AI 客户端验收状态；仍为 `BLOCKED_EXTERNAL`。
