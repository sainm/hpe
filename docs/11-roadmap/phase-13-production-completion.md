# Phase 13：生产级补全

> 状态：**IN_PROGRESS**（2026-11-02）。本阶段关闭 Phase 1～12 本地 MVP 之外的生产级缺口。
>
> 完成判据：代码、迁移、自动化测试、可重复 E2E、部署/运维文档和证据同时存在。外部账号或基础设施缺失时标记 `BLOCKED_EXTERNAL`，不得用 mock、截图或本地单节点结果替代生产验收。

## 1. 目标

把 Auralis/HPA 从“本地 MVP + 单控制面 Hybrid Worker”推进到可部署、可水平扩展、可接入真实企业基础设施和外部 AI 客户端的生产候选版本。

Phase 13 必须保持以下不变量：

- PostgreSQL 是 Canonical 控制面事实源，租户隔离由数据库策略和应用校验共同执行。
- Workflow/Agent/Skill/MCP 仍使用单一能力与执行契约，不形成云端第二套语义。
- 至少一次语义不变；不得宣称外部副作用 Exactly Once。
- 所有高风险执行、审批、租户切换和凭证使用都必须有负向测试。
- 真实外部集成失败或缺少凭证时保持 BLOCKED，不降级为本地 PASS。

## 2. 工作包

### P13-01 PostgreSQL 控制面与租户 RLS

> 进度：**IN_PROGRESS**。PostgreSQL 18 schema、SQLx checksum migration、七个 NOLOGIN 角色、66 张租户表的 FORCE RLS、checksum mismatch、双租户隔离、幂等 SQLite 导入和临时库 dump/restore 校验已建立；`HPA_POSTGRES_RUNTIME=1` 时 Workflow/Execution/Trigger Repository、execution lease、remote marker 与 **Worker 注册/心跳/停用**均已切到 PostgreSQL（`/api/enterprise/workers` 不再写 SQLite 镜像，见下方 `postgres_worker_registry` 证据）。Enterprise 的**平台令牌与审计事件**同样已切到 PostgreSQL：`hpa enterprise init` 在配置数据库时直接写 `hpa_control.tokens`，`/api/enterprise/tokens` 的创建/列表/吊销与 Bearer 校验全部走 PostgreSQL，`hpa_control.audit` 承接审计写入（`postgres_token_store` 证明丢失整个 data_dir 后同一管理员令牌仍可用、SQLite 令牌表保持 0 行）。租户/成员/配额/SCIM/浏览器会话仍以 SQLite 为权威；这三张表已建立**镜像 + 写穿**（启动全量 upsert、建租户写 PG 配额、成员增删改写穿、配额读取已在 PG 模式走 PostgreSQL），翻转权威的分步计划与风险见 [企业控制面迁移](../08-platform/postgres-enterprise-migration.md)。运维契约见 [PostgreSQL 控制面基线](../08-platform/postgres-control-plane.md)。

交付：

- PostgreSQL schema、版本化 migration、SQLite→PostgreSQL 导入工具和回滚策略。
- 平台元数据、工作流、执行、事件、Artifact、Agent、Skill、Plugin、Connector、MCP、OAuth、Worker 数据迁移。
- 每张租户表的 `tenant_id`、外键、索引、唯一约束和 PostgreSQL RLS policy。
- 应用角色、迁移角色、只读审计角色和运维角色分离。
- 双租户并发、跨租户 ID 探测、连接池复用、事务失败与备份恢复测试。

退出标准：

- 真实 PostgreSQL 18 集群上完成 migration/upgrade/rollback。
- 跨租户读取、写入、ID 猜测和连接复用测试结果为 0 泄漏。
- 不允许只测试 SQLite 旁路；所有生产路由必须走 PostgreSQL。

### P13-02 分布式调度、租约与 outbox

> 进度：**IN_PROGRESS**。PostgreSQL transactional outbox、tenant idempotency ledger、Topic Event outbox consumer、Scheduler fencing lease、实际 Scheduler leader 选举/续租/双进程强杀接管、execution owner lease/fenced state commit、Worker heartbeat/lease renewal/stale fencing/re-drive 已实现并通过并发、过期接管和死信测试；`HPA_POSTGRES_RUNTIME=1` 时 trigger facts 与 **Worker Job API（注册、心跳、租约、完成、取消、停用）**均走 PostgreSQL，`postgres_worker_registry` 断言 PostgreSQL 模式下 SQLite 注册表与任务表保持 0 行。真实 Worker daemon 已在 PostgreSQL 控制面上完成租约→执行→写回（见下方证据）；多控制面滚动升级与跨节点 Worker 租约演练仍未验证。契约见 [分布式运行时原语](../08-platform/postgres-distributed-runtime.md)。

