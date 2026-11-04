# Phase 21：配额门禁覆盖所有创建入口

> 状态：**本地验收完成**（2026-11-04）。补齐 Phase 20 未覆盖的插件/连接创建入口，避免通过样例路由或 Marketplace 安装绕过 `maxPlugins` / `maxConnections`。

## 1. 范围

插件创建入口统一进入 `maxPlugins` 检查：

- `POST /plugins/install`
- `POST /plugins/sample`
- `POST /marketplace/{id}/versions/{version}/install`
- `POST /marketplace/remote/{source}/{id}/versions/{version}/install`

连接创建入口统一进入 `maxConnections` 检查：

- `POST /connections`
- `POST /connectors/sample`

计数在 PG 模式来自 `hpa_tenant.plugins` / `hpa_tenant.connections`（Phase 20），SQLite 模式保持本地计数。

## 2. 行为契约

- 配额为 0 时上述所有入口返回 429 `TENANT_QUOTA`，请求不进入 handler、不产生数据。
- 配额提高后同一入口立即恢复，无需重启或缓存刷新。
- Marketplace 安装（本地与远程）与直接安装共用同一插件计数；已存在插件再次安装会按当前计数判断，达到上限即拒绝。
- 配额审计仍记录 `label`、`count`、`limit` 与请求路径。
- 插件安装内部贡献的 Skill/Agent 仍按既有行为处理，不在本阶段做安装前的能力配额预检。

## 3. PostgreSQL schema

无新增 migration；复用 Phase 20 的 `count_plugins` / `count_connections`。

## 4. 验收

```bash
HPA_POSTGRES_TEST_URL=postgresql://user@host:5432/db \
  cargo test -p hpa-local --test postgres_quota_counts
```

`postgres_quota_counts` 覆盖：

- `maxPlugins=0` / `maxConnections=0` 时，`/plugins/sample`、本地 Marketplace 安装、远程 Marketplace 安装与 `/connectors/sample` 全部返回 429。
- 配额提升到 1/1 后样例插件与连接创建成功；第二个插件安装与连接创建在节点 B 返回 429。
- PostgreSQL 计数与提高配额后的 2/2 结果一致；两节点 SQLite 表 0 行。

门禁：全量 Rust 247 passed / 1 ignored；真实 PostgreSQL 18.4 上 31 个 PG 集成测试文件（32 用例）通过；PostgreSQL 备份演练恢复 21 个 migration、71 个业务关系与 68 张 FORCE RLS 表一致；Web typecheck、12 Vitest 与 build 通过；扩展 65 Vitest、7 个 Extension Host 检查通过；品牌/IR/契约/文档（178 文件 868 链接）/镜像/OTel/供应链门禁通过。

## 5. 非目标

- 不改变配额默认值、RBAC、租户暂停或审计格式。
- 不实现插件贡献 Skill/Agent 的安装前配额预检。
- 不改变 SQLite 单节点模式。
