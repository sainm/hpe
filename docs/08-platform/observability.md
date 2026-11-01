# 可观测性（Phase 12）

> 状态：本地实现完成（2026-11-01）。范围是**单进程指标 + 就绪探针 + 执行时间线**。
> 不做分布式追踪后端，也不内置 Prometheus/Grafana。

## 1. 问题

Phase 11 之后调度器在后台每秒跑一次，但外部完全看不见：不知道它是否还活着、上次 tick 多久、漏了多少 slot；执行失败只能一条条点开看；`/api/health` 永远返回 ok（它只证明进程还在）。

Phase 12 补三件事：

1. `/metrics`：Prometheus 文本格式，覆盖 HTTP、执行、节点、调度器、触发器。
2. `/api/ready`：真正检查依赖（控制库、租户库、签名密钥、调度器心跳）的就绪探针。
3. 执行详情页的**步骤时间线**：从事件还原每个节点的耗时与状态，最慢步骤高亮。

## 2. 指标

进程内注册表（`crates/hpa-local/src/metrics.rs`），互斥锁 + 原子值，无外部依赖。所有指标都是**单进程**语义：多节点部署时由抓取端相加，进程内不做聚合。

### 2.1 HTTP

| 指标 | 类型 | 标签 |
| --- | --- | --- |
| `hpa_http_requests_total` | counter | `route`（路由模板，如 `/api/workflows/{id}`）、`method`、`status` |
| `hpa_http_request_seconds` | histogram | `route` |

- `route` 用 axum 的 `MatchedPath`（模板而非实际路径），避免 ID 造成的基数爆炸。
- 未匹配任何路由的请求记为 `route="unmatched"`（不会把真实路径写进标签）。
- `/api/{*rest}` 是租户网关的兜底路由；真正的标签由租户内层路由补上（`/api` 前缀由网关打标），因此不会重复计数。

### 2.2 工作流执行

| 指标 | 类型 | 标签 |
| --- | --- | --- |
| `hpa_workflow_executions_total` | counter | `tenant`、`status`（`succeeded`/`failed`/`cancelled`） |
| `hpa_workflow_execution_seconds` | histogram | `status`（另有一条无标签的汇总） |
| `hpa_node_executions_total` | counter | `type`（节点类型）、`status`（`succeeded`/`failed`/`skipped`） |
| `hpa_node_seconds` | histogram | `type` |

- 只统计**驱动器跑完**的执行（`RuntimeManager::drive_inner` 进入终态时记录一次）。手工取消、租户删除等不经过驱动器的路径不计入，避免重复计数。
- 节点耗时是 `step()` 调用的墙钟时间；一个 step 里若同时完成多个节点，会为每个完成事件各记一次（并行分支场景），因此节点耗时之和可能大于执行总时长。

### 2.3 调度器与触发器

| 指标 | 类型 | 说明 |
| --- | --- | --- |
| `hpa_scheduler_ticks_total` | counter | tick 次数 |
| `hpa_scheduler_tick_seconds` | histogram | 单次 tick（遍历所有租户）耗时 |
| `hpa_scheduler_last_tick_timestamp_seconds` | gauge | 最近一次 tick 的 unix 时间 |
| `hpa_scheduler_fires_total` | counter | 调度器启动的执行数 |
| `hpa_trigger_fires_total` | counter | 标签 `kind`、`result`（`started`/`duplicate`） |

### 2.4 租户状态（抓取时现算）

| 指标 | 类型 | 标签 |
| --- | --- | --- |
| `hpa_workflows_total` | gauge | `tenant` |
| `hpa_executions_running` / `hpa_executions_waiting_approval` | gauge | `tenant` |
| `hpa_executions_succeeded_total` / `hpa_executions_failed_total` | gauge | `tenant` |
| `hpa_triggers_enabled` | gauge | `tenant`、`kind` |

这些值每次抓取时按租户查一次 `stats()` 与 `trigger_state` 计算；某个租户打不开只会跳过并记 warn，不会让抓取失败。

