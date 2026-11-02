# 实现状态与验收边界

> 核对日期：2026-11-02。本文是当前代码、README 与路线图之间唯一的完成度口径。

## 1. 结论

Phase 1～12 的**本地 MVP 子集**已经实现并通过仓库内自动化；这不等于原路线图和 `acceptance-criteria.md` 中全部 SaaS、云端、企业级退出标准。

- “本地实现完成”表示代码、CLI/API/UI、负向测试和当前机器可重复的命令均存在。
- “后续目标”表示没有实现，或只在外部账号、真实云环境、多节点部署中才能成立；不得计入本地 PASS。
- 本机没有 Claude/Copilot 模型会话授权，因此外部模型主动调用 MCP 工具仍需人工凭证环境验证。

## 2. 分阶段状态

| Phase | 当前可验证范围 | 仍未完成或未证明 |
| --- | --- | --- |
| 1 Core Workflow | IR、13 类节点、发布冻结、分支/循环/并行/审批/子流程/错误图；YAML SourceMap；强杀进程恢复；OpenAPI 与 JSON Schema 契约；键盘/axe 基础检查 | 完整跨浏览器矩阵、全量属性测试、所有节点组合的 UI E2E |
| 2 Skill | 导入/测试/运行/发布/导出、三运行时沙箱、源码编辑生成新版本、本地 Ed25519 签名与篡改拒绝 | Claude Code 真实模型会话加载、在线依赖安装、容器级便携沙箱 |
| 3 MCP | modern/legacy、MRTR、PAT/OAuth、鉴权 GET SSE、`subscriptions/listen`、企业租户路由、schema/artifact 资源；PostgreSQL 持久 session/subscription/notification/task、跨节点 task update 命令与双进程强停 SSE 恢复、有界慢消费者背压与 sequence 重放；官方 2025-11-25 核心 conformance 9/9 | 真实外部模型工具调用、官方 2026-07-28 server suite、统一事件总线、跨节点低延迟通知传输 |
| 4 Agent | Rust deterministic/OpenAI 兼容 planner；显式 Python 3.13 LangGraph planner、预算、审批/恢复、跨请求取消、控制面强停恢复、SQLite 与租户隔离 PostgreSQL checkpoint、实际 ToolService 与工具幂等账本、预算继承 sub-agent handoff、只读 rootfs/cgroup/无网络容器演练、LiteLLM 受控端到端 | 真实云模型、受控模型网络/容器 mTLS 与多副本 session 路由 |
| 5 Plugin | 声明式插件、Ed25519、安装/升级/回滚；HTTP/File/PostgreSQL/Git/Redis MQ/S3 Connector；签名远程/离线 Marketplace、发布者/索引 key 轮换与撤回、Remote Marketplace Web/VS Code UI、持久 SSE 撤回推送；SBOM/许可/静态扫描、在线 OSV/离线漏洞库门禁、key-based cosign bundle + OCI digest provenance | 可执行 TS handler、恶意软件情报、Fulcio/Rekor keyless、OCI Registry attestation 拉取、跨控制面 federation |
| 6 Enterprise | 单节点 SQLite 多租户、8 角色、配额、审计、备份、成员目录、SCIM Users/Groups、单节点 SSO；PostgreSQL 18 schema、SQLx migration、角色分离、66 表 FORCE RLS、幂等 SQLite 导入；`HPA_POSTGRES_RUNTIME=1` 的 Workflow/Execution/Trigger Repository、execution lease、重启读取与 topic 幂等 | Enterprise/Worker Job/Agent 等其余 API 切换、双写核对、HA/多节点、控制库在线备份、SaaS 计费、完整合规评审 |
| 7 VS Code | 扩展、10 个 Tree View、Runtime/MCP/Skill 集成、7 个 Host 检查 | Copilot 模型主动 MCP 工具调用、跨 VS Code 版本矩阵、Remote 工作区高级场景 |
| 8 Hybrid Worker | 远程执行、租约、审批恢复、取消、本地持久 outbox、远程审批 SLA；PostgreSQL outbox、幂等账本、Scheduler/Worker fencing lease、实际 Scheduler 双进程 leader election/failover 与 re-drive 原语 | 触发器/Worker 生产路由切换、双 Worker 强杀与滚动升级、mTLS、对象存储、远程 Agent/Plugin、自动扩缩容 |
| 9 Browser SSO | OIDC Code+PKCE、三种 ID Token 算法、服务端 session/CSRF、成员目录、加密 refresh token、会话轮换、silent renew、RP-initiated logout；SCIM 2.0 Users/Groups 与组角色映射 | 多 IdP、真实企业 IdP/SCIM 验收 |
| 10 OAuth AS | Code+PKCE、DCR、同意、refresh 轮换/复用检测、吊销/内省/JWKS | ID Token、CIMD、PAR、JARM、DPoP、RS256、跨节点吊销同步 |
| 11 Triggers | cron、webhook、topic event、幂等、固定偏移时区、历史；PostgreSQL trigger state/fire 与可选 Topic Event outbox consumer | IANA/DST、webhook/schedule/task 统一 outbox、mTLS/IP 白名单、直接远程 Worker 派发 |
| 12 Observability | Prometheus、ready、执行时间线、A11y/Playwright；Rust/Python OTLP traces/metrics/logs、W3C context、有界队列、Collector 故障隔离、error/latency/probabilistic tail sampling、敏感属性删除和 Tempo/Loki/Grafana 示例 | 真实 HA Collector 压测与 traceID 一致路由、持久化指标和可配置直方图桶 |

