# Changelog

## Unreleased

### Changed

- Phase 18：Agent 主动执行增加 PostgreSQL 会话 owner lease（`lease_owner/lease_token/lease_expires_at`，migration `0020`）；start/drive/approval resume 获取租约，persist/emit 按 holder+token+未过期围栏，终态或等待审批释放；`recover` 跳过仍被其他节点持有的 running 会话，只接管过期会话，旧 token 写入失败；本地 SQLite 模式不变。
- Phase 17：插件生命周期、本地/远程 Marketplace 与信任元数据（plugin_packages、plugins、plugin_capabilities、plugin_events、plugin_market_listings、plugin_reviews、publishers/sources/remote_packages、marketplace_event_stream）在 `HPA_POSTGRES_RUNTIME=1` 时以 PostgreSQL 为权威；插件贡献的 MCP server 与 Connector 分别走 Phase 15/16 存储，支持跨节点安装状态、启停、审核、评分、撤回与信任 key 管理；插件包文件、签名私钥与安装目录仍在节点本地，SQLite 单节点模式与插件 CLI/UI 保持不变。
- Phase 16：Connector 定义、连接与审计（connector_definitions、connections、connector_audit）在 `HPA_POSTGRES_RUNTIME=1` 时以 PostgreSQL 为权威；节点启动幂等注册内置连接器，支持跨节点创建/读取/更新/测试/删除连接与审计，插件启停/卸载的 Connector 贡献走同一路径，SQLite 单节点模式与执行引擎保持不变。
- Phase 15：MCP 控制面元数据（mcp_tokens、mcp_approvals、mcp_audit、mcp_servers）在 `HPA_POSTGRES_RUNTIME=1` 时以 PostgreSQL 为权威；支持跨节点 PAT 创建/查找/吊销、审批决定/一次性消费/过期、审计写入与外部服务器管理，SQLite 单节点模式与 MCP Gateway/Center 行为保持不变；持久 MCP session/task 仍由 `HPA_POSTGRES_MCP=1` 独立覆盖。
- Phase 14：Skill 包与版本元数据（skills、skill_versions、skill_files、skill_runs、skill_run_artifacts）在 `HPA_POSTGRES_RUNTIME=1` 时以 PostgreSQL 为权威；支持跨节点导入、版本/文件读取、发布状态、测试运行与运行产物，SQLite 单节点模式保持不变；Skill 配额按 PostgreSQL 计数。
- Phase 13 P13-01：企业租户、配额、成员与 SCIM Group 在 `HPA_POSTGRES_RUNTIME=1` 时以 PostgreSQL 为权威，SQLite 仅作为可重建缓存；API 不再写入 SQLite 权威行，周期任务改为从 PostgreSQL 重建缓存。
- 浏览器会话、OAuth 2.1 AS 状态（clients/requests/codes/tokens/consents/signing keys）与 Agent 版本/会话/事件/审批/工具调用切换到 PostgreSQL，支持跨节点读取、恢复、取消、审批与工具幂等。
- Agent 配额按 PostgreSQL 计数；`verify-mirror` 继续作为 PostgreSQL 与 SQLite 缓存的漂移门禁。
- 平台令牌、审计、Worker 注册/心跳与 Worker Job 保持 PostgreSQL 权威；SQLite 单节点模式与现有 API、Worker、浏览器 SSO 行为不变。

### Tests

- 全量 Rust：225 passed / 14 failed / 1 ignored；14 个失败全部是本机禁止 `sandbox-exec` 的既有用例（runner 8、bin 3、agents 1、mcp_http 1、mcp_stdio 1），与代码无关。真实 PostgreSQL 18.4 上用 `pnpm test:postgres` 做文件级隔离运行：29 个 PG 测试文件 28 个通过，`postgres_skill_cross_node` 因为技能测试要执行 python 脚本被同一沙箱限制拦截。
- Web：typecheck、11 Vitest、build；扩展：63 Vitest、7 个 Extension Host 检查。
- 品牌、IR、契约、文档（173 文件 856 链接）、镜像、OTel、供应链门禁通过。
