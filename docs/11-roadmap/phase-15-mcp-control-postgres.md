# Phase 15：MCP 控制面元数据 PostgreSQL 权威

> 状态：**本地验收完成**（2026-11-03）。Phase 15 把 MCP PAT、审批、审计与外部服务器元数据从租户 SQLite 迁到 PostgreSQL；本地 SQLite 模式保持不变。

## 1. 范围

- `HPA_POSTGRES_RUNTIME=1` 时，以下数据只读写 `hpa_tenant` PostgreSQL schema（租户 RLS）：
  - `mcp_tokens`：PAT 名称、SHA-256 token hash、scope、`last_used_at`、`revoked_at`。
  - `mcp_approvals`：高风险工具审批的 token、工具名、参数 JSON、参数哈希、request state、risk、TTL、决定与一次性消费时间。
  - `mcp_audit`：MCP Gateway 调用的 token、方法、工具、状态、延迟与结构化 detail。
  - `mcp_servers`：外部 MCP 服务器名称、传输、stdio 命令、URL、工具/网络白名单与凭证环境变量名。
- 本地 SQLite 模式、MCP Gateway/stdio/HTTP 传输、MCP Center、OAuth 资源元数据与工具目录语义不改变。
- `mcp_overview` 的 stats（待审批、已发布 Skill 工具、请求数）在 PostgreSQL 模式按 `hpa_tenant` 行计数。

## 2. 行为契约

- PAT 认证按 `token_hash` 查询并触碰 `last_used_at`；吊销后两端点立即拒绝。
- 审批创建为 `pending`；决定只接受 `pending`；消费只接受 `approved` 且 `used_at IS NULL`，重复消费返回 false。TTL 过期的 `pending` 审批由读取路径与后台扫描收敛为 `expired`。
- 审批按 `(tenant_id, id)` 与 `request_state` 查询；跨节点创建、决定、消费与过期立即一致。
- 审计写入不阻断请求；授权成功与失败路径都写 `mcp_audit`。列表按 `at DESC` 截断。
- 服务器元数据 upsert 幂等；`enabled` 在 PostgreSQL 中恒为 true，删除为物理删除。JSON 字段以 JSONB 保存，读回顺序不保证与写入字符串逐字相同。
- `credential_env` 只保存环境变量名，不保存凭证值；服务器进程与 stdio 命令仍在各节点本地执行。
- 持久 MCP session/subscription/task 状态不在本阶段：`HPA_POSTGRES_MCP=1` 继续使用 `hpa_control` 的既有表与 owner-node 命令。
- Plugin/Connector 元数据仍未迁移；`hpa mcp token ...` CLI 仍直接操作本地 SQLite，PostgreSQL 模式的生产路径是 `hpa serve` 的 HTTP API。

## 3. PostgreSQL schema

复用 `migrations/postgres/0002_tenant_core.sql`，无需新增 migration：

```text
hpa_tenant.mcp_tokens
hpa_tenant.mcp_approvals
hpa_tenant.mcp_audit
hpa_tenant.mcp_servers
```

所有表由 FORCE RLS 隔离；写事务使用 `SET LOCAL ROLE hpa_tenant_runtime` 与 `set_config('hpa.tenant_id', ..., true)`。`mcp_approvals.token_id` 通过 `(tenant_id, token_id)` 复合外键引用 `mcp_tokens`。

## 4. 验收

```bash
HPA_POSTGRES_TEST_URL=postgresql://user@host:5432/db \
  cargo test -p hpa-local --test postgres_mcp_meta_cross_node
```

`postgres_mcp_meta_cross_node` 覆盖：

- 节点 A 通过 `/api/mcp/tokens` 创建 PAT，节点 B 列表立即可见。
- 节点 B 用该 PAT 调用 `/mcp` 的 `server/discover` 返回 200；节点 A 的 `/api/mcp/audit` 读到这次跨节点调用。
- 节点 B 吊销 PAT 后，节点 A 读到 `revokedAt`，且同一 PAT 在节点 A 调用 `/mcp` 返回 401。
- 审批在 A 创建后 B 可列出为 pending；B 决定、消费，A 读到 `approved` 与 `used_at`；重复消费为 false；过期审批被 B 的过期扫描收敛为 `expired`。
- 审计行 A 写入 B 读取；服务器元数据 A upsert 后 B 的 `/api/mcp/servers` 可见，B 删除后 A 读取为 None。
- stats 返回 pending=0、published Skill tools=0、requests≥1。
- 两节点 SQLite `mcp_tokens/mcp_approvals/mcp_audit/mcp_servers` 均为 0 行。

门禁：全量 Rust 236 passed / 1 ignored；真实 PostgreSQL 18.4 上 26 个 PG 集成测试文件（27 用例）通过；Web typecheck、11 Vitest 与 build 通过；扩展 63 Vitest、7 个 Extension Host 检查通过；品牌/IR/契约/文档/镜像/OTel/供应链门禁通过。

## 5. 非目标

- 不迁移 MCP session/subscription/task 持久状态；该能力由 `HPA_POSTGRES_MCP=1` 独立覆盖。
- 不改变 MCP 协议、MRTR 审批流程、限流、工具/资源/Prompt 目录或 OAuth AS 行为。
- 不共享外部 MCP 服务器进程、stdio 子进程或凭证环境变量值；跨节点只共享元数据。
- 不迁移 Plugin/Connector 元数据，不改变本地 SQLite 模式与 CLI 路径。
