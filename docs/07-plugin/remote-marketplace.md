# Remote Marketplace、发布者与撤回

> 状态：**P13-05 IN_PROGRESS**。签名远程索引、发布者账号/公钥、发布者密钥吊销、Marketplace 索引密钥轮换、HTTP/离线镜像刷新、包哈希校验、远程安装与撤回传播、在线 OSV/离线漏洞库安装门禁、cosign Sigstore bundle + OCI digest provenance、Remote Marketplace Web/VS Code 管理面、持久 SSE 撤回推送已实现并通过测试；Fulcio/Rekor keyless 身份策略、OCI Registry 拉取和跨控制面 webhook federation 仍未完成。

## 1. Trust chain

```text
configured Marketplace index key
  -> verifies hpa.marketplace.index/v1
       -> package SHA-256
       -> publisher ID
       -> registered publisher signing key
       -> verifies plugin manifest/content signature
       -> install
```

Marketplace 索引签名和插件包签名是两层独立信任：

- Index key 只证明索引内容；
- Publisher key 只证明具体插件包；
- 索引把 `pluginId + version + publisherId + signerKeyId + packageSha256` 绑定在一起；
- 任一层缺失、密钥过期/吊销、发布者暂停或哈希不一致都 fail-closed。
- 可选 provenance 层用 cosign 验证 statement bundle，并要求 statement subject 同时绑定包 SHA-256 与 OCI manifest digest。

## 2. Index

Schema：`hpa.marketplace.index/v1`。索引包含：

```json
{
  "schemaVersion": "hpa.marketplace.index/v1",
  "sourceId": "community",
  "sequence": 12,
  "generatedAt": "2026-11-01T00:00:00Z",
  "packages": [{
    "pluginId": "plugin_example",
    "version": "1.0.0",
    "path": "packages/example-1.0.0.hplugin",
    "sha256": "...",
    "publisherId": "publisher_example",
    "signerKeyId": "ed25519-...",
    "reviewStatus": "approved",
    "publishedAt": "2026-11-01T00:00:00Z",
    "provenance": {
      "kind": "cosign-v1",
      "statementPath": "provenance/example.json",
      "statementSha256": "...",
      "bundlePath": "provenance/example.bundle.json",
      "bundleSha256": "..."
    },
    "oci": {
      "repository": "registry.example/hpa/plugin_example",
      "digest": "sha256:..."
    }
  }],
  "withdrawals": [{
    "pluginId": "plugin_old",
    "version": "0.9.0",
    "reason": "security advisory",
    "at": "2026-11-01T01:00:00Z"
  }],
  "signature": {
    "algorithm": "ed25519",
    "keyId": "source-ed25519-...",
    "publicKey": "...",
    "value": "..."
  }
}
```

签名 payload 是去掉 `signature` 后的固定结构 JSON。`sequence` 不得倒退；相同 sequence 可幂等重放。

## 3. 发布者与密钥

```bash
hpa plugin publisher-add publisher_example "Example Publisher"
hpa plugin publisher-key-add publisher_example \
  --key-id ed25519-... --public-key BASE64 \
  --valid-from 2026-11-01T00:00:00Z \
  --valid-until 2027-11-01T00:00:00Z
hpa plugin publisher-key-revoke publisher_example --key-id ed25519-old
```

- Publisher 状态支持 `active`/`suspended`。
- 一个发布者可同时登记多个有效 key，用于轮换；每个包必须匹配索引声明的 key。
- key 有效期、吊销时间和发布者状态在每次刷新/安装时重新检查。
- 已吊销 key 不再可通过远程安装，即使旧索引仍引用它。

## 4. Source 与离线 mirror

```bash
hpa plugin marketplace-source-add community \
  --label "Community" \
  --index https://marketplace.example/index.json \
  --key-id source-ed25519-... --public-key BASE64

hpa plugin marketplace-source-refresh community
hpa plugin marketplace-remote --source community
hpa plugin marketplace-remote-install community plugin_example 1.0.0
```

Source 可以是：

- HTTPS index（loopback 测试可用 HTTP）；禁止重定向、代理、userinfo、私网/metadata 目标；
- 本地离线 mirror 目录。`path` 必须是索引目录内相对路径，绝对路径和 `..` 拒绝。

下载包缓存在：

```text
<data_dir>/marketplace/sources/<sourceId>/packages/<pluginId>/<version>.hplugin
```

