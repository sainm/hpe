# PostgreSQL 分布式运行时原语

> 状态：**P13-02 IN_PROGRESS**。事务 outbox、幂等账本、Scheduler fencing lease、实际 Scheduler leader 选举/续租/故障接管、execution owner lease/fencing、Worker Job lease/re-drive 已在 PostgreSQL 18 实现并通过真实双进程测试；Workflow/Execution/Trigger 已可选 PG 路由，但 Worker Job API 仍使用 SQLite，完整多控制面滚动升级仍属于外部环境待验证项。

## 1. 语义

运行时采用 PostgreSQL 作为可靠队列基线，不额外引入第二套消息真相：

- 业务事务和 outbox INSERT 同事务提交。
- Consumer 使用 `FOR UPDATE SKIP LOCKED` 领取消息，多个节点不会同时持有同一有效 lease。
- 认领递增 `fencing_token`；完成、失败和 Worker 上报必须同时匹配 lease owner 与 token。
- 旧 owner 在 lease 过期、任务已被接管后不能提交结果。
- 投递是至少一次；消费者必须用 `tenant_id + idempotency_key` 去重。不得宣称外部副作用 Exactly Once。

## 2. Outbox

`hpa_control.outbox` 保存 tenant、topic、event type、aggregate、payload、trace、尝试次数、下一次可用时间、lease 和 fencing token。

```sql
BEGIN;
SELECT set_config('hpa.tenant_id', $tenant_id, true);
-- business mutation
INSERT INTO hpa_control.outbox (...)
VALUES (...)
ON CONFLICT (tenant_id, idempotency_key) DO NOTHING;
COMMIT;
```

Rust API 位于 `hpa_local::distributed_runtime::TenantRuntime`：

- `enqueue`：`tenant_id + idempotency_key` 唯一。
- `claim`：调用 `hpa_internal.claim_outbox`，返回 lease token。
- `complete` / `fail`：必须匹配 consumer 和 token。
- 失败后按 available time 重投；达到 `max_attempts` 后进入 `dead`。

## 3. 幂等账本

`hpa_control.idempotency_keys` 以 `(tenant_id, scope, idempotency_key)` 为主键，保存 request hash、状态、资源引用和响应快照。

- 相同 key + 相同 hash：返回已有 in-progress/succeeded/failed 状态。
- 相同 key + 不同 hash：返回 `IdempotencyKeyReused`，不执行第二次副作用。
- 完成后保存 resource type/id 和响应，可直接返回稳定结果。
- 账本和 outbox 均受 tenant RLS 约束。

当前已经覆盖 Workflow Job、Trigger event 和通用 Agent tool side effect 的键模型；具体 API 路由接入仍属于后续切换工作。

## 4. Topic Event Bus

`HPA_POSTGRES_RUNTIME=1 HPA_POSTGRES_EVENT_BUS=1` 启用异步 Topic Event：

1. `/api/events/{topic}` 先校验当前租户存在 enabled subscriber。
2. `TenantRuntime.enqueue` 在 PostgreSQL outbox 写入 `topic=hpa.event.topic`、`event_type=topic.published` 和稳定 `topic-event:{eventId}` 幂等键。
3. 任一节点 consumer 使用 `claim_outbox_topic(... SKIP LOCKED)` 领取，租约 30 秒。
4. consumer 从 PostgreSQL 读取冻结 workflow/version，调用同一 Trigger/Execution Repository 执行 fan-out，完成后 `complete`；失败按 2 秒重投，最多 10 次进入 dead。
5. execution、start event 与 trigger fire 在同一 PostgreSQL 事务提交；重复 Event ID 导致 fire 唯一冲突时整事务回滚，不会留下孤立 execution。
6. 崩溃后新 consumer 重新领取时，共享 `trigger_fires` 逐订阅者去重，只补投缺失订阅者。

发布专用节点可设置 `HPA_EVENT_BUS_CONSUMER=0`，保留 Event API/outbox 写入但不竞争消费；consumer 节点仍需相同 PostgreSQL 和 active tenant 控制库记录。

API 对同 Event ID + 同 payload 返回 202 `duplicate` 和原 message ID；同 ID + 不同 payload 返回 409 `EVENT_ID_CONFLICT`。异步模式响应不再同步返回 execution ID，客户端应查询 trigger fire/execution 或 SSE。

当前统一范围是 Topic Event。Webhook/schedule 已写 PostgreSQL trigger facts，但尚未进入 outbox；MCP task 仍使用独立 task update 表。Redis Stream 与跨控制面低延迟广播尚未实现。

## 5. Scheduler leadership

`hpa_control.runtime_leases` 是数据库时钟驱动的 leader lease：

- `acquire` 只在无 lease 或旧 lease 已过期时成功。
- 每次接管递增 `fencing_token`。
- `renew` 必须匹配 holder 和 token，且 lease 尚未过期。
- `release` 只删除当前 holder/token 的记录。
- TTL 范围 1–3600 秒；所有比较使用数据库 `now()`，避免跨节点本地时钟直接决定领导者。

Scheduler 运行连接应 `SET ROLE hpa_scheduler_runtime`。该角色没有 tenant 数据权限，也没有 `BYPASSRLS`。