### 2.5 进程

`hpa_build_info{version}`、`hpa_process_uptime_seconds`。

## 3. 端点

| 方法 | 路径 | 鉴权 |
| --- | --- | --- |
| GET | `/metrics` | 本地模式开放；`HPA_ENTERPRISE_MODE` 下需要平台管理员 Bearer 或会话 |
| GET | `/api/ready`、`/ready` | 不需要鉴权（供探针使用） |
| GET | `/api/health` | 不需要鉴权，仅证明进程存活 |

`/metrics` 响应头为 `content-type: text/plain; version=0.0.4`、`cache-control: no-store`。

Prometheus 抓取示例：

```yaml
scrape_configs:
  - job_name: hpa
    scrape_interval: 15s
    authorization:
      credentials_file: /etc/hpa/metrics-token   # 仅企业模式需要
    static_configs:
      - targets: ["127.0.0.1:8787"]
```

### 就绪语义

`/api/ready` 返回 200 只有同时满足：

| 检查 | 含义 | 失败值 |
| --- | --- | --- |
| `controlDb` | `SELECT 1` 于 `control.db` | `error` |
| `tenantDb` | `SELECT 1` 于 `local` 租户库 | `error` |
| `signingKey` | OAuth JWKS 至少有一把未删除的键 | `missing` |
| `scheduler` | 最近 tick 在 15 秒内 | `stale` / `starting` |

不满足时返回 503，body 形如：

```json
{"ready":false,"checks":{"scheduler":"starting"},"schedulerTickAgeMs":null,"uptimeSeconds":2}
```

注意：`/api/ready` 只检查 `local` 租户库，不逐个探测其他租户（避免探针变成 N 次磁盘打开）。

## 4. 界面

- **执行详情 → 步骤时间线**：从 `node.started` / `node.succeeded|failed|skipped|waiting` 事件配对，按开始时间排序，条形宽度相对总时长；最慢节点用橙色标注；重试显示 `#attempt`；未闭合的 span 显示为 `running`。事件流仍然是事实来源，时间线只是它的可视化。
- **设置 → 运行时状态**：四项检查（控制库、租户库、签名密钥、调度器）各一张卡片，附运行时长、最近 tick 间隔与 `/metrics` 链接；15 秒自动刷新。

## 5. 验收

```bash
cargo test -p hpa-local --lib  # 指标渲染与标签转义（2 个单元测试）
cargo test -p hpa-local --test observability
HPA_BASE_URL=http://127.0.0.1:8787 node scripts/phase12-observability-e2e.mjs
```

覆盖：

- `/api/ready` 从 `starting` 变为 `ready`，四项检查均为 ok。
- 跑完一个工作流后 `/metrics` 含 `hpa_workflow_executions_total{status="succeeded",tenant="local"}`、节点直方图 `type="transform"`、`hpa_workflows_total{tenant="local"}`、调度器心跳与 `/api/executions/{id}` 路由标签。
- 企业模式下匿名抓取 `/metrics` 返回 401，带管理员令牌返回 200。
- 浏览器：执行详情时间线出现 3 个节点、最慢标记；设置页 4 项检查全部 ok；1440/607 均无横向溢出。

## 6. 明确不做

- 没有 OpenTelemetry / OTLP 导出，也没有 span 关联与采样；跨进程链路（如 HTTP 节点调用外部服务）不追踪。
- 指标只在内存里，进程重启归零；没有持久化、没有 `node_exporter` 式的进程外采集。
- 直方图桶固定 13 档（5ms–60s），不可配置；没有 exemplars。
- 多租户指标除 `tenant` 标签外没有配额；高基数标签（workflow id、execution id）一律不采集。
- `/metrics` 不做限流，也不区分租户视角（企业模式下只有“全有或全无”）。
- 执行时间线完全由前端从事件推导，服务端不存储聚合后的耗时；事件被裁剪或延迟到达时，时间线会随之变化。