刷新时写临时文件再 rename。安装前重新校验缓存 SHA-256，防止下载后篡改。

Index key 轮换：

```bash
hpa plugin marketplace-source-rotate community \
  --key-id source-ed25519-new --public-key BASE64 --label "2027 key"
```

旧 index key 立即吊销。新索引应由新 key 签署；旧 key 的索引刷新失败。

## 5. Sigstore / OCI provenance

从国内 Go proxy 安装固定版本 cosign：

```bash
./scripts/install-cosign-domestic.sh
export HPA_COSIGN_BIN="$PWD/target/tools/cosign"
```

签名模式：

```bash
# 离线 key-based Sigstore bundle
export HPA_COSIGN_PUBLIC_KEY="$PWD/trust/marketplace-cosign.pub"
export HPA_REQUIRE_PROVENANCE=1

# 或 keyless 身份策略（需要 Fulcio/Rekor 与 OIDC 环境）
export HPA_COSIGN_CERTIFICATE_IDENTITY='https://issuer.example/workload'
export HPA_COSIGN_OIDC_ISSUER='https://issuer.example'
```

验证步骤：

1. 从已签名 index 读取 statement/bundle 路径与 SHA-256。
2. 拒绝超过 4 MiB、哈希不符或缺少路径的 provenance。
3. 校验 in-toto Statement type、predicateType、包 SHA-256 subject。
4. 有 `oci.digest` 时，必须另有一个 subject 绑定该 OCI manifest digest。
5. 执行无 shell 的 `cosign verify-blob --bundle ... --key ...` 或 keyless identity 参数。
6. refresh 将已验证 statement/bundle 写入 package 同目录；install 前重新计算哈希并再次执行 cosign。

Key-based 模式使用 `--insecure-ignore-tlog=true`，证明签名但**不证明 Rekor 透明日志收录**。Keyless 模式需要 Fulcio/Rekor/OIDC 外部环境，未提供时保持 `BLOCKED_EXTERNAL`。当前实现绑定 OCI digest，但不会从 Registry 拉取或枚举 attestation manifest；该步骤仍需真实 Registry 权限。

## 6. Review、安装与撤回

- Index 中 remote review status 只接受 `approved`/`rejected`。
- 只有 `approved` 且未撤回的包可安装。
- 安装再次执行 manifest integrity、Ed25519、license、兼容性、依赖、capability 和 SBOM 包检查。
- withdrawal 将本地 remote package 标记撤回；若该版本已安装，插件及其 Connector/MCP/capability 被停用。
- 从 index 消失的已缓存 package 按 `removed-from-index` 撤回，不能继续安装。
- 每次刷新和撤回写 plugin event。

## 7. API

```text
GET    /api/marketplace/remote?source=community
GET    /api/marketplace/publishers
POST   /api/marketplace/publishers
POST   /api/marketplace/publishers/{id}/keys
DELETE /api/marketplace/publishers/{id}/keys
GET    /api/marketplace/sources
POST   /api/marketplace/sources
POST   /api/marketplace/sources/{id}/rotate-key
POST   /api/marketplace/sources/{id}/refresh
POST   /api/marketplace/remote/{source}/{pluginId}/versions/{version}/install
GET    /api/marketplace/vulnerabilities/{pluginId}/{version}
GET    /api/marketplace/events/latest
GET    /api/marketplace/events/stream?since=...
```

VS Code Plugin Tree 提供：

- Installed / Remote Sources / Publishers 分组；
- `hpa.marketplace.manage` 操作面板；
- source 添加/刷新、index key 轮换；
- publisher 与 signing key 添加/吊销；
- 远程包安装与漏洞扫描；
- package description/tooltip 展示 Sigstore、OCI digest、publisher、signer 和 SHA-256。

撤回推送：

- 每次 plugin event 与 `marketplace_event_stream` 在同一 SQLite/PostgreSQL 事务写入。
- SSE 以单调 sequence 作为 `Last-Event-ID`，支持 `since` 和 header 恢复。
- Web Plugin Center 与 VS Code Plugin Tree 建立长连接；收到 `withdrawn` event 后刷新列表并提示。
- SSE 使用 256 个事件的有界 channel；客户端阻塞导致连接终止，服务端不会无界积压。
- PostgreSQL 表启用 FORCE RLS，sequence 在 tenant 内单调，跨租户不可见。

## 8. 漏洞门禁

安装任何插件包前按 `HPA` ecosystem 查询 `pluginId@version`：