## 3. 新增的本地实现

- Hybrid Worker 的 `start/events/complete` 上报先写 `<data_dir>/worker-outbox.db`，重启后排空；成功响应后确认删除，旧 lease 清理。
- 控制面后台扫描远程 `waiting_approval`，按 SLA 调用 `expire_approval` 并重新入队。
- MCP legacy GET SSE、modern `subscriptions/listen`、目录变更通知、`tasks/get` 与等待审批任务取消；legacy session 绑定 token/tenant，禁止跨 token 复用。
- MCP PAT/OAuth token 从 token 选择租户，资源 URI 与工具目录按租户隔离。
- Workflow YAML 生成 UTF-8 byte offset/line/column SourceMap，并通过 validation API 返回。
- `/api/openapi.json` 暴露本地 API OpenAPI 3.1 契约；仓库增加 Node Registry、事件与 Tool capability JSON Schema。
- Skill Center 支持文本源码编辑并强制生成新版本；Skill 支持本地 Ed25519 签名、验证和篡改阻止发布。
- `event` trigger 支持 topic 订阅与 `/api/events/{topic}` 幂等发布。
- 前端补齐严重/关键 axe 规则和首 Tab 焦点检查；浏览器脚本改用项目依赖，不再引用其他仓库的绝对路径。
- 根目录提交 `pnpm-lock.yaml`，CI 执行 fmt/clippy/test、Web/扩展、Playwright E2E、cargo-deny、SBOM/许可扫描和 npm audit。
- PostgreSQL 18 迁移由 SQLx checksum/lock 管理；七个 NOLOGIN 角色分离迁移、租户运行时、Worker、Scheduler、平台管理、运维和只读审计权限。集成测试验证 66 张租户表的 FORCE RLS、双租户隔离、默认拒绝、连接复用、只读限制和 checksum mismatch。
- SCIM 2.0 提供 ServiceProviderConfig、ResourceTypes、Schemas、Users/Groups CRUD、精确 `eq` filter、分页、PATCH、弱 ETag 和事务化固定角色映射；角色冲突、重复 userName、跨租户 ID 与 If-Match 失配均有 HTTP 负向测试。
- MCP 持久 task 使用稳定 `HPA_NODE_ID` 绑定执行 owner；跨节点取消命令持久化并按 owner/lease 消费。双进程测试强停 owner 后由另一节点提交命令，owner 重启完成本地 execution 取消和 PostgreSQL task 投影更新。
- Remote Marketplace Web UI 通过只读 source/publisher 目录 API 展示签名 key、index sequence、刷新状态、远程包和安装入口；Playwright 在 1440/607 验证 source/publisher/key 可见、无横向溢出和 serious/critical axe=0。
- `hpa db import-sqlite` 以只读方式导入控制库和租户库，覆盖 JSON、Blob、时间戳与复合外键；业务目标租户由源目录强制注入，重复执行不新增行。短生命周期 OIDC/OAuth 状态不复用，升级要求重新登录。
- PostgreSQL backup rehearsal 使用真实 `pg_dump`/`pg_restore` 恢复到临时库，并比较 migration checksum、69 个业务关系行数和 66 张 FORCE RLS 表。
- PostgreSQL transactional outbox、tenant 幂等账本、Scheduler leader lease 和 Worker Job fencing 已实现；测试验证并发消费者仅一方领取、过期接管递增 token、旧 token 提交失败、重试进入 dead、幂等 hash 冲突拒绝和 job re-drive。
- MCP 的 PostgreSQL session/subscription/notification/task 状态实现 tenant + token 绑定、TTL、sequence 恢复和终态约束；`HPA_POSTGRES_MCP=1` 时 session、subscription、目录/task SSE 和 owner-node task update 消费者均已接入。
- 可选 Python Worker 使用 LangGraph；显式 `python` planner 通过认证 JSONL 启动/恢复/取消独立进程，工具调用回到 Rust ToolService。模型风险由 Rust 元数据覆盖，审批能力绑定调用哈希，工具账本拒绝重复 in-progress/参数变化；LiteLLM 通过本地受控 OpenAI-compatible 网关完成模型→Python→Rust→Workflow 端到端，控制面强停后 `continue` 可从 SQLite 或租户独立 PostgreSQL schema 恢复。
- Connector 增加真实 PostgreSQL 参数化只读查询、shell-free 本地 Git、Redis Lua 幂等发布与 S3 SigV4；负向测试覆盖 SQL 写语句误入 query、Git 路径注入/未授权 commit、Redis channel/重复键与 S3 key traversal。
- Remote Marketplace 使用独立 index key 验证签名索引，再绑定 publisher key 与包 Ed25519；支持 HTTPS/离线 mirror、序列防回滚、包哈希、发布者/索引 key 轮换与吊销、安装前复验和 withdrawal 自动停用。
- 插件安装在配置漏洞源后按 HPA ecosystem 查询 OSV 或本地 OSV 快照；离线库校验 schema、20 MiB 上限和最大年龄，任一 advisory 命中即 fail-closed，查询失败不回退为 clean。
- Remote Marketplace 可在 `HPA_REQUIRE_PROVENANCE=1` 下要求 key-based cosign Sigstore bundle；in-toto statement 同时绑定包 SHA-256 与 OCI manifest digest，refresh/install 双重验证且缓存篡改失败。
- VS Code Plugin Tree 提供 Remote Marketplace source、publisher、signing key、index key 轮换、远程安装和漏洞扫描操作；Web 与扩展共享同一 API。
- OTLP 导出使用官方 Rust/Python SDK：HTTP、Runtime、MCP、Worker span 继承 W3C traceId；BatchSpanProcessor 有界，Collector 停机不阻断 ready/API；Prometheus tenant 标签改为全局聚合。
- OTel metrics/logs bridge 同步导出 Rust Prometheus metrics、tracing events 及 Python worker session/token/tool logs；信号级 endpoint、queue/batch/interval 均有上限。
- OAuth 高级协议的明确产品边界已记录在 ADR-0021；PAR/JAR/response_mode=JWT/DPoP 请求被拒绝而非静默降级。企业 mTLS/PKI/Secret Manager/生产 S3 的使用边界和 BLOCKED_EXTERNAL 条件记录在 ADR-0022。

