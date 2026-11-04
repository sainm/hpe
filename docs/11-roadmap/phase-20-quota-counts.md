# Phase 20：PG 模式配额计数收口

> 状态：**本地验收完成**（2026-11-04）。修复 PostgreSQL 运行时下 Plugin 与 Connection 配额仍查 SQLite、导致 `maxPlugins` / `maxConnections` 被绕过的问题。

## 1. 范围

- `EnterpriseState::check_quota` 的 `POST /plugins/install` 与 `POST /connections` 改用 `Db::plugin_count()` / `Db::connector_connection_count()`。
- 新增 `PostgresPluginStore::count_plugins`（排除 `uninstalled`）与 `PostgresConnectorStore::count_connections`。
- 本地 SQLite 模式保持原查询语义；PG 模式计数来自 `hpa_tenant.plugins` 与 `hpa_tenant.connections`。
- 配额值、429 错误码（`TENANT_QUOTA` / `TENANT_CONCURRENCY`）与审计事件不变。

## 2. 行为契约

- 任一节点创建的插件/连接立即计入所有节点的配额检查，无需缓存刷新。
- `uninstalled` 插件不计入 `maxPlugins`；连接删除后计数立即下降。
- 配额检查仍发生在请求进入对应 handler 之前；被拒绝的请求不写入 PostgreSQL。
- 提高配额后同一请求可立即成功，不需要重启。
- 未迁移的配额（Workflow/Skill/Agent/执行/并发）继续使用原有 PG 计数路径。

## 3. PostgreSQL schema

复用 `hpa_tenant.plugins` 与 `hpa_tenant.connections`，无需新增 migration。

## 4. 验收

```bash
HPA_POSTGRES_TEST_URL=postgresql://user@host:5432/db \
  cargo test -p hpa-local --test postgres_quota_counts
```

`postgres_quota_counts` 覆盖：

- 设置 `maxPlugins=1`、`maxConnections=1` 后，节点 A 创建样例插件与连接。
- 节点 B 安装第二个插件、创建第二个连接均返回 429 `TENANT_QUOTA`。
- 直接查询 PostgreSQL 确认计数为 1/1。
- 提高配额到 2/2 后，同一节点 B 的安装与连接立即成功，计数变为 2/2。
- 两节点 SQLite `plugins/plugin_packages/connections` 保持 0 行。

门禁：全量 Rust 247 passed / 1 ignored；真实 PostgreSQL 18.4 上 31 个 PG 集成测试文件（32 用例）通过；PostgreSQL 备份演练恢复 21 个 migration、71 个业务关系与 68 张 FORCE RLS 表一致；Web typecheck、12 Vitest 与 build 通过；扩展 65 Vitest、7 个 Extension Host 检查通过；品牌/IR/契约/文档（177 文件 865 链接）/镜像/OTel/供应链门禁通过。

## 5. 非目标

- 不改变配额默认值、RBAC 或租户暂停语义。
- 不引入按节点/按用户的细分配额。
- 不改变 SQLite 单节点模式的计数查询。

创建入口的完整覆盖（样例路由与 Marketplace 安装）见 [Phase 21](phase-21-quota-entrypoints.md)。
