# Phase 19：远程 Marketplace 全链路

> 状态：**本地验收完成**（2026-11-03）。Phase 19 打通签名远程 Marketplace 索引 → 跨节点刷新 → 远程安装 → 撤回传播，并让未缓存包的节点在安装时自动刷新来源。

## 1. 范围

- `install_remote_package` 在本地缓存包文件缺失时自动调用 `refresh_marketplace_source`，重新拉取索引与包，再按刷新后的元数据继续校验和安装；刷新后若包已撤回或未批准，安装失败关闭。
- 远程索引校验（Ed25519 源 key）、包 SHA-256、发布者 key、插件签名、许可证与 provenance 规则保持不变。
- 远程包元数据与撤回状态继续存 PostgreSQL；包缓存仍按节点本地保存，不新增跨节点文件复制。
- 撤回刷新会把已安装版本停用；其他节点通过共享 PostgreSQL 立即看到撤回状态并拒绝再次安装。

## 2. 行为契约

- 刷新按来源 `last_sequence` 拒绝索引回退；重复刷新相同 sequence 幂等。
- 包只缓存在执行刷新的节点；其他节点看到 `package_path` 指向刷新节点本地路径。缺少该文件的节点执行安装时会自动刷新并在本地重建缓存。
- 自动刷新失败原样返回错误；不会跳过签名、哈希、发布者或撤回检查。
- 索引撤回标记 `withdrawn_at` 与 `revocation_reason`；已安装版本被停用，后续安装返回 403。
- 来源 key 轮换/撤销、发布者状态、许可证与漏洞门禁仍按既有规则执行。
- 本地 SQLite 模式的远程 Marketplace 行为不变；跨节点一致性只在 `HPA_POSTGRES_RUNTIME=1` 时成立。

## 3. PostgreSQL schema

复用 `plugin_publishers`、`plugin_publisher_keys`、`marketplace_sources`、`marketplace_source_keys`、`marketplace_remote_packages` 与 `marketplace_event_stream`，无需新增 migration。包文件不进入 PostgreSQL。

## 4. 验收

```bash
HPA_POSTGRES_TEST_URL=postgresql://user@host:5432/db \
  cargo test -p hpa-local --test postgres_remote_marketplace_cross_node
```

`postgres_remote_marketplace_cross_node` 覆盖：

- 本地 HTTP 服务提供 Ed25519 签名的远程索引与真实签名插件包。
- 节点 A 注册发布者/key 与来源/key；节点 B 刷新来源并缓存包，节点 A 从 PostgreSQL 读到远程包元数据。
- 节点 A 卸载本地样例插件后，未刷新、无缓存直接远程安装成功；安装过程自动刷新 A 的缓存。
- 节点 B 通过 PostgreSQL 读到安装后的插件状态。
- 索引 sequence 2 撤回该版本；B 刷新后 A 读到撤回原因，已安装插件在 A 被停用，再次安装返回 403。
- 两节点 SQLite Marketplace/Plugin 表均为 0 行。

门禁：全量 Rust 246 passed / 1 ignored；真实 PostgreSQL 18.4 上 30 个 PG 集成测试文件（31 用例）通过；PostgreSQL 备份演练恢复 21 个 migration、71 个业务关系与 68 张 FORCE RLS 表一致；Web typecheck、12 Vitest 与 build 通过；扩展 65 Vitest、7 个 Extension Host 检查通过；品牌/IR/契约/文档（176 文件 863 链接）/镜像/OTel/供应链门禁通过。

## 5. 非目标

- 不实现远程包跨节点二进制复制；未命中缓存的节点通过重新刷新来源解决。
- 不实现 Marketplace 跨控制面 federation、Fulcio/Rekor keyless、OCI Registry attestation 或真实发布者账号体系。
- 不改变索引签名算法、信任模型、撤回原因分类或本地 Marketplace。
