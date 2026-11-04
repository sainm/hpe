# Changelog

## Unreleased

### Changed

- Phase 25：2026-07-28 MCP MRTR conformance——新增 8 个 `InputRequiredResult` 工具 fixture 与 1 个 prompt fixture，覆盖 elicitation、sampling、roots/list、requestState 完整性、多输入、多轮、能力过滤、缺失/非法响应与 `prompts/get`；`pnpm test:mcp-conformance:2026-mrtr` 覆盖 14 个场景、40 项检查全部通过，2026 计分 server requirements 达到 37/37（tasks extension 不计分且未实现）。
- Phase 24：2026-07-28 MCP conformance 内容场景——工具图像/音频/嵌入资源/混合内容/进度、Resources/Prompts、completion、caching、JSON Schema 2020-12、标准与自定义 Header（base64 Mcp-Param）全部通过；`pnpm test:mcp-conformance:2026-content` 覆盖 18 个场景。
- Phase 23：官方 `@modelcontextprotocol/conformance` 升级到 `0.2.0-alpha.12`；新增 requirements 运行模式与 `pnpm test:mcp-conformance:2026`。2025-11-25 核心 9/9、2026-07-28 `server-stateless` 28/28 通过；补齐 per-request `_meta` 校验、-32020/-32022/-32021/-32601 错误与 HTTP 状态、移除方法 404、订阅 acknowledged 首帧与 subscriptionId 标记。
- Phase 22：Agent 驱动循环每轮、工具执行前、Python 工具服务与审批恢复前续租 Agent 会话租约；租约过期或被接管时新工具副作用不执行，已开始的单次工具调用不中断。
- Phase 21：`maxPlugins` 配额覆盖 `/plugins/sample` 与本地/远程 Marketplace 安装，`maxConnections` 覆盖 `/connectors/sample`；配额为 0 时这些入口统一 429，避免绕过 Phase 20 的计数修复。
- Phase 20：`HPA_POSTGRES_RUNTIME=1` 时 `maxPlugins` 与 `maxConnections` 配额改为按 `hpa_tenant.plugins` / `hpa_tenant.connections` 计数，修复此前仍查 SQLite 导致跨节点配额被绕过；本地 SQLite 模式不变。
- Phase 19：远程 Marketplace 全链路打通——未缓存包的节点在远程安装时自动刷新来源并重建本地缓存；签名索引、包哈希、发布者 key、插件签名、批准/撤回状态在刷新后重新校验；撤回刷新会停用已安装版本并阻止再次安装。包缓存仍为节点本地，元数据继续走 PostgreSQL。
- Phase 18：Agent 主动执行增加 PostgreSQL 会话 owner lease（`lease_owner/lease_token/lease_expires_at`，migration `0020`）；start/drive/approval resume 获取租约，persist/emit 按 holder+token+未过期围栏，终态或等待审批释放；`recover` 跳过仍被其他节点持有的 running 会话，只接管过期会话，旧 token 写入失败；本地 SQLite 模式不变。
- Phase 17：插件生命周期、本地/远程 Marketplace 与信任元数据（plugin_packages、plugins、plugin_capabilities、plugin_events、plugin_market_listings、plugin_reviews、publishers/sources/remote_packages、marketplace_event_stream）在 `HPA_POSTGRES_RUNTIME=1` 时以 PostgreSQL 为权威；插件贡献的 MCP server 与 Connector 分别走 Phase 15/16 存储，支持跨节点安装状态、启停、审核、评分、撤回与信任 key 管理；插件包文件、签名私钥与安装目录仍在节点本地，SQLite 单节点模式与插件 CLI/UI 保持不变。
- Phase 16：Connector 定义、连接与审计（connector_definitions、connections、connector_audit）在 `HPA_POSTGRES_RUNTIME=1` 时以 PostgreSQL 为权威；节点启动幂等注册内置连接器，支持跨节点创建/读取/更新/测试/删除连接与审计，插件启停/卸载的 Connector 贡献走同一路径，SQLite 单节点模式与执行引擎保持不变。
- Phase 15：MCP 控制面元数据（mcp_tokens、mcp_approvals、mcp_audit、mcp_servers）在 `HPA_POSTGRES_RUNTIME=1` 时以 PostgreSQL 为权威；支持跨节点 PAT 创建/查找/吊销、审批决定/一次性消费/过期、审计写入与外部服务器管理，SQLite 单节点模式与 MCP Gateway/Center 行为保持不变；持久 MCP session/task 仍由 `HPA_POSTGRES_MCP=1` 独立覆盖。
- Phase 14：Skill 包与版本元数据（skills、skill_versions、skill_files、skill_runs、skill_run_artifacts）在 `HPA_POSTGRES_RUNTIME=1` 时以 PostgreSQL 为权威；支持跨节点导入、版本/文件读取、发布状态、测试运行与运行产物，SQLite 单节点模式保持不变；Skill 配额按 PostgreSQL 计数。
- Phase 13 P13-01：企业租户、配额、成员与 SCIM Group 在 `HPA_POSTGRES_RUNTIME=1` 时以 PostgreSQL 为权威，SQLite 仅作为可重建缓存；API 不再写入 SQLite 权威行，周期任务改为从 PostgreSQL 重建缓存。
- 浏览器会话、OAuth 2.1 AS 状态（clients/requests/codes/tokens/consents/signing keys）与 Agent 版本/会话/事件/审批/工具调用切换到 PostgreSQL，支持跨节点读取、恢复、取消、审批与工具幂等。
- Agent 配额按 PostgreSQL 计数；`verify-mirror` 继续作为 PostgreSQL 与 SQLite 缓存的漂移门禁。
- 平台令牌、审计、Worker 注册/心跳与 Worker Job 保持 PostgreSQL 权威；SQLite 单节点模式与现有 API、Worker、浏览器 SSO 行为不变。