交付：

- PostgreSQL transactional outbox + Redis Streams 或等价可靠队列；至少一个队列适配器通过故障注入。
- Scheduler leadership/lease、跨节点时间同步边界、重复 tick 和节点故障接管。
- Workflow Job、Trigger fire、Agent tool side effect 的稳定幂等键。
- Worker registration heartbeat、lease renewal、stale lease fencing、queue re-drive。
- 多控制面实例和至少两个 Worker 的无停机滚动升级测试。

退出标准：

- 强杀任一控制面或 Worker 后，任务不丢失且旧 lease 不能提交。
- 并发重复投递只产生一次内部执行记录；外部副作用仍以业务幂等键防重。
- 队列、outbox、数据库之间不存在无法解释的永久悬挂记录。

### P13-03 分布式 MCP 与事件总线

> 进度：**IN_PROGRESS**。PostgreSQL session/subscription/notification/task 状态、tenant/token/TTL 隔离、sequence 恢复和 task 终态约束已通过集成测试；`HPA_POSTGRES_MCP=1` 时 legacy session、subscription、目录通知、`tasks/get/update` 与 owner-node 命令消费者已切换。双进程测试验证 node-a 强停后由 node-b 提交取消、node-a 重启恢复并触发跨节点 task SSE。持久通知 backlog 使用租户内有界统计，超过 `HPA_MCP_NOTIFICATION_MAX_LAG` 主动断开并通过 `Last-Event-ID` 在保留窗口内重放。Topic Event 已使用 PostgreSQL outbox 异步消费和共享 trigger fire 去重；webhook/schedule/MCP task 尚未统一到同一事件账本。官方 conformance 2025-11-25 核心矩阵 9/9 通过；官方包尚无 2026-07-28 server 场景，真实客户端仍未完成。契约见 [MCP 持久状态](../06-mcp/persistent-state.md)。

交付：

- MCP session/subscription/task 从进程内状态迁到 Redis/PostgreSQL。
- `subscriptions/listen` 跨节点广播、断线语义和背压；保持 2026-07-28 无会话主路径。
- `tasks/get/update` 的持久任务状态、取消、审批输入和跨节点恢复。
- Topic Event Bus、webhook、schedule 与 MCP task 的统一幂等记录。
- MCP conformance suite、协议版本矩阵和多节点 SSE 集成测试。

退出标准：

- 任一节点断开后，通知和任务状态不丢失或按协议明确失败。
- 多节点同时消费不会重复产生同一 event/task 的最终执行。
- 官方 conformance 和至少两个真实 MCP 客户端完成工具调用。

### P13-04 可选 Python Agent Worker 与模型网关

> 进度：**IN_PROGRESS**。Explicit `python` planner、Python 3.13 + LangGraph 独立进程、Rust ToolService、会话能力、工具幂等账本、常量时间认证、预算、审批/恢复、跨请求取消、控制面强停后 continue、SQLite/租户隔离 PostgreSQL checkpoint、只读 rootfs/cgroup/无网络 Podman 演练、预算继承 sub-agent handoff 和 LiteLLM OpenAI-compatible 受控调用已通过测试；模型伪造低风险被 Rust 元数据覆盖，ToolService 拒绝错误 token、未声明工具、租户错配和风险降级。真实云模型、受控模型网络/容器 mTLS 和多副本 session 路由尚未完成。契约见 [Python Worker](../04-agent/python-worker.md)。

交付：

- Python 3.13 `ai-worker`：LangGraph、Pydantic、gRPC/JSON 契约和 checkpoint。
- LiteLLM 独立模型网关配置、预算、provider 路由、fallback 和 usage 回传。
- Rust Core → Python Worker 的认证、取消、审批、resume、超时和进程隔离。
- deterministic planner 与 Python planner 的同一 AgentVersion/Session 契约及本地/集群一致性测试。

退出标准：