## 4. 可复现命令

```bash
npx -y pnpm@12.16.1 install --frozen-lockfile
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check licenses bans sources advisories
npx -y pnpm@12.16.1 --filter @hpa/web typecheck
npx -y pnpm@12.16.1 --filter @hpa/web test
npx -y pnpm@12.16.1 --filter @hpa/web build
cargo build -p hpa-local
HPA_POSTGRES_TEST_URL=postgresql://localhost:5432/hpa_p13_test cargo test -p hpa-local --test postgres_rls -- --nocapture
HPA_POSTGRES_TEST_URL=postgresql://localhost:5432/hpa_p13_test cargo test -p hpa-local --test postgres_import -- --nocapture
HPA_POSTGRES_TEST_URL=postgresql://localhost:5432/hpa_p13_test cargo test -p hpa-local --test postgres_distributed -- --nocapture
HPA_POSTGRES_TEST_URL=postgresql://localhost:5432/hpa_p13_test cargo test -p hpa-local --test postgres_execution_store -- --nocapture
HPA_POSTGRES_TEST_URL=postgresql://localhost:5432/hpa_p13_test cargo test -p hpa-local --test postgres_execution_lease -- --nocapture
HPA_POSTGRES_TEST_URL=postgresql://localhost:5432/hpa_p13_test cargo test -p hpa-local --test postgres_execution_recovery -- --nocapture
HPA_POSTGRES_TEST_URL=postgresql://localhost:5432/hpa_p13_test cargo test -p hpa-local --test postgres_runtime -- --nocapture
HPA_POSTGRES_TEST_URL=postgresql://localhost:5432/hpa_p13_test cargo test -p hpa-local --test postgres_mcp -- --nocapture
HPA_POSTGRES_TEST_URL=postgresql://localhost:5432/hpa_p13_test cargo test -p hpa-local --test mcp_http -- --nocapture
cd services/ai-worker && uv sync --frozen --default-index https://pypi.tuna.tsinghua.edu.cn/simple && uv run --frozen pytest -q && uv run --frozen mypy src && uv run --frozen ruff check . && uv run --frozen pip-audit --local --progress-spinner off
cd ../.. && HPA_AI_WORKER_BIN="$PWD/services/ai-worker/.venv/bin/hpa-ai-worker" cargo test -p hpa-local --test ai_worker_process -- --nocapture
HPA_POSTGRES_TEST_URL=postgresql://localhost:5432/hpa_p13_test cargo test -p hpa-local --test connectors_extended -- --nocapture
cargo test -p hpa-local --test marketplace_remote -- --nocapture
cargo test -p hpa-local --test otel -- --nocapture
HPA_POSTGRES_URL=postgresql://localhost:5432/hpa_p13 cargo run -p hpa-local -- db migrate
HPA_POSTGRES_URL=postgresql://localhost:5432/hpa_p13 cargo run -p hpa-local -- db status
HPA_POSTGRES_TEST_URL=postgresql://localhost:5432/hpa_p13_test node scripts/check-postgres-backup.mjs
npx -y pnpm@12.16.1 test:e2e
npx -y pnpm@12.16.1 --filter workbench test
npx -y pnpm@12.16.1 --dir apps/vscode-extension run test:host
npx -y pnpm@12.16.1 --dir apps/vscode-extension run package
npx -y pnpm@12.16.1 check
```

