# 内置 OAuth 2.1 授权服务器（Phase 10）

> 状态：本地实现完成（2026-11-01）。范围是 **授权服务器**，不是完整 IdP：不签发 ID Token、不做用户注册、不做密码登录。
> 目标：让 MCP 客户端用标准 OAuth 拿到访问令牌，不再只能依赖 PAT。

## 1. 为什么需要它

Phase 3 的 MCP Gateway 只接受本地 PAT / stdio 能力令牌，`.well-known/oauth-protected-resource` 里也没有 `authorization_servers`。这带来两个实际后果：

- 支持 OAuth 的 MCP 客户端（Claude Code、Copilot、Codex）拿到 401 后无法自动完成授权，只能让用户手工粘贴令牌。
- 浏览器 SSO 依赖外部 IdP；没有 IdP 的机器上无法完成任何“以人的身份授权第三方客户端”的动作。

Phase 10 在本地运行时内置一个授权服务器：平台自己当 AS，MCP Gateway 当 Resource Server。

## 2. 端点

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/.well-known/oauth-authorization-server` | AS 元数据（RFC 8414） |
| GET | `/oauth/jwks` | 签名公钥集，含 24h 轮换重叠期 |
| GET | `/oauth/authorize` | 授权端点；未登录时跳 `/login`，需要同意时跳 `/consent` |
| POST | `/oauth/register` | 动态客户端注册（RFC 7591） |
| POST | `/oauth/token` | `authorization_code` + `refresh_token` |
| POST | `/oauth/revoke` | 吊销（RFC 7009），刷新令牌吊销会带走整条令牌链 |
| POST | `/oauth/introspect` | 内省（RFC 7662） |
| GET | `/api/oauth/requests/{id}` | 同意页读取待批准请求 |
| POST | `/api/oauth/requests/{id}/decision` | 允许 / 拒绝 |
| GET | `/api/enterprise/oauth/overview` | 控制台概览 |
| GET/POST | `/api/enterprise/oauth/clients` | 列出 / 手工注册客户端 |
| DELETE | `/api/enterprise/oauth/clients/{id}` | 停用客户端并吊销其全部令牌与同意记录 |
| POST | `/api/enterprise/oauth/clients/{id}/secret` | 轮换客户端密钥（仅机密客户端） |
| GET | `/api/enterprise/oauth/tokens` | 已签发令牌 |
| DELETE | `/api/enterprise/oauth/tokens/{id}` | 按令牌吊销整条链 |
| POST | `/api/enterprise/oauth/keys/rotate` | 轮换签名密钥 |

管理接口走平台管理员鉴权：`HPA_ENTERPRISE_MODE=1` 时需要 Bearer 或浏览器会话；本地模式等价于 `local` 租户 owner。

## 3. 协议细节

### 3.1 授权码 + PKCE

- `response_type=code` 唯一支持；`code_challenge_method=S256` 强制，`plain` 直接拒绝。
- `code_challenge` 43–128 字符，`code_verifier` 同样长度区间，校验方式为 `BASE64URL(SHA256(verifier)) == challenge`。
- 授权码 120 秒有效、一次性消费（`UPDATE ... WHERE consumed_at IS NULL` 保证并发下只有一个请求成功）。
- `redirect_uri` **精确字符串匹配**客户端注册值；未匹配时不重定向（防止开放重定向），直接返回 400 错误页。
- 授权响应回传 `state`；`error` / `error_description` 同样带回 `state`。

### 3.2 客户端

| 类型 | `token_endpoint_auth_method` | 凭证 |
| --- | --- | --- |
| 公共（MCP 客户端、CLI） | `none` | 仅 PKCE |
| 机密 | `client_secret_post` / `client_secret_basic` | `hpa_cs_…`，DB 只存 SHA-256 |

- DCR 与手工注册共用同一套校验：名称 1–120 字符、1–8 个重定向 URI、`https` 或回环 `http`、禁止 fragment 与 userinfo。
- 客户端密钥只在创建/轮换响应里出现一次；DB 不保存明文。
- 停用客户端会同时吊销其全部访问/刷新令牌并删除同意记录。

### 3.3 令牌

- 访问令牌是 JWT：`typ=at+jwt`、`iss=<base_url>`、`aud=<base_url>/mcp`、`sub`、`tenant_id`、`role`、`client_id`、`scope`、`jti`、`iat`、`exp`（3600s）。
- 签名算法默认 **ES256**（ring 生成 P-256，PKCS#8 存 `control.db`），可用 `HPA_OAUTH_SIGNING_ALG=EdDSA` 切换为 Ed25519。
- 刷新令牌是不透明串 `hpa_rt_…`，30 天有效，**每次使用即轮换**；检测到旧令牌复用会吊销整条 family（`refresh_token_reuse`）。
- 每个访问令牌都落库（`jti` 唯一索引），校验时同时验签与查吊销，因此吊销立即生效，不依赖短 TTL。
- `offline_access` 只影响是否在同意页单独展示，刷新令牌本身总是签发。

### 3.4 同意与资源所有者

- 有浏览器会话时，资源所有者 = 会话主体（`session:{id}`）；无会话且 `HPA_ENTERPRISE_MODE` 未开启时，资源所有者 = 本地匿名 owner（`local-owner` / `local` 租户 / `owner`）。
- 无会话且强制企业模式时，`/oauth/authorize` 302 到 `/login?redirect=<原始请求>`，登录后回到授权流程。
- 首次授权展示同意页（`/consent`）列出客户端名、回调主机、权限中文/日文/英文说明。
- 勾选“记住该客户端”后写入 `oauth_consents`，下次同一 client/tenant/subject 且 scope 被覆盖时直接签发授权码；`prompt=consent` 可强制重新询问。
- 同意写入与授权码签发在同一个请求里完成，待批准请求一次性消费（`consumed_at`），重复提交返回 400。

### 3.5 Resource Server 侧（MCP Gateway）

- `/.well-known/oauth-protected-resource` 增加 `authorization_servers: ["<base_url>"]`。
- `/mcp` 鉴权顺序：本地 PAT → AS 访问令牌 → 401。AS 令牌必须是三段 JWT、`aud == <base_url>/mcp`、租户为 `local`（与 PAT 相同的单租户约束）。
- `AuthContext.token_id` 形如 `oauth:<client_id>:<jti>`，审计与限流按客户端隔离。

## 4. 数据表（`control.db`）

| 表 | 用途 |
| --- | --- |
| `oauth_clients` | 客户端注册、密钥哈希、启停、最后使用时间 |
| `oauth_signing_keys` | 签名密钥（算法、PKCS#8/种子、公钥 JWK、退役/删除时间） |
| `oauth_requests` | 待批准的授权请求（绑定 client/redirect/scope/PKCE/owner） |
| `oauth_codes` | 授权码（哈希存储、一次性、120s） |
| `oauth_tokens` | 访问/刷新令牌（哈希、`jti`、family、吊销原因、轮换目标） |
| `oauth_consents` | 记住的授权（client + tenant + subject + scope） |

启动时执行一次惰性清理：删除过期 1 小时以上的授权码与请求，删除退役超过 24h 的签名密钥。

## 5. 界面

- `/consent`：授权同意页。显示客户端、回调主机与完整 URI、逐项权限说明、当前身份（或“本地匿名所有者”）、记住开关、允许/拒绝。607px 下单列铺开。
- Enterprise Center → **OAuth 2.1**：
  - 左栏：issuer/resource/JWKS/DCR 端点（可复制）、签名算法徽标、密钥轮换、客户端列表（状态/认证方式/来源/scope/最后使用）、可折叠的注册表单。
  - 右栏：已签发令牌（access/refresh、scope、到期时间、单键吊销）。
  - 客户端密钥与轮换结果只在页面顶部提示条显示一次。

## 6. 验收

```bash
cargo test -p hpa-local --bin hpa -- oauth::           # 4 个校验单元测试
cargo test -p hpa-local --test oauth_server            # 4 个 HTTP 端到端测试
HPA_BASE_URL=http://127.0.0.1:8787 node scripts/phase10-oauth-e2e.mjs
```

`tests/oauth_server.rs` 覆盖：

- DCR → 授权（302 `/consent`）→ 同意 → 换令牌 → `/mcp` `server/discover` 成功 → 内省 active → 刷新轮换 → 吊销后 401。
- 未注册 `redirect_uri` 返回 400 且不重定向；缺 PKCE / `plain` 方法 / 未注册 scope 都以 `error` 回到客户端并保留 `state`。
- 错误 `code_verifier` 与授权码重放均 `invalid_grant`；刷新令牌复用触发整链吊销，最新的访问令牌随之失效。
- 记住同意后跳过同意页；`prompt=consent` 强制重问；拒绝返回 `error=access_denied`。
- 管理员接口：手工注册机密客户端、概览、密钥轮换后 JWKS 保留旧键、错误密钥 401、停用后不能轮换密钥。

`scripts/phase10-oauth-e2e.mjs` 用 Playwright 跑真实浏览器：注册 → 授权页 → 同意页（1440/607 截图）→ 回调收码 → 换令牌 → `/mcp` 200 → 控制台列出客户端；同时断言两个页面在 1440 与 607 都没有横向溢出。

## 7. 明确不做

- 不签发 ID Token，不实现 `/userinfo`、`openid` scope、登录会话联邦。
- 不实现 CIMD（客户端元数据文档）、PAR、JARM、DPoP、mTLS 绑定、token exchange。
- 不实现 RS256 签名（ring 不能生成 RSA 私钥；需要新增依赖，本阶段未引入）。
- 不支持第三方 Cookie、`SameSite=None`、静默续期（`prompt=none`）。
- 单节点 SQLite：令牌表不跨节点共享，多实例部署时吊销状态不同步。
- 撤销列表是同步 DB 查询，不是分布式缓存；高 QPS 场景需要外部存储。
- 本地模式允许匿名 owner 完成授权，端口暴露给他人等同于交出授权能力——与 Phase 6 的边界一致。