- 无 Python/LiteLLM 时确定性 Workflow/Skill 不回归。
- Python Worker 崩溃后可从 checkpoint 恢复，预算与审批不可绕过。
- 真实或受控 OpenAI-compatible 网关完成多工具回合、usage/成本记录和失败回退。

### P13-05 完整 Connector 与远程 Marketplace

> 进度：**IN_PROGRESS**。PostgreSQL、只读 Git、Redis MQ、S3-compatible Connector 已接入统一边界并通过真实服务测试；签名 HTTP/离线 Marketplace index、发布者账号、发布者/索引密钥轮换与吊销、远程安装和撤回传播、source/publisher/key Web 与 VS Code 管理面、在线 OSV/离线漏洞库安装门禁、cosign Sigstore bundle + OCI digest provenance、持久 SSE 撤回推送已完成。尚未完成 Fulcio/Rekor keyless 策略、OCI Registry attestation 拉取、恶意软件情报与跨控制面 federation。见 [Connector 生产扩展](../08-platform/connector-implementation.md) 与 [Remote Marketplace](../07-plugin/remote-marketplace.md)。

交付：

- DB、Git、MQ、S3 Connector，统一认证、幂等、限流、超时、错误分类和审计。
- Connector SDK、插件 Kotlin/Java/TS handler 隔离策略（如选择外部进程/WASM）。
- 远程 Marketplace 索引、发布者账号、Ed25519/公钥轮换、审核/撤回、下载校验和可离线镜像。
- package-level SBOM、漏洞源、签名/来源证明和撤回传播。

退出标准：

- 每个 Connector 有真实测试服务/容器和安全负向测试。
- 发布→审核→远程下载→签名校验→安装→撤回全链路可重复。
- 未签名、未知发布者、篡改包、撤回版本和漏洞阻断均 fail-closed。

### P13-06 企业认证、HA 与运维

> 进度：**IN_PROGRESS**。OAuth 高级协议边界已通过 [ADR-0021](../adr/0021-oauth-protocol-boundary.md) 明确并把 PAR/JAR/DPoP/JARM 请求 fail-closed；企业 mTLS/PKI/Secret Manager/生产 S3 部署边界由 [ADR-0022](../adr/0022-enterprise-transport-and-secrets.md) 固化；tenant suspend/resume 已强制普通 token/session 失效；Browser SSO 已支持 AES-256-GCM 加密 token、refresh 轮换、静默续期和带一次性 state 的 RP-initiated logout；SCIM 2.0 Users/Groups、filter、PATCH、ETag 与固定角色映射已在本地通过 HTTP/E2E。多 IdP、真实企业 SCIM、HA/read replica 演练和外部 Vault/CA 环境仍未完成。

交付：

- PostgreSQL HA/read replica、连接重试、迁移锁、控制库在线备份与恢复。
- OIDC SSO refresh-token/silent renew/RP-initiated logout；SCIM/组和多 IdP 中至少确定一个完整实现。
- OAuth 的 PAR/CIMD/DPoP/JARM/RS256 或明确产品级取舍 ADR。
- mTLS Worker、证书轮换、Vault/Secret Manager 凭证注入、对象存储产物。
- SLO、容量规划、租户暂停、配额计量和告警手册。

退出标准：

- 节点故障、数据库切换、证书轮换和备份恢复均有演练证据。
- 认证/租户/RBAC 负向矩阵通过；没有本地匿名模式误入生产；SCIM 仍需以真实企业目录完成外部验收，多 IdP 仍未实现。
- 计费如启用，必须有账单对账与幂等测试；否则明确关闭。

### P13-07 OTLP 与跨进程追踪

> 进度：**IN_PROGRESS**。Rust OTLP HTTP/protobuf traces/metrics/logs exporter、Python Worker traces/metrics/logs、W3C traceparent、API→Runtime→HTTP client→MCP→Python Worker span 传播、有界队列、Collector 故障隔离、Prometheus 低基数约束、能力有界的 error/latency/probabilistic tail sampling、三信号敏感属性删除、配置门禁和 Collector/Tempo/Loki/Grafana/告警示例已实现并通过本地 OTLP receiver 测试；真实 HA 部署、traceID 一致路由和容量压测仍未完成。契约见 [OTLP 与跨进程追踪](../08-platform/otlp.md)。

交付：