`cargo test` 和 `pnpm` 的具体计数以命令输出为准，不在文档中维护容易过期的硬编码总数。

2026-11-02 的 `cargo deny --offline check licenses bans sources advisories` 使用现有 RustSec 缓存通过；本轮实时更新无法访问 GitHub advisory source，因此最新公告状态仍需在可访问权威源的 CI 中复核，不能用许可证 PASS 替代。npm audit 使用官方 npm registry 的 security endpoint；Python 使用 `pip-audit`。

## 5. 发布判定

- 只有第 2 节“当前可验证范围”全部通过时，才能标记对应 Phase 的**本地 MVP PASS**。
- 任一“仍未完成或未证明”项不得被截图、设计文档或 mock 替代。
- 外部模型、真实企业 IdP、PostgreSQL RLS、HA 和多节点结果必须保存独立环境证据后，才能升级为完整 Phase PASS。

## 6. 下一个目标

Phase 13“生产级补全”是当前 active 目标，范围和退出标准见 [Phase 13 路线图](../11-roadmap/phase-13-production-completion.md)。整体状态为 `IN_PROGRESS`；P13-01 的 schema/RLS/checksum migration/SQLite 导入、PostgreSQL Workflow/Execution/Trigger Repository 与 execution owner lease、P13-02 的 outbox/fencing、Topic Event consumer 与双进程 Scheduler leader failover、P13-03 的持久 MCP 与慢消费者重放、Python Worker、四类新 Connector、Remote Marketplace Web UI、Browser SSO refresh/logout、SCIM Users/Groups 和 OTLP traces/metrics/logs/tail sampling/属性门禁已有当前环境证据，但 Enterprise/Worker Job 等其他生产 API 未切换 PostgreSQL，webhook/schedule/task 尚未统一到同一 outbox，多 IdP、真实企业 SCIM、HA/VS Code Remote UI/真实外部客户端仍未闭合，不提前标记 PASS。国内包镜像与容器 digest 有独立静态门禁，npm audit 的权威公告源是显式安全例外。
