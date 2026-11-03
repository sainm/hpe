# Changelog

## Unreleased

### Changed

- Phase 13 P13-01：企业租户、配额、成员与 SCIM Group 在 `HPA_POSTGRES_RUNTIME=1` 时以 PostgreSQL 为权威，SQLite 仅作为可重建缓存；API 不再写入 SQLite 权威行，周期任务改为从 PostgreSQL 重建缓存。
- 浏览器会话、OAuth 2.1 AS 状态（clients/requests/codes/tokens/consents/signing keys）与 Agent 版本/会话/事件/审批/工具调用切换到 PostgreSQL，支持跨节点读取、恢复、取消、审批与工具幂等。
- Agent 配额按 PostgreSQL 计数；`verify-mirror` 继续作为 PostgreSQL 与 SQLite 缓存的漂移门禁。
- 平台令牌、审计、Worker 注册/心跳与 Worker Job 保持 PostgreSQL 权威；SQLite 单节点模式与现有 API、Worker、浏览器 SSO 行为不变。

### Tests

- 全量 Rust：234 passed / 1 ignored；真实 PostgreSQL 18.4 上 24 个 PG 集成测试文件（25 用例）通过。
- Web：typecheck、11 Vitest、build；扩展：63 Vitest、7 个 Extension Host 检查。
- 品牌、IR、契约、文档（166 文件 841 链接）、镜像、OTel、供应链门禁通过。