### Added

- Workflow execution controls：终态 `POST /api/executions/{id}/retry` 使用原快照/输入重跑；启动与重试支持 `Idempotency-Key`（SQLite/PostgreSQL 原子映射）；运行中取消通过持久化 `execution_cancel_requests` 协作完成；重复审批决定幂等，相反决定返回 409。
- Web 与 VS Code 增加执行重试入口；运行中取消会先显示“取消请求已接收”，再由执行 owner 在当前步骤结束后的边界停止。
- 本地 OpenAPI 现在覆盖 `ApiState::router` 的全部 109 个路径模板及 HTTP 方法；`check:contracts` 增加运行时路由漂移检查。
- 执行列表服务端分页：`GET /api/executions` 支持 `offset` 与 `status`，`X-Total-Count` 返回过滤后总数；控制台 50 条/页并显示「共 N 条 · 第 X/Y 页」，状态筛选移到服务端（此前只在前 100 条里筛）。
- 只读副本：`HPA_POSTGRES_REPLICA_URL` 让控制台列表/计数走流复制 standby，写入与强一致读走主库；副本不可用时启动降级、查询回退主库。
- `pnpm test:ha`：临时目录建真实主备两集群，验证读走副本、复制延迟、副本故障回退、主库宕机后提升备库继续读写。
- `pnpm test:rolling`：两轮「杀持有待审批执行的节点 → 空 data_dir 起替代节点 → 替代节点批准并跑完」，断言不丢、不重、无偏斜。
- `hpa doctor` 在配置 `HPA_POSTGRES_URL` 时检查连通性、migration drift、按 schema 拆分的 FORCE RLS 表数（42 租户 + 26 控制面）与插件包缺口（`packagesMissing` + 可执行告警）。
- `pnpm test:postgres`：每个 PostgreSQL 测试文件使用独立数据库，消除跨文件配额/会话污染；CI 的 postgres job 与双节点、滚动重启、并发迁移步骤都接到这套脚本上。

### Fixed

- `db migrate` 原先把 `before`/`after` 计数放在 SQLx 自身锁之外，三进程并发时都报告相同非零计数；现在整个窗口持有 `pg_advisory_lock`。
- `hpa serve` 连接 PostgreSQL 不再直接失败：`HPA_POSTGRES_CONNECT_RETRIES`（默认 5）与 `HPA_POSTGRES_CONNECT_TIMEOUT_SECS`（默认 5s），失败信息为 `cannot connect to PostgreSQL after N attempt(s): …`。
- CLI 错误不再打印 `Error: CliError("…")`，改为直接输出消息。
- `postgres_mirror_verify` 在缓存方向反转后存在竞态（100ms 重建期间改 SQLite 可能被覆盖），改为停服改缓存、重启观察修复。
- `postgres_member_authority` 曾在 100ms 缓存刷新与“SQLite 必须为 0”断言之间竞态；该套件改为验证 PostgreSQL 权威，缓存刷新由 `postgres_control_refresh` 独立覆盖。

### Tests

- 全量 Rust：250 passed / 0 failed / 1 ignored；Clippy `-D warnings` 与 fmt 通过。
- 真实 PostgreSQL 18.4 上 `pnpm test:postgres` 文件级隔离运行：**31 / 31 文件（32 / 32 用例）通过**，含远程 Marketplace 签名索引/安装/撤回链路与跨节点配额计数。
- MCP 官方 conformance：2025-11-25 核心 9/9；2026-07-28 `server-stateless` 28/28、内容/Header 场景 18/18、MRTR 14 场景/40 检查全通过；计分 server requirements 37/37，总报告 162 passed / 29 failed（失败全部在不计分的 tasks extension）。
- PostgreSQL 备份/恢复：21 个 migration、71 个业务关系、68 张 FORCE RLS 表一致。
- Web：typecheck、12 Vitest、build；扩展：65 Vitest、7 个 Extension Host 检查、build。
- 品牌、IR、契约（含运行时路由/方法漂移检查）、文档（182 文件 881 链接）、镜像、OTel、供应链门禁通过。
