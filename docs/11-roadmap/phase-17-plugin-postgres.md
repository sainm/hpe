# Phase 17：Plugin 元数据 PostgreSQL 权威

> 状态：**本地验收完成**（2026-11-03）。Phase 17 把插件生命周期、本地 Marketplace 与远程 Marketplace 元数据从租户 SQLite 迁到 PostgreSQL，并把插件贡献的 MCP server 纳入 Phase 15 的 PostgreSQL 存储；本地 SQLite 模式保持不变。

## 1. 范围

- `HPA_POSTGRES_RUNTIME=1` 时，以下数据只读写 `hpa_tenant` PostgreSQL schema（租户 RLS）：
  - `plugin_trusted_keys`：本地与受信 Ed25519 公钥。
  - `plugin_packages`、`plugins`、`plugin_capabilities`、`plugin_events`：包元数据、安装状态、能力贡献与插件事件。
  - `plugin_market_listings`、`plugin_reviews`：本地 Marketplace 上架、审核、评分与下载计数。
  - `plugin_publishers`、`plugin_publisher_keys`：发布者与签名 key 注册、撤销。
  - `marketplace_sources`、`marketplace_source_keys`、`marketplace_remote_packages`：远程索引来源、轮换 key、缓存包与撤回状态。
  - `marketplace_event_stream`：Marketplace SSE 事件流。
- 插件贡献的 MCP server 通过 Phase 15 的 `hpa_tenant.mcp_servers` 写入插件归属、启停与删除；Connector 贡献继续走 Phase 16 的 Connector 存储。
- 插件包文件、签名私钥与安装目录仍属于节点本地；跨节点共享的是元数据、状态与信任材料。

## 2. 行为契约

- 包按 `(plugin_id, version)` 冻结：相同 content hash 幂等；同版本不同内容失败。
- 安装/升级/回滚/启停/卸载更新 PostgreSQL 中的 `plugins` 状态、安装版本与能力状态；插件贡献同步写入 Connector 与 MCP server 存储。
- 跨节点读取插件列表、详情、包、能力、事件、Marketplace 列表/详情/评分与发布者/来源立即可见。
- 停用可在任意节点执行；启用与回滚会读取节点本地的已安装目录与能力文件，因此要求该节点具备插件包文件。只有元数据而没有本地文件的节点应先在本地安装或接入共享存储。
- Marketplace 发布、审核、评分、撤回与下载计数写入 PostgreSQL；远程 Marketplace 的索引仍由节点拉取并验证签名，索引与包缓存路径保存在元数据中，包文件由拉取节点本地持有。
- 插件事件在同一事务中写入 `plugin_events` 与 `marketplace_event_stream`；SSE 序列由 PostgreSQL identity 生成。
- 本地 SQLite 模式、插件 CLI、Marketplace UI 与安装校验（签名、许可、OSV、provenance）语义不改变。

## 3. PostgreSQL schema

复用既有 migration，无需新增：

```text
hpa_tenant.plugin_trusted_keys
hpa_tenant.plugin_packages
hpa_tenant.plugins
hpa_tenant.plugin_capabilities
hpa_tenant.plugin_market_listings
hpa_tenant.plugin_reviews
hpa_tenant.plugin_events
hpa_tenant.plugin_publishers
hpa_tenant.plugin_publisher_keys
hpa_tenant.marketplace_sources
hpa_tenant.marketplace_source_keys
hpa_tenant.marketplace_remote_packages
hpa_tenant.marketplace_event_stream
```

所有表由 FORCE RLS 隔离；写事务使用 `SET LOCAL ROLE hpa_tenant_runtime` 与 `set_config('hpa.tenant_id', ..., true)`。

## 4. 验收

```bash
HPA_POSTGRES_TEST_URL=postgresql://user@host:5432/db \
  cargo test -p hpa-local --test postgres_plugin_cross_node
```

`postgres_plugin_cross_node` 覆盖：

- 节点 A 安装签名样例插件，节点 B 读取插件、包、三项能力与插件贡献的 Connector。
- 节点 B 停用插件，节点 A 读到 `disabled`；节点 A 重新启用。
- 节点 A 发布 Marketplace，B 审核通过、评分 5 分、撤回；A 读到审核状态、评分与撤回。
- 节点 A 注册发布者/key 与 Marketplace 来源/key，B 读取；A 撤销发布者 key 后 B 读到 `revokedAt`。
- 存储级验证远程包 upsert、来源 sequence、撤回，以及插件 MCP server 归属、启停与删除。
- 节点 A 卸载插件后，节点 B 读到 `uninstalled`。
- 两节点 SQLite 的插件、Marketplace、信任 key 与 `mcp_servers` 表均为 0 行。

门禁：全量 Rust 238 passed / 1 ignored；真实 PostgreSQL 18.4 上 28 个 PG 集成测试文件（29 用例）通过；Web typecheck、11 Vitest 与 build 通过；扩展 63 Vitest、7 个 Extension Host 检查通过；品牌/IR/契约/文档（171 文件 853 链接）/镜像/OTel/供应链门禁通过。

## 5. 非目标

- 不把插件包文件、签名私钥或已安装目录复制到其他节点；跨节点启用需要目标节点具备本地文件或共享存储。
- 不改变 Marketplace 索引签名、OSV、许可证、cosign/provenance 校验或安装信任模型。
- 不实现跨控制面 Marketplace federation、真实外部发布者治理或自动 key 分发。
- 不改变本地 SQLite 模式与 `hpa plugin ...` CLI 路径。
