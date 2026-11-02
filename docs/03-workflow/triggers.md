# 触发器与调度（Phase 11）

> 状态：本地实现完成（2026-11-01）。`triggers` 从“IR 里的声明”变成真正会执行的入口。
> 调度与 Webhook 只运行**已发布版本**，与 MCP Gateway 的语义一致。

## 1. 为什么要有这一层

Phase 1–10 里 `triggers` 只是 IR 的一个字段：schema 允许、JSON 能解析、设计器能原样保存，但运行时没有任何代码读取它。结果是「Automation First」只能手动点运行——没有定时、没有外部事件入口。

Phase 11 补齐三件事：

- **调度**：`schedule` 触发器按 cron 到点自动启动执行。
- **入站**：`webhook` 触发器让外部系统用签名请求启动执行。
- **内部事件**：`event` 触发器订阅 topic，通过 `/api/events/{topic}` 启动匹配的已发布版本；`mcp` 仍走 `workflow.execute` 工具。

## 2. 配置契约

`trigger.config` 由自由对象收紧为强类型结构，未知字段一律拒绝（`deny_unknown_fields`）。

### 2.1 schedule

```yaml
triggers:
  - id: nightly-report
    type: schedule
    config:
      cron: "0 9 * * *"        # 必填，5 字段
      timezone: "+08:00"       # 可选，默认 UTC
      misfire: skip            # skip | run_once | catch_up，默认 skip
      maxCatchUp: 3            # 仅 catch_up 可用，1..10，默认 10
      input: { window: daily } # 可选，注入执行输入
      enabled: true            # 可选，默认 true
```

- **cron**：`分 时 日 月 周`，支持 `*`、`n`、`a-b`、`*/n`、`a-b/n`、列表、月份/星期英文缩写（`JAN`、`MON`）。周日可写 `0` 或 `7`。
- **日与周同时受限时取并集**（Vixie cron 语义），例如 `0 0 1 * MON` = 每月 1 号 **或** 每周一。
- **时区只接受 `UTC` 与固定偏移**（`+08:00`、`-0530`、`+9`）；`Asia/Shanghai` 这类 IANA 名称会被拒绝，因为本构建不带时区数据库，静默按固定偏移处理会在夏令时上出错。

### 2.2 webhook

```yaml
triggers:
  - id: github-release
    type: webhook
    config:
      secret: "至少 16 个字符的共享密钥"
      enabled: true
```

- `secret` 缺失时触发器仍存在，但**所有投递都会被拒绝**（401 `HOOK_NO_SECRET`），避免“看起来能收、实际裸奔”。
- 发布后 URL 固定为 `{base}/hooks/{tenant}/{workflowId}/{triggerId}`，另有 `{base}/hooks/{workflowId}/{triggerId}` 等价于 `local` 租户。

### 2.3 event / mcp

```yaml
triggers:
  - id: invoice-created
    type: event
    config:
      topic: invoice.created
      enabled: true
```

- `topic` 省略时使用 trigger id；只允许 `[A-Za-z0-9._:-]{1,128}`。
- 发布 `POST /api/events/{topic}` 时，所有同租户、同 topic 且 enabled 的触发器各启动一次已发布版本。
- Event ID 幂等按订阅者记录。若一次 fan-out 只有部分订阅者成功，携带同一 Event ID 重试时只补投缺失订阅者；所有订阅者均已投递才返回 409 duplicate。
- `X-HPA-Event-Id` 可选；同一 trigger 重复 ID 返回 409，先在触发前检查，避免生成孤立执行。
- `mcp` 不额外启动入口，仍由 MCP `workflow.execute` 工具调用。

## 3. 调度语义

### 3.1 状态

发布成功时把触发器快照写入租户库 `trigger_state`（`workflow_id + trigger_id` 主键），记录 `kind`、`config_json`、`workflow_version`、`enabled`、`next_fire_at`。重新发布时 `last_fire_at` / `last_execution_id` 保留，`next_fire_at` 按当前时刻重算。

调度器每秒 tick 一次（`HPA_SCHEDULER_TICK_MS` 可调，测试用 200ms），遍历所有 active 租户，取 `next_fire_at <= now` 的行。

