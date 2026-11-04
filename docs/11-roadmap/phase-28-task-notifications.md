# Phase 28：MCP `notifications/tasks` 推送

> 状态：**本地验收完成**（2026-11-04）。`subscriptions/listen` 的 `tasksChanged` 订阅现在接收 SEP-2663 的 `notifications/tasks`；内存 fixture 任务与 PostgreSQL 持久任务使用同一方法，任务创建、完成、失败和取消都会推送 `taskId`/`status`。

## 1. 范围

- 将任务通知方法从 v1 风格的 `notifications/tasks/status_changed` 统一为 SEP-2663 的 `notifications/tasks`。
- `subscriptions/listen` 的 `subscriptions.tasksChanged` 控制是否订阅任务状态；未声明时不发送。
- 内存路径：conformance fixture 任务在创建、`working`/`input_required`/`completed`/`failed`/`cancelled` 状态变化时广播 `taskId`、`status`、`updatedAt`，终态附带 `result` 或 `error`。
- PostgreSQL 路径：持久任务投影更新继续发布 `tasks` 通知，SSE 方法改为 `notifications/tasks`，保留跨节点 task update 重放。
- 每条通知的 `_meta.io.modelcontextprotocol/subscriptionId` 与订阅请求 id 一致。
- 幂等终态取消不重复发送通知；无状态变化的空 `tasks/update` 不发送通知。

## 2. 行为契约

- 通知只出现在 `subscriptions/listen` 的现代协议流；legacy GET SSE 与轮询路径不变。
- `tasksChanged` 是 opt-in；客户端也可继续使用 `tasks/get` 轮询，推送不是必需。
- 通知是提示，不替代 `tasks/get` 的权威 DetailedTask；重连与去重仍按任务状态读取。
- 持久任务通知经 PostgreSQL `mcp_notifications` 序列化，受租户、订阅和 backlog 上限约束；内存通知经进程内 broadcast，慢消费者按既有策略断开重连。
- 官方 conformance 的 `tasks-status-notifications` 场景仍由套件标记为可选并跳过；本阶段以仓库内 SSE/跨节点测试作为验收证据。

## 3. 验收

```bash
cargo test -p hpa-local --test mcp_http conformance_task_status_notifications_are_streamed
HPA_POSTGRES_TEST_URL=... cargo test -p hpa-local --test mcp_multinode
HPA_CONFORMANCE_REQUIREMENTS=2026-07-28 npm run test:mcp-conformance
```

- 新 SSE 测试：订阅 `tasksChanged` 后调用 `slow_compute` fixture，流中同时出现 `notifications/tasks` 的 `working` 与 `completed` 状态、正确 `taskId` 和 `subscriptionId`。
- `mcp_multinode` 跨节点测试：node-a 停机、node-b 提交取消、node-a 重启后，node-b SSE 收到 `notifications/tasks` 的 `cancelled`。
- 完整 2026-07-28 requirements 保持 **193 passed / 0 failed**；tasks extension 10 个场景 0 failed。
- Rust 全量 253 passed / 0 failed / 1 ignored；PostgreSQL 31 文件（32 用例）、Web 12、扩展 65 + 7 host checks、文档 185 文件 896 链接与供应链门禁保持通过。

## 4. 非目标

- 不实现 `notifications/tasks` 的低延迟 `LISTEN/NOTIFY`/Redis 传输；当前持久通知仍是有界轮询。
- 不改变任务状态机、`tasks/get` 权威读取或 `tasks/update` 部分输入语义。
- 不把官方跳过场景计为 conformance PASS；它仍是可选扩展能力。
