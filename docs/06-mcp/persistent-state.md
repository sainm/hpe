# MCP 持久状态与跨节点通知

> 状态：**P13-03 IN_PROGRESS**。PostgreSQL session/subscription/notification/task 存储、跨节点 `tasks/get/update`、owner-node 命令消费者、双进程 SSE 与节点强停恢复、慢消费者背压和重放、Topic Event outbox、`notifications/tasks` 推送、官方 2025-11-25 核心 conformance、2026-07-28 完整 conformance（含 MRTR 与 tasks extension）已实现；webhook/schedule/task 统一事件账本和真实外部客户端仍未完成。

## 1. 数据边界

五张 `hpa_control` 租户表均启用并强制 RLS：

| 表 | 内容 |
| --- | --- |
| `mcp_sessions` | legacy MCP session、token 绑定、TTL、撤销 |
| `mcp_subscriptions` | `subscriptions/listen` 请求条件、所属 session、TTL |
| `mcp_notifications` | tenant 内单调 sequence 的目录变更 |
| `mcp_tasks` | task 状态、输入/输出、审批摘要、execution 引用、TTL |
| `mcp_task_updates` | 按 owner node 路由的持久 task 命令、重试、租约和终态 |

- session 与 subscription 绑定 `tenant_id + token_id`；同租户的其他 token 也不能复用。
- session 关闭级联删除 subscription；已创建 task 保留，但 `session_id` 置空。
- notification 使用数据库 identity sequence；tenant 之间的 sequence 不要求全局连续。
- JSONB 只保存协议数据和摘要，不保存 bearer token 或 secret。
- PAT/审批/审计/外部服务器元数据（`hpa_tenant.mcp_tokens/mcp_approvals/mcp_audit/mcp_servers`）由 [Phase 15](../11-roadmap/phase-15-mcp-control-postgres.md) 管理，与本节的 session/task 状态分开：`HPA_POSTGRES_RUNTIME=1` 即切到 PostgreSQL，不依赖 `HPA_POSTGRES_MCP=1`。

## 2. 恢复语义

`hpa_local::persistent_mcp::PostgresMcpStore` 提供：

- `create_session` / `validate_session` / `close_session`。
- `create_subscription`：在同一个 SQL 中验证 session/token/tenant/TTL。
- `publish_notification`：目录变更落库。
- `poll_subscription(after_sequence)`：按订阅 kind 过滤，支持断线后从 sequence 恢复。
- `upsert_task` / `get_task` / `cancel_task` / `update_task_projection`：token 隔离、终态不可取消、过期任务不可读。
- `request_task_cancel`：任务所有者节点未知时失败；相同 task/action 只保留一个 pending/claimed 命令。
- `PostgresMcpTaskNode.claim/complete/retry/fail`：owner node 使用 `SKIP LOCKED` 领取命令，30 秒 lease，进程崩溃后可重新领取。

通知采用有界轮询接口，不依赖持有跨节点内存 channel。SSE 使用 `Last-Event-ID` 或保存的最后 sequence 恢复，轮询上限 100 条。每次读取前只统计最多 `max_lag + 1` 条租户内匹配通知；超过 `HPA_MCP_NOTIFICATION_MAX_LAG` 时主动结束连接，SSE `retry` 提示客户端 2 秒后携带最后事件 ID 重连。进程内单节点模式遇到广播 `Lagged` 也结束连接，不再静默跳过事件。

## 3. 启用

```bash
export HPA_POSTGRES_URL='postgresql://runtime@db.internal:5432/hpa'
export HPA_POSTGRES_MCP=1
export HPA_NODE_ID='hpa-control-a'
export HPA_MCP_TASK_TICK_MS=250
export HPA_MCP_NOTIFICATION_MAX_LAG=10000
hpa serve --port 8787
```

`HPA_NODE_ID` 必须在节点生命周期内稳定且跨节点唯一；任务记录保存该值，其他节点收到 `tasks/update` 后把命令路由回执行 owner。未设置时本地默认由 base URL 派生，生产多节点必须显式设置。