- OTLP trace/metrics/logs 导出、采样、resource attributes 和 trace context 传播。
- API→Runtime→Worker→MCP→Agent Python Worker 的跨进程 span。
- tenant/job/execution/tool 的低基数关联与非高基数 Prometheus 标签。
- Collector/Grafana/Tempo/Loki 部署示例与 SLO dashboard。

退出标准：

- 一条执行可在各进程间按 traceId 串联。
- Collector 停机时应用可用且导出有界，不无限占用内存。
- 指标 cardinality、采样和脱敏策略通过审查。

### P13-08 真实外部客户端与发布验收

> 状态：**BLOCKED_EXTERNAL**（仅客户端与外部基础设施部分）。发布工程链路已就绪：`scripts/release-pack.mjs` 产出 `bin/`、`web/`、`migrations/postgres/`、`deploy/`、CycloneDX SBOM、`SHA256SUMS` 与带 git sha/工具链的 `manifest.json`；`scripts/release-verify.mjs` 逐文件校验并用临时 data dir 跑 `hpa doctor`。流程、回滚与已知边界见 [发布清单](../10-quality/release-checklist.md)。真实 Claude Code/Codex/Copilot 模型会话、双 Worker/双控制面、企业 IdP/CA/Vault/生产 Collector 仍未提供凭证或环境，复现 prompt 与权限清单见 [外部 AI 客户端发布验收](../10-quality/external-ai-client-acceptance.md)；本地 MCP Client/curl/mock 不计入外部模型 PASS。

交付：

- Claude Code、Codex、GitHub Copilot/Chat 的真实凭证环境 E2E。
- 外部客户端“发现工具→只读调用→高风险审批→重试→结果”证据。
- 本地、单控制面 Hybrid、PostgreSQL 多节点三种部署的同一 fixture 对照结果。
- Release SBOM、签名、checksums、迁移说明、回滚手册和已知边界。

退出标准：

- 至少两个真实外部 AI 客户端完成完整工具调用流程。
- 外部环境不可用时状态为 `BLOCKED_EXTERNAL`，提供复现命令和所需权限清单。
- 不得把 CLI 模拟、mock server 或浏览器测试计入客户端的模型调用 PASS。

## 3. 执行顺序

1. P13-01：冻结 PostgreSQL/RLS 与迁移契约。
2. P13-02：完成 outbox、队列、租约和 HA 基础。
3. P13-03：迁移 MCP/事件状态，跑协议矩阵。
4. P13-04：接入可选 Python Agent Worker/LiteLLM。
5. P13-05：补 Connector 和远程 Marketplace。
6. P13-06/P13-07：认证、运维、HA 与 OTLP。
7. P13-08：真实客户端和发布门禁。

每个工作包独立提交，不得把多个未验证能力合成一个“完成”提交。

## 4. 环境与阻塞

需要外部提供：

- PostgreSQL 18、Redis 或其他选定队列、S3/对象存储和至少两台控制面主机。
- 企业 IdP 测试租户、OAuth 客户端、SCIM 权限。
- Claude Code、Codex、GitHub Copilot/Chat 的测试账号或 CI 凭证。
- 允许访问模型网关的 API 凭证；不得把凭证写入 repo、日志或 Workflow。

缺少任一外部依赖时，相关工作包保持 `PLANNED` 或 `BLOCKED_EXTERNAL`，其余本地单元测试可以继续。

## 5. 当前环境证据（2026-11-02）

