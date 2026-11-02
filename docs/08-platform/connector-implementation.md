# Connector 生产扩展

> 状态：**P13-05 IN_PROGRESS**。HTTP、File、PostgreSQL DB、只读 Git、Redis MQ、S3-compatible Connector 已通过统一 ConnectorRuntime 接入；远程 Marketplace、发布者账号与完整来源证明仍未完成。

## 1. 统一契约

所有 Connector 复用同一套：

- `hpa.connector/v1` manifest 与 JSON Schema 输入验证。
- connection credential environment 引用；不把 secret 写入 config/audit。
- 每 connection/operation 的进程内滑动窗口限流。
- 超时、输出大小上限、稳定错误分类和 `connector_audit`。
- Connector tool risk；write/destructive 操作进入 Agent/MCP 审批边界。

新增定义：

| ID | 能力 | 默认风险 |
| --- | --- | --- |
| `database` 1.0.0 | PostgreSQL `query` / `execute` | read / destructive |
| `git` 1.0.0 | status/log/show/diff/ls-files/commit | read；commit 由 `allowWrite` 控制 |
| `mq` 1.0.0 | Redis `ping` / `publish` | read / write |
| `s3` 1.0.0 | put/get/head/list/delete | read/write/destructive |

## 2. PostgreSQL

配置示例：

```json
{
  "allowedHosts": ["db.internal:5432"],
  "statementTimeoutMs": 15000,
  "maxRows": 500,
  "maxResponseBytes": 2097152,
  "rateLimitPerMinute": 60
}
```

`credentialEnv` 指向完整 `postgresql://...` connection string。

- `query` 只允许 SELECT/WITH/SHOW/VALUES/EXPLAIN，并强制 PostgreSQL read-only transaction。
- `execute` 使用参数绑定，单语句、最大 256 KiB SQL、最多 100 个参数。
- 行数与响应字节有界；数据库 statement timeout 同时由连接内 GUC 控制。
- 当前只支持 PostgreSQL；MySQL/Oracle/SQL Server 未实现。
- read-only transaction 阻止数据库写入，但数据库外部函数仍可能产生非数据库副作用；生产数据库角色必须单独限制。

## 3. Git

配置示例：

```json
{
  "repositoryRoot": "/srv/repositories/service",
  "allowWrite": false,
  "maxOutputBytes": 2097152,
  "rateLimitPerMinute": 60
}
```

- 直接 argv 调用 `git -C <root>`，禁止 shell、prompt、system config 和 pager。
- revision 禁止选项注入/控制字符；path 禁止绝对路径和 `..`。
- 只支持本地 worktree/bare repository；不执行 fetch/pull/push。
- `commit` 只提交已 staged 内容，并要求 connection `allowWrite=true`；默认只读。
- 输出有超时和字节上限。

## 4. Redis MQ

配置示例：

```json
{
  "url": "redis://broker.internal:6379",
  "username": "hpa",
  "allowedHosts": ["broker.internal:6379"],
  "allowedChannels": ["hpa.events.*"],
  "database": 0,
  "maxMessageBytes": 1048576
}
```

- `credentialEnv` 指向密码；URL 禁止内嵌凭据。
- channel 必须匹配显式 allowlist；消息与响应有界。
- `publish` 必须提供 idempotency key。Lua 脚本以 Redis key 原子执行 `SET NX` + `PUBLISH`，24h 内重复 key 不再发布。
- 当前使用 Redis pub/sub，不是 durable stream：无订阅者或连接断开仍可能丢失投递；平台持久可靠性由 PostgreSQL outbox 提供。
- 不支持 `rediss://`/TLS、Streams、Consumer Group 或 cross-region failover。

## 5. S3-compatible

配置示例：

```json
{
  "endpoint": "https://objects.internal",
  "region": "us-east-1",
  "bucket": "hpa-artifacts",
  "allowedHosts": ["objects.internal"],
  "maxObjectBytes": 2097152,
  "timeoutMs": 15000
}
```

`credentialEnv` 是 JSON：

```json
{"accessKey":"...","secretKey":"...","sessionToken":"optional"}
```

- 手写 AWS Signature V4，path-style，固定 bucket；支持 put/get/head/list/delete。
- TLS/HTTP endpoint 必须显式 origin，无 userinfo/base path；解析到 link-local/metadata 地址会被拒绝。
- object 有 2 MiB 默认上限；响应不写日志或 audit 正文。
- 不支持 virtual-hosted style、multipart、presigned URL、版本/对象锁或 SigV4 chunked upload。
- 本地已用 MinIO（DaoCloud 镜像）完成 SigV4 put/head/get/list/delete 与 key traversal 负向测试。

## 6. 验证

```bash
HPA_POSTGRES_TEST_URL='postgresql://localhost:5432/hpa_p13_test' \
  cargo test -p hpa-local --test connectors_extended -- --nocapture
```

本地和 CI 额外启动 Redis 与 MinIO：

- PostgreSQL：真实建表/参数化插入/查询，拒绝 query 执行 DELETE。
- Git：真实 repository 的 status/log，拒绝 `../` 和未授权 commit。
- Redis：真实 subscribe 收到一次消息；重复 idempotency key 不再次发布；禁止 channel fail-closed。
- MinIO：真实 SigV4 对象往返、HMAC/SHA-256、list/delete 和 `../` key 拒绝。

## 7. 尚未完成

- 远程 Marketplace、发布者账号、公钥轮换、撤回传播与离线镜像。
- 包级 SBOM/CVE/provenance 与远程下载链路。
- DB 其他产品、rediss/Streams、S3 multipart/对象一致性策略。
- Connector 限流仍是单进程状态；多节点需要 Redis/PostgreSQL 共享限流。
