# Phase 16：Connector 元数据 PostgreSQL 权威

> 状态：**本地验收完成**（2026-11-03）。Phase 16 把 Connector 定义、连接与审计从租户 SQLite 迁到 PostgreSQL，并让插件启停/卸载的 Connector 贡献走同一权威路径；本地 SQLite 模式保持不变。

## 1. 范围

- `HPA_POSTGRES_RUNTIME=1` 时，以下数据只读写 `hpa_tenant` PostgreSQL schema（租户 RLS）：
  - `connector_definitions`：连接器 id/version、名称、类型、manifest JSON、来源插件、状态与创建时间。
  - `connections`：连接名称、引用定义、config JSON、凭证环境变量名、状态与时间戳。
  - `connector_audit`：test/invoke 的连接、操作、状态、延迟与脱敏 detail。
- 六个内置连接器（http/file/database/git/mq/s3）在节点启动时幂等 upsert；多节点看到同一批定义。
- 插件启用/停用/卸载对 Connector 定义的状态变更与删除通过同一 PostgreSQL 存储完成，不再只改 SQLite。
- Connector 执行（HTTP/File/PostgreSQL/Git/Redis MQ/S3）仍由各节点本地执行；本阶段只迁移元数据。

## 2. 行为契约

- 定义按 `(connector_id, version)` 冻结：相同 manifest 幂等返回 `inserted=false`；同版本不同内容返回 `Invalid`；已被其他插件注册返回 `Forbidden`。
- 按 id 查询未指定版本时取 `status='enabled'` 且 `version` 文本降序的第一条；插件停用后该定义不再被解析。
- 连接必须引用已存在的 `(connector_id, connector_version)`；保存连接前校验定义 enabled 与 config。
- `save_connection` 对同 id 执行更新并保留 `created_at`；删除为物理删除，审计行保留。
- 审计写入不阻断执行；detail 在写入前脱敏为 JSONB。列表按 `at DESC` 截断。
- 插件包、签名、Marketplace 发布者/来源/远程索引与插件自身表仍使用 SQLite（Phase 17）；本阶段只保证 Connector 与其插件贡献一致。

## 3. PostgreSQL schema

复用 `migrations/postgres/0002_tenant_core.sql` 与 `0007_remote_marketplace.sql`，无需新增 migration：

```text
hpa_tenant.connector_definitions
hpa_tenant.connections
hpa_tenant.connector_audit
```

所有表由 FORCE RLS 隔离；写事务使用 `SET LOCAL ROLE hpa_tenant_runtime` 与 `set_config('hpa.tenant_id', ..., true)`。`connections` 通过 `(tenant_id, connector_id, connector_version)` 复合外键引用定义。

## 4. 验收

```bash
HPA_POSTGRES_TEST_URL=postgresql://user@host:5432/db \
  cargo test -p hpa-local --test postgres_connector_cross_node
```

`postgres_connector_cross_node` 覆盖：

- 节点 A/B 都从 PostgreSQL 读取同一批内置 Connector 定义。
- 节点 A 创建样例 File 连接，节点 B 读到相同 config。
- 节点 B 更新连接名称，节点 A 立即读到新名称。
- 节点 B 执行连接测试，节点 A 在 `/api/connector-audit` 读到该 `test` 审计。
- 节点 A 删除连接后，节点 B 列表不再包含它。
- 存储级验证定义幂等、同版本变更失败、跨插件注册失败、停用与删除。
- 两节点 SQLite `connector_definitions/connections/connector_audit` 均为 0 行。

门禁：全量 Rust 237 passed / 1 ignored；真实 PostgreSQL 18.4 上 27 个 PG 集成测试文件（28 用例）通过；Web typecheck、11 Vitest 与 build 通过；扩展 63 Vitest、7 个 Extension Host 检查通过；品牌/IR/契约/文档（170 文件 850 链接）/镜像/OTel/供应链门禁通过。

## 5. 非目标

- 不迁移插件包、签名、Marketplace 发布者/来源/远程索引与插件自身表；这些仍是 Phase 17 的范围。
- 不共享连接器进程、凭证环境变量值或本地文件工作区；跨节点只共享元数据。
- 不改变 Connector manifest 校验、SSRF/主机白名单、速率限制或执行引擎。
- 不改变本地 SQLite 模式与 CLI 路径。