- PostgreSQL 18.4：19 个 SQLx migration 从空库应用成功，第二次 applied=0；66 张 FORCE RLS 租户表，备份恢复后 69 个业务关系行数与 checksum 一致。
- PostgreSQL tests：RLS、checksum mismatch、幂等导入、outbox/fencing、持久 MCP、HTTP SSE 路由全部通过。
- Scheduler leadership：两个独立 `hpa serve` 进程共享 PostgreSQL lease，启动后只有一个 `hpa_scheduler_leader=1`；强杀 leader 后 standby 在 2 秒 TTL 内接管，旧 leader 重启仍为 standby，未形成并发 leader。
- Connector：真实 PostgreSQL 参数化读写、Git、Redis pub/sub 与 MinIO SigV4 往返通过；负向测试覆盖跨租户/SQL 写误用/路径/通道/篡改。
- Remote Marketplace：HTTP + offline mirror、index/publisher key 轮换与吊销、包篡改、sequence rollback、withdrawal 全链路通过。
- Python：19 项测试、mypy、ruff、模型 fallback、SQLite/PostgreSQL checkpoint、并发初始化、显式 `python` planner 的 Rust→ToolService→Workflow 回合、审批风险覆盖、幂等重放、跨请求取消和控制面强停恢复通过；`pip-audit` 对当前锁定环境为 0 已知漏洞。
- Python sub-agent：受控模型回合调用 `agent.handoff`，Rust 验证声明/深度/handoff 预算后启动 deterministic 子 Agent，父会话记录 `childSessionId` 和聚合 usage；未声明 handoff 在 ToolService 负向测试中拒绝。
- OTLP：API/Runtime/MCP span 展示与 Rust→Python traceparent 继承通过；Collector 停机不阻断 ready，Prometheus labels 去除 tenant/execution 高基数。
- OTLP metrics/logs：本地 Collector 接收 Rust HTTP/workflow metrics 与 execution log；Python Worker 接收 session/tool metrics、tokens 和 finished log，所有导出均有界且不阻断 runtime。
- OTLP sampling/security：Collector 示例保留 error、慢于 5 秒和 10% 基线 trace，限制 `decision_wait`/`num_traces`/新 trace 速率；traces/metrics/logs 均删除已知 Authorization、Cookie、API key、模型 key、SQL statement 和 checkpoint 属性，`pnpm check:otel` 验证策略存在及有界。
- Browser SSO：AES-256-GCM token-at-rest、refresh 轮换/旧 Cookie 失效/上游拒绝撤销、Web silent renewal、RP-initiated logout 与一次性 state 通过 Rust HTTP 和 Playwright E2E。
- SCIM 2.0：Users/Groups CRUD、精确 eq filter、分页、PATCH、ETag、租户隔离、重复身份冲突、角色映射事务回滚与删除组恢复直接角色通过 Rust HTTP 和独立 E2E；真实企业 IdP 推送仍为 `BLOCKED_EXTERNAL`。
- PostgreSQL SCIM：`0010`～`0013` migration 已应用；当前 schema 共 66 张 FORCE RLS 租户表，备份恢复后 69 个业务关系、migration checksum 与 RLS 数量一致。
- PostgreSQL ExecutionRuntime：真实 `hpa serve` 在 `HPA_POSTGRES_RUNTIME=1` 下完成 workflow snapshot/registry version 物化、execution insert/update、event 追加和 output artifact；PostgreSQL 中可直接核对 execution/events/artifact。强停并重启后 API 仍从 PostgreSQL 读取 execution 与 events。相同 workflow/version 的 IR 内容变化返回 `WorkflowSnapshotConflict`，跨租户查询为 0 行；remote execution marker 同库保存，本地 recovery 不接管远程执行；每日/并发配额也按 PostgreSQL execution 计数。
- PostgreSQL WorkflowRepository：draft upsert、publish、current/list/version、workflow count 全部 PG；同版本变更失败关闭，新版本可追加，跨租户不可见。`hpa serve` E2E 证明发布版本不在 SQLite，API versions 与 Event Bus consumer 均从 PG 读取。
- Execution Lease：每 step 前续租，state/events 提交匹配 holder/token/未过期 lease。lease 过期后节点 B 递增 token 接管，节点 A 的 save/release 失败；空 data_dir 节点可从 PG running execution + workflow snapshot 恢复完成并释放 lease。
- 发布产物：`pnpm release:pack --profile debug --skip-build` 生成 `hpa-0.1.0-<sha>-darwin-arm64.tar.gz`（51 个文件、19 个迁移、12 个前端资源、2 份 CycloneDX SBOM），`pnpm release:verify` 校验压缩包 sha256、逐文件 `SHA256SUMS`，并在临时 data dir 上跑通 `hpa doctor`（status ok）。
- Hybrid Worker + PostgreSQL 端到端：真实 `hpa worker` daemon 在 `HPA_POSTGRES_RUNTIME=1` 下从 PostgreSQL 领取任务、上报 `start`、执行完成并写回 `hpa_tenant.executions`/`events`，SQLite `enterprise_worker_jobs` 保持 0 行（`postgres_worker_daemon`）。该测试暴露并修复了一个只在 PostgreSQL 出现的缺陷：`remote_executions` 外键要求执行行先存在，而 worker `start` 原先在插入执行行之前就写远端标记，导致守护进程一直 500 重试。
- PostgreSQL 企业控制面镜像：`HPA_POSTGRES_RUNTIME=1` 启动时把 `enterprise_tenants/quotas/members` 全量 upsert 进 `hpa_control`；建租户同步 PG 租户与默认配额，配额更新与成员增删改均写穿。`postgres_control_mirror` 断言删掉整个 data dir 后用空目录启动的节点仍从 PostgreSQL 读到同一配额（`maxWorkflows=7`、`maxExecutionsPerDay=11`），SCIM E2E 在 SQLite 模式回归通过。
- PostgreSQL tokens & audit：`HPA_POSTGRES_RUNTIME=1` 时管理员令牌与审计事件均以 PostgreSQL 为权威——`hpa enterprise init` 写入 `hpa_control.tokens`，`/api/enterprise/tokens` 创建/列表/吊销与 Bearer 认证读同一张表，审计写入 `hpa_control.audit`。`postgres_token_store` 删除整个 data_dir 后用空目录重启，同一令牌仍通过认证、吊销在另一节点立即生效、SQLite `enterprise_tokens` 保持 0 行；未知租户的审计行回退为 NULL tenant，避免外键阻塞请求。
- PostgreSQL 租户供给：`POST /api/enterprise/tenants` 在 PostgreSQL 模式下同步写入 `hpa_control.tenants`，避免新建租户注册 Worker 时触发 `workers_tenant_id_fkey`；`postgres_worker_registry` 每次运行使用独立租户并清理，重复执行结果一致。
- PostgreSQL Worker registry：`HPA_POSTGRES_RUNTIME=1` 下 `POST /api/enterprise/workers`、worker 心跳与停用不再写入 SQLite 镜像，`enterprise_workers`/`enterprise_worker_jobs` 保持 0 行；任务租约（`/api/worker/lease`）从 PostgreSQL 领取并只改 PostgreSQL 行。该测试还暴露并修复了一个真实缺陷：`hpa_control.workers.max_concurrency` 是 `INTEGER`，此前按 `i64` 解码导致 `list/heartbeat/disable/lease` 全部 500。
- PostgreSQL TriggerRuntime：event workflow 发布后 trigger_state 写入 PG；`/api/events/{topic}` 同步生成 PG trigger_fires 与 execution。重复 Event ID 返回 409，fire 行数保持 1。schedule due/claim、启停、republish 保留 last state 和跨租户隔离由独立 PG store 测试覆盖。
- PostgreSQL Topic Event Bus：`HPA_POSTGRES_EVENT_BUS=1` 下 API 只返回 202 queued，outbox 由 consumer 领取后标记 processed；同 payload duplicate、不同 payload conflict，PG trigger fire/execution 最终都只有 1 条。
- Event Bus 双控制面：发布节点设置 consumer=0，消费节点使用独立空 data_dir；消费节点从 PG workflow_versions 读取冻结 IR，执行 outbox 并产生唯一 PG trigger fire/execution。
- Event atomicity：execution、start event 与 trigger fire 在同一 PG 事务中提交；重复 trigger fire 的唯一约束会使第二次 execution 一并回滚，最终 execution/fire 各 1，消除“execution 已写入但 fire 未记录”的崩溃窗口。
- MCP 跨节点任务：`tasks/get/update` 通过 PostgreSQL；node-b 在 node-a 停机期间提交取消命令，node-a 使用相同 node ID 重启后消费命令、更新 execution 与 task，node-b SSE 收到 `notifications/tasks/status_changed`。
- MCP 慢消费者：真实 PostgreSQL/SSE 测试制造 25 条租户通知并设置 backlog 上限 20，超限连接主动结束；以倒数第 3 条为 `Last-Event-ID` 重连后，最后两条通知按 sequence 重放。租户内统计使用上限子查询，不扫描并物化完整积压。
- MCP 官方 conformance：`@modelcontextprotocol/conformance@0.1.16` 从 npmmirror 锁定安装；临时 PAT 认证代理下执行 2025-11-25 的 initialize、ping、logging、tools list/call/error、resources list、prompts list、DNS rebinding 共 9/9 PASS。测试工具仅在 `HPA_MCP_CONFORMANCE=1` 的临时 Runtime 出现，常规 `tools/list` 回归断言 `test_*` 不存在。
- Event 部分 fan-out：同一 topic 两个订阅者中预投递一个后，携带相同 Event ID 重试只为缺失订阅者创建执行；所有订阅者投递完成后再重试返回 duplicate，避免部分成功阻塞其余订阅者。
- Remote Marketplace UI：source/publisher/key 只读目录 API、Web 管理面、远程包安装入口通过 Playwright；1440/607 无横向溢出，axe serious/critical=0。VS Code Tree 增加 source/publisher/key/install/scan 管理。HPA 包支持在线 OSV/离线漏洞库、key-based cosign/OCI digest provenance 和持久 SSE 撤回推送；keyless Fulcio/Rekor、Registry attestation 拉取与跨控制面 federation 仍未完成。
- OSV：真实 `api.osv.dev` 查询 `jinja2@3.1.0`（`HPA_OSV_ECOSYSTEM=PyPI`）返回 10 个 advisory；未知 `plugin_online@1.0.0` 按 name-only 查询返回 clean。在线结果不代表 HPA 包安全结论，生产应配置正确 ecosystem 或使用带 `HPA` ecosystem 的离线快照。
- AI Worker 容器：Podman 5.8.5 rootless VM 构建 Python 3.13 镜像通过；UID 10001、只读 rootfs、无网络、512 MiB 内存、1 CPU、64 PID、drop ALL capabilities 和 no-new-privileges 演练通过，并完成 deterministic LangGraph 工具回合。真实模型网络/mTLS 仍未验证。
- 国内下载：Cargo 已实际从 `rsproxy.cn` 稀疏索引拉取锁定依赖；npm/pnpm、清华 PyPI、Playwright、Rustup 与 Go proxy 统一由 [国内下载源](../13-tech-stack/domestic-download-mirrors.md) 配置，锁文件与哈希校验保持开启。`pnpm check:mirrors` 校验 Containerfile、Compose 与 CI service 镜像使用 DaoCloud 镜像域名并固定 64 位 SHA-256 digest；`registry.npmmirror.com` 不支持 npm audit bulk advisory 端点，因此 npm audit 保留官方安全元数据例外。
- 供应链：`cargo deny` advisories/bans/licenses/sources、npm audit 通过；Rust/Python CycloneDX SBOM 可生成。