### 3.2 单飞与幂等

推进 slot 用乐观锁：

```sql
UPDATE trigger_state SET next_fire_at = ?, last_fire_at = ?
WHERE workflow_id = ? AND trigger_id = ? AND enabled = 1 AND next_fire_at = ?
```

只有 `rows_affected = 1` 的 tick 才会启动执行，因此多实例、或多 tick 重叠都不会重复跑同一 slot。

### 3.3 漏跑策略

| 策略 | 行为 | 适用 |
| --- | --- | --- |
| `skip`（默认） | 迟到 ≤ 60s 正常触发；更晚则**不补**，直接把 `next_fire_at` 推到未来 | 高频、不能堆积的任务 |
| `run_once` | 无论迟到多久，先触发一次，再把时间推到未来 | 每日报表这类“补一次就够” |
| `catch_up` | 逐个补齐错过的 slot，最多 `maxCatchUp` 次；触顶后把剩余 slot 一次性推过 `now` | 计费、对账这类不能漏 |

`catch_up` 触顶后的“推进”很关键：否则每次 tick 都会继续补，等于忽略上限。

## 4. Webhook 投递

### 4.1 签名

```
X-HPA-Timestamp: 1793581200
X-HPA-Signature: sha256=<hex hmac-sha256(secret, "{timestamp}.{raw body}")>
X-HPA-Event-Id: release-1            # 可选，用于幂等
```

- 时间戳必须在 `now ± 300s` 内，超出返回 401 `HOOK_TIMESTAMP_STALE`。
- 签名常量时间比较；格式必须是 64 位十六进制（可带 `sha256=` 前缀）。
- 带 `X-HPA-Event-Id` 时按 `(trigger_id, event_id)` 唯一索引去重，重复投递返回 409 `HOOK_DUPLICATE`。
- 请求体上限 1 MiB；超过返回 413。

签发侧示例：

```bash
TS=$(date +%s)
BODY='{"action":"published","tag":"v1.2.0"}'
SIG=$(printf '%s.%s' "$TS" "$BODY" | openssl dgst -sha256 -hmac "$SECRET" -r | cut -d' ' -f1)
curl -sS -X POST "$BASE/hooks/wf_demo/github-release" \
  -H "content-type: application/json" \
  -H "x-hpa-timestamp: $TS" \
  -H "x-hpa-signature: sha256=$SIG" \
  -d "$BODY"
```

### 4.2 输入映射

- 请求体是 JSON 对象 → 直接作为执行输入。
- 否则（表单、纯文本） → `{"body": "<原始文本>", "contentType": "<header>"}`。

### 4.3 限流与审计

进程内 per-trigger 滑动窗口，默认 120 次/分钟（`HPA_HOOK_RATE_LIMIT`），超限返回 429 + `Retry-After`。每次成功投递都写一行 `trigger_fires`（`kind`、`fired_at`、`execution_id`、`event_id`、`origin`）。

## 5. 数据表

| 表 | 说明 |
| --- | --- |
| `trigger_state` | 发布快照 + 运行状态（`next_fire_at`、`last_fire_at`、`last_execution_id`、`last_status`） |
| `trigger_fires` | 触发历史；(trigger_id, event_id) 唯一索引做幂等 |

启动时随租户库一起迁移；没有单独的迁移脚本，靠 `CREATE TABLE IF NOT EXISTS`。