启动前必须已运行 `hpa db migrate`。启用后：

- legacy initialize 创建的 session 写入 PostgreSQL；进程重启后仍可校验。
- legacy GET SSE 和 modern `subscriptions/listen` 使用持久 subscription。
- API 目录变更同时写本地广播和 `mcp_notifications`；SSE 按 tenant sequence 轮询。
- `Last-Event-ID` 作为恢复起点。
- 租户内待消费通知超过 backlog 上限时主动断开；保留窗口内的通知可从最后 sequence 重放，窗口外需求应由客户端重新拉取目录状态，不依赖丢失的事件补齐。
- task owner 节点同步本地 execution 投影到 PostgreSQL；状态变化写入 `tasks` notification。
- 其他节点读取或提交 task update 时只访问 PostgreSQL；owner 节点从 `mcp_task_updates` 领取取消命令。
- 未设置 `HPA_POSTGRES_MCP=1` 时保留单节点进程内路径。

## 4. 验证

```bash
export HPA_POSTGRES_TEST_URL='postgresql://localhost:5432/hpa_p13_test'
cargo test -p hpa-local --test postgres_mcp -- --nocapture
HPA_POSTGRES_TEST_URL='postgresql://localhost:5432/hpa_p13_test' \
  cargo test -p hpa-local --test mcp_multinode -- --nocapture
```

覆盖：

- 同租户不同 token、不同租户均不能复用 session。
- 订阅只收到 enabled kind；新 store 实例可按 sequence 继续。
- 跨租户不能轮询 subscription。
- 其他 token 不能读取或取消 task；completed task 不能改为 cancelled。
- 关闭 session 后 subscription 消失，task 保留但解除 session 引用。
- TTL 到期后 session 自动失效。
- task update 只允许持有原 token 的客户端提交；其他 token 得到 task not found。
- owner node 收到的取消命令幂等完成；lease 过期可重领，重复命令不会并发放大。
- 两个独立 `hpa serve` 进程共享 PostgreSQL：node-b 订阅 `tasksChanged`，node-a 创建审批等待任务；强停 node-a 后由 node-b 提交取消，node-a 使用相同 `HPA_NODE_ID` 重启后完成取消并触发 node-b SSE。
- SSE 积压从 25 条超过测试上限 20 时连接结束；以倒数第 3 条为 `Last-Event-ID` 重连后，最后两条持久通知按 sequence 重放。
- backlog 统计受 tenant/token 订阅边界和数量上限保护；其他租户不能探测订阅积压。
- `pnpm test:mcp-conformance` 运行官方 `@modelcontextprotocol/conformance@0.2.0-alpha.12` 2025-11-25 核心 server 场景；`pnpm test:mcp-conformance:2026` 运行 2026-07-28 `server-stateless` 28/28 PASS；`test:mcp-conformance:2026-mrtr` 覆盖 14 个 MRTR 场景、40 项检查全部通过；`test:mcp-conformance:2026-tasks` 覆盖 10 个 tasks extension 场景，完整 requirements 193 passed / 0 failed。

## 5. 尚未完成

- 当前跨节点通知是 PostgreSQL 持久 sequence + 有界轮询，不是 `LISTEN/NOTIFY` 或 Redis Stream 低延迟推送；正确性不依赖内存 channel。
- Topic Event 已使用 PostgreSQL outbox + shared trigger fire 去重；webhook、schedule 和 task 仍使用各自持久记录，尚未统一到一张幂等事件账本。
- tasks extension 的 conformance fixture 使用进程内任务状态；生产持久任务仍由 PostgreSQL task 投影与 execution handle 承担。`subscriptions/listen` 的 `notifications/tasks` 已覆盖内存 fixture 与持久任务；官方场景仍跳过，通知本身是可选能力。
- 事件总线统一账本和至少两个真实外部客户端尚未验证。