### 复核记录（2026-11-02，提交前）

本次对工作树做了一次完整复核，命令与结果如下（本地 PostgreSQL 18.4，临时库 `hpa_verify_*` + `HPA_POSTGRES_TEST_URL`）：

- `cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`：通过（提交前修复了 `postgres_worker.rs` 的类型推断、`worker.rs` 的借用与一处未使用导入）。
- `cargo test --workspace --no-fail-fast`：除 12 个依赖 seatbelt 的既有用例外全部通过（本沙箱 shell 无法执行 `sandbox-exec`，报 `sandbox_apply: Operation not permitted`，与本次改动无关）。
- PostgreSQL 套件 12/12 真实执行通过：`postgres_rls`、`postgres_import`、`postgres_distributed`、`postgres_event_bus`、`postgres_execution_store`、`postgres_workflow_store`、`postgres_trigger_store`、`postgres_runtime`、`postgres_execution_lease`、`postgres_execution_recovery`、`postgres_atomic_execution_fire`、`postgres_mcp`、`postgres_scheduler_leader`（leader 接管 3.6s）。
- `node scripts/run-local-e2e.mjs`：通过（含时间线与设置页 1440/607 截图、无横向溢出）。
- Web：11 Vitest、typecheck、build 通过；VS Code 扩展：63 Vitest、build 通过。
- Python：`uv run --frozen pytest` 17 passed / 2 skipped，叠加 PostgreSQL checkpoint 3 passed；`ruff check`、`mypy src` 干净。
- 仓库门禁：`check:brand`、`check:ir`、`check:contracts`、`check:docs`（164 文件 835 链接）、`check:otel`、`check:mirrors`、`check:supply-chain` 全部通过。

## 6. 完成定义

Phase 13 只有同时满足以下条件才可标记完成：

- P13-01～P13-08 的退出标准全部有当前环境证据。
- 所有生产路径不再依赖 SQLite-only 或进程内-only 状态。
- 全量 Rust/Web/Extension/Protocol/E2E/安全/迁移/HA 回归通过。
- 外部客户端调用使用真实模型会话完成。
- README、路线图、部署手册、验收标准与代码计数一致。