## 6. 接口

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/api/triggers?workflowId=` | 触发器列表（含 cron/时区/misfire、webhook URL、下次/上次触发） |
| POST | `/api/triggers/{workflow}/{trigger}/enable` | 启用；schedule 会重算 `next_fire_at` |
| POST | `/api/triggers/{workflow}/{trigger}/disable` | 停用 |
| POST | `/api/triggers/{workflow}/{trigger}/fire` | 手动触发一次（body 可选 `{input}`） |
| GET | `/api/trigger-fires?limit=` | 最近触发记录 |
| POST | `/hooks/{workflow}/{trigger}` | 入站投递（HMAC） |
| POST | `/hooks/{tenant}/{workflow}/{trigger}` | 指定租户的入站投递 |

`/api/*` 走常规鉴权（会话 + CSRF 或 Bearer）；`/hooks/*` 不走会话，只认签名。

## 7. 界面

侧边栏「実行管理 → トリガー」（`/triggers`）：

- 卡片：触发器、所属工作流、类型徽标、启停状态、cron + 时区 + 漏跑策略、Webhook URL（可复制）+ 密钥状态、下次/上次触发、跳到执行详情。
- 右栏：最近 30 次触发（类型、工作流 › 触发器、执行 ID、时间）。
- 每 15 秒自动刷新；启用/停用与手动触发即时反馈。

## 8. 验收

```bash
cargo test -p hpa-core --lib                                  # cron 与配置契约（10 个）
cargo test -p hpa-local --bin hpa -- scheduler::              # 调度与部分 fan-out 语义
cargo test -p hpa-local --test triggers                       # HTTP 端到端（2 个）
HPA_BASE_URL=http://127.0.0.1:8787 node scripts/phase11-triggers-e2e.mjs
```

覆盖点：

- 发布后 `trigger_state` 生成 3 行（schedule / webhook / event），三个触发器默认 enabled；`next_fire_at` 落在 `09:00 +08:00` 边界。
- 到期 slot 触发一次并推进 `next_fire_at`；同一时刻重复 tick 不重复触发。
- `skip` 丢弃迟到 10 分钟的 slot；`catch_up` 按 `maxCatchUp` 补齐且触顶后跳过剩余。
- Webhook：正确签名 202 并生成执行；重复 `event_id` 409；错误签名 / 过期时间戳 / 缺签名 401；未知触发器 404；停用后 409。
- Event：topic 发布 202 并生成执行；重复 `event_id` 409；停用后没有订阅者并返回 404。
- Event 部分成功：两个订阅者中预投递一个后重试同一 `event_id`，只为第二个创建执行；再次重试才返回 duplicate。
- PostgreSQL execution、start event 和 trigger fire 原子提交；重复 Event ID 在 fire 唯一约束处回滚整次 execution，不产生“有执行、无触发记录”。
- 手动触发 200；`/api/trigger-fires` 记录每次投递。
- 非法 cron、重复 trigger id 在网络层被 422 拒绝，错误详情含 `TRIGGER_INVALID` / `TRIGGER_ID_DUPLICATE`。

浏览器验收：创建带 3 个触发器的工作流 → 发布 → Webhook 投递 → 手动触发 → `/triggers` 页面（1440/607 截图，无横向溢出）→ 停用/启用切换 → 从界面触发并看到 toast 与新记录。

## 9. 明确不做

- 默认 Event 仍是进程内同步 fan-out + SQLite 幂等；`HPA_POSTGRES_RUNTIME=1 HPA_POSTGRES_EVENT_BUS=1` 时改为 PostgreSQL outbox 异步消费，支持崩溃重投与共享 trigger fire 去重。
- Event Bus 当前只覆盖 Topic Event；webhook、schedule 和 MCP task 尚未统一到同一 outbox 账本，也没有 Redis/Kafka 低延迟广播。
- `HPA_POSTGRES_SCHEDULER=1` 提供 PostgreSQL leader lease、续租和强杀接管；`HPA_POSTGRES_RUNTIME=1` 时 Workflow、trigger_state、schedule claim、trigger_fires 与 execution 均写 PostgreSQL，Event ID 去重可供多节点共享。`HPA_POSTGRES_EVENT_BUS=1` 再把 Topic Event 切到持久 outbox consumer；仍没有 Redis/Kafka 低延迟广播。
- 时区只支持固定偏移，没有 DST 规则；跨 DST 的每日任务会偏移一小时。
- 没有退避/抖动、并发上限、触发器级配额；限流只覆盖 Webhook 进程内滑窗。
- Webhook 不支持 mTLS、IP 白名单、请求体 schema 校验或响应回写。
- 调度器不派发到远程 Worker：Hybrid Worker 目前只接受显式 `create_worker_job`。