启用实际 Scheduler coordination：

```bash
export HPA_POSTGRES_URL='postgresql://runtime@db.internal:5432/hpa'
export HPA_POSTGRES_SCHEDULER=1
export HPA_NODE_ID='hpa-scheduler-a'
export HPA_SCHEDULER_LEASE_KEY='hpa:scheduler:production'
export HPA_SCHEDULER_LEASE_TTL_SECONDS=15
hpa serve --port 8787
```

每个 Scheduler 在每轮及每个 tenant 前续租；standby 仍更新本地 agent heartbeat 并暴露 `hpa_scheduler_leader 0`，但不执行 schedule。leader 的 `hpa_scheduler_leader` 为 1。正常退出释放 lease；强杀后由 TTL 过期触发接管。

## 6. Worker lease

`worker_jobs` 在现有 `lease_id` 上增加单调 `fencing_token`：

- 只领取 `queued` 或 lease 已过期的任务。
- Worker capability 必须覆盖 Job 的 `required_capabilities_json`。
- heartbeat 更新注册 Worker 的最后心跳。
- renew、complete 和 requeue 同时校验 Worker、lease id、fencing token、状态与未过期 lease。
- requeue 清空 Worker/lease 并回到 `queued`；下一次领取递增 token。

Worker 连接应 `SET ROLE hpa_worker_runtime`，并在同一事务设置 `hpa.tenant_id`。

## 7. Execution lease

`hpa_tenant.execution_leases` 控制 running Workflow 的 driver owner：

- 每个控制面以稳定 `HPA_NODE_ID` 获取 execution lease；
- step 前续租，默认 TTL 30 秒；
- `save_execution_fenced` 同时校验 holder、fencing token、lease 未过期和 execution tenant；
- 过期 takeover 递增 token，旧 owner 的新 state/event commit 与 release 均失败；
- recovery monitor 周期扫描 running execution，每个节点都可尝试接管，但只有一个 lease winner 驱动。

## 8. 验证

```bash
export HPA_POSTGRES_TEST_URL='postgresql://localhost:5432/hpa_p13_test'
cargo test -p hpa-local --test postgres_distributed -- --nocapture
HPA_POSTGRES_TEST_URL='postgresql://localhost:5432/hpa_p13_test' \
  cargo test -p hpa-local --test postgres_scheduler_leader -- --nocapture
HPA_POSTGRES_TEST_URL='postgresql://localhost:5432/hpa_p13_test' \
  cargo test -p hpa-local --test postgres_event_bus -- --nocapture
```

测试覆盖：

- 同一 outbox 行被两个 consumer 并发领取时只有一个成功。
- 过期 lease 被另一 consumer 接管，旧 token 无法完成。
- 有界重试后进入 `dead`。
- 幂等 key 相同 hash 可重放；不同 hash 失败关闭。
- Scheduler 同一时刻只有一个 holder；过期接管后旧 holder 不能 renew。
- 两个独立 `hpa serve` 进程启用 `HPA_POSTGRES_SCHEDULER=1` 后恰好一个暴露 leader=1；强杀 leader 后 standby 在 TTL 内接管；重启旧 leader 仍保持 standby。
- Worker Job heartbeat、过期接管、旧 Worker 完成拒绝、requeue 和再次领取。
- Event Bus：API queued→outbox processed，PG trigger fire/execution 各 1；同 payload duplicate、不同 payload conflict，最终 fire/execution 数量不增加。
- Event Bus 双节点：发布节点禁用 consumer，另一节点没有 Workflow SQLite 副本；consumer 仍从 PG 读取版本并完成 execution，证明 outbox/trigger/execution 路由不依赖原发布节点。
- Atomic fire：两次并发/重复 fire 使用同一 Event ID 时，第二次 execution insert 随 trigger fire 唯一冲突一起回滚，最终 execution/fire 各 1。

## 9. 运维

待处理积压：

```sql
SELECT tenant_id, status, count(*), min(available_at), max(attempts)
FROM hpa_control.outbox
GROUP BY tenant_id, status
ORDER BY tenant_id, status;
```

死信：

```sql
SELECT tenant_id, id, topic, attempts, last_error, updated_at
FROM hpa_control.outbox
WHERE status = 'dead'
ORDER BY updated_at;
```

生产告警至少应覆盖 outbox oldest age、dead count、claim latency、lease takeover 和 scheduler lease 过期。尚未接入 OTLP/持久化指标。

## 10. 尚未完成

- Scheduler 已接入 leader coordination；Workflow/Trigger/Execution、Worker Job、Enterprise、Agent、Skill、MCP、Connector 与 Plugin 元数据在 `HPA_POSTGRES_RUNTIME=1` 时均使用 PostgreSQL 权威。
- 尚无真实两台控制面、两台 Worker 的滚动升级/强杀演练。
- Topic Event 已接 PostgreSQL outbox；Webhook、schedule 与 MCP task 仍使用各自记录，尚未成为一张统一事件账本。
- 尚无 Redis Stream 适配器；PostgreSQL queue 是当前选定的“等价可靠队列”，容量上限和多区域复制需在 P13-06 压测。
- 外部副作用仍需业务系统提供自己的幂等键；outbox 只保证平台记录和投递可恢复。