```bash
# 在线 OSV（默认 https://api.osv.dev/v1/query）
export HPA_VULNERABILITY_SOURCE=osv
# 可选：插件确实对应某个 OSV 已知 ecosystem 时指定；默认仅按包名保守查询
export HPA_OSV_ECOSYSTEM=PyPI

# 离线 OSV 快照
export HPA_VULNERABILITY_SOURCE=offline
export HPA_VULNERABILITY_DB=/secure/hpa-vulnerabilities.json
export HPA_VULNERABILITY_MAX_AGE_HOURS=24
```

默认 `disabled` 仅适用于本地开发。生产应将 `osv` 或 `offline` 配置纳入部署门禁；一旦配置，查询失败、响应超限、离线库过期或 schema 不兼容均阻止安装，不回退为“无漏洞”。

离线库格式：

```json
{
  "schemaVersion": "hpa.vulnerability-db/v1",
  "generatedAt": "2026-11-02T00:00:00Z",
  "records": [{
    "id": "HPA-2026-0001",
    "summary": "Example advisory",
    "modified": "2026-11-02T00:00:00Z",
    "affected": [{
      "package": {"ecosystem": "HPA", "name": "plugin_example"},
      "ranges": [{
        "type": "SEMVER",
        "events": [{"introduced": "1.0.0"}, {"fixed": "1.1.0"}]
      }]
    }]
  }]
}
```

查询报告：

```text
GET /api/marketplace/vulnerabilities/{pluginId}/{version}
```

OSV 公共 API 不接受自造的 `HPA` ecosystem，因此在线模式默认只发送 `name + version`，结果按包名保守处理；插件映射到 PyPI/npm 等已知 ecosystem 时显式设置 `HPA_OSV_ECOSYSTEM`。离线库则明确匹配 `HPA` ecosystem。报告中的 `query.ecosystem` 记录实际查询口径。

报告状态为 `disabled`、`clean` 或 `vulnerable`。安装包含任何命中的漏洞时返回 `PLUGIN_FORBIDDEN`，错误中列出 advisory ID。在线端点禁止重定向、代理、userinfo；非 loopback 必须 HTTPS，解析到私网/metadata 地址时拒绝。

## 9. 验证

`marketplace_remote` 集成测试覆盖：

- 本地签名插件包 → 发布者 key 注册 → Ed25519 index → HTTP 远程索引 → refresh → install；
- 包被篡改但 index hash 不变时拒绝刷新；
- publisher key 吊销后拒绝包；
- withdrawal 停用已安装插件并阻止再次安装；
- index key 轮换后旧 key 签名失败；
- 本地离线 mirror 路径和 HTTP loopback source。
- 离线 OSV 数据库命中后阻止安装并返回 advisory ID；
- loopback OSV 查询验证在线响应解析和 vulnerable 状态。
- 真实 cosign v2.6.0 生成 key pair 与 Sigstore statement bundle；
- in-toto statement 同时绑定 package SHA-256 和 OCI manifest digest；
- cosign verify-blob 成功，statement 被篡改后安装 fail-closed；
- `HPA_REQUIRE_PROVENANCE=1` 时缺失 provenance 的远程包拒绝刷新。
- VS Code 扩展 63 项单元测试覆盖 marketplace request、Tree 分组、manifest command 和本地化。
- Rust HTTP SSE 测试先接收 `plugin.installed`，断开后生成 `plugin.disabled`，再用 `Last-Event-ID` 只接收后续事件。

Remote Marketplace UI E2E：

```bash
HPA_BASE_URL=http://127.0.0.1:8787 \
HPA_DATA_DIR=/path/to/hpa-data \
pnpm test:marketplace-ui
```

覆盖 source/publisher/key 列表、远程包表、实时撤回通知、1440/607 布局、axe serious/critical=0 和横向溢出拒绝。

## 10. 尚未完成

- VS Code Webview 尚未提供 Remote Marketplace publisher/source 管理；Web UI 已完成。
- 跨控制面/已离线部署实例的 webhook 或 federation 推送；当前 SSE 针对同一控制面的 Web/VS Code 客户端。
- Fulcio/Rekor keyless 身份策略、OCI Registry attestation 拉取和在线恶意软件情报源。
- 发布者所有权验证、SCIM 组织成员绑定和审核 SLA/申诉流程。
- 自动撤回通知到远程已部署实例仍需事件总线/控制面推送接入。
