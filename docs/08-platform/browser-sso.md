# Phase 9 Browser SSO：OIDC Authorization Code + PKCE 契约

> 本地 Enterprise 控制面的浏览器登录。它不改变现有 Bearer API：本地匿名模式、`hpa_ent_`/OIDC Bearer、Worker、VS Code 集成继续可用。浏览器会话是额外的 Cookie 认证路径。

## 1. 范围与边界

- OIDC Authorization Code + PKCE（S256），public client，不使用 client secret。
- `state`、`nonce`、`redirect`、`iss`、`aud`、`exp`、`nbf`、JWKS `kid` 校验。
- ID Token 支持 EdDSA、RS256、ES256；JWKS 支持 OKP/Ed25519、RSA、EC/P-256。
- 服务端 session，HttpOnly Cookie，`SameSite=Lax`，可选 `Secure`；服务端只存 session token 哈希。
- refresh token 与 ID token 使用安装级 AES-256-GCM 加密落库；access token 原文不落库，只保存到期时间。
- 支持 refresh token 轮换、静默续期、会话 token 再生成与 RP-initiated logout；旧 Cookie 在刷新后立即失效。
- Cookie 认证的非 GET 请求必须带 `X-HPA-CSRF`；Bearer 请求不需要 CSRF。
- 成员目录联动：disabled 成员拒绝登录，active 成员的角色覆盖 claims；tenant 必须 active。
- 平台管理员 allowlist（`adminSubjects`）决定浏览器 session 能否访问 `/api/enterprise/*`；OIDC claims 中的 `platform_admin` 不被信任。
- Browser SSO 端点本身不做 SCIM/组同步；独立 SCIM 契约见 [SCIM](scim.md)。不做授权服务器、第三方 Cookie、云 SaaS；PAR/CIMD/DPoP/JARM 由企业 IdP/Gateway 边界承担，见 ADR-0021。

## 2. OIDC 配置扩展

现有 `enterprise_oidc` 增加：

| 字段 | 说明 |
| --- | --- |
| `clientId` | OIDC public client ID |
| `authorizationEndpoint` | 授权端点，绝对 URL |
| `tokenEndpoint` | token 端点，绝对 URL |
| `redirectUri` | 必须指向本服务 `/auth/oidc/callback` |
| `scopes` | 字符串数组，默认 `["openid","email","profile"]`；兼容接收空格分隔字符串 |
| `endSessionEndpoint` | IdP RP-initiated logout 端点；与 `postLogoutRedirectUri` 成对配置 |
| `postLogoutRedirectUri` | 本服务 `/auth/oidc/logout/callback`，必须与登录 redirect 同源 |
| `adminSubjects` | `sub` 或 email 白名单，允许进入平台控制面 |
| `cookieSecure` | HTTPS 部署置 true；loopback HTTP 可为 false |
| `sessionTtlSeconds` | 默认 28800（8h），范围 300–604800 |

`issuer`、`audience`、`jwksUrl`、`network` 沿用。`audience` 必须与 ID Token 的 `aud` 匹配（通常等于 clientId）。authorization/token/JWKS 的非回环 URL 必须 HTTPS 且在 `network` 中；禁止 userinfo；不跟随重定向、不使用系统代理。配置读取与保存都返回 `scopes` 数组。

## 3. 登录流程

### 3.1 开始

`GET /auth/oidc/login?redirect=/workflows`

1. 校验 `redirect`：只允许以单个 `/` 开头、非 `//`、无反斜杠与控制字符；缺省为 `/`，非法值返回 `/login?error=redirect_invalid`，不会跳转到外部站点。
2. 生成 `state`、`nonce`、`code_verifier`（32 字节随机，base64url）；`code_challenge = base64url(SHA-256(verifier))`。
3. 在 control.db 保存 flow：`state_hash`、verifier（明文存储；加密不是本阶段能力）、nonce、redirect、created/expires（10 分钟）。
4. 302 到 authorization endpoint，参数：`response_type=code`、`client_id`、`redirect_uri`、`scope`、`state`、`nonce`、`code_challenge`、`code_challenge_method=S256`。

未配置或配置不完整时重定向 `/login?error=sso_unavailable`。

### 3.2 回调

`GET /auth/oidc/callback?code=...&state=...`

1. 按 `state_hash` 取 flow，一次性消费；过期/不存在 → `/login?error=state_invalid`。IdP `error` 参数也重定向 `/login?error=<safe-code>`。
2. POST token endpoint（form）：`grant_type=authorization_code`、`code`、`redirect_uri`、`client_id`、`code_verifier`；不发送 client secret。
3. 响应必须有 Bearer `access_token`、合法正整数 `expires_in` 与 `id_token`；建议 scope 包含 `offline_access` 以获得 `refresh_token`。
4. 校验 JWT：算法白名单、kid、JWKS、iss/aud/exp/nbf/nonce。
5. claims：`sub`、`tenant_id`/`tenantId`、`role`/`roles`；可选 `email`、`name`。
6. tenant active；若成员目录命中 subject/email：disabled → `/login?error=member_disabled`，active 使用目录 role/displayName；未命中使用 claims role。
7. `platform_admin = adminSubjects` 命中 sub/email（email 大小写不敏感）。
8. 创建 session，设置 Cookie，302 回安全 redirect。refresh token 与 ID token 以 AES-256-GCM 加密保存，access token 到期时间单独保存；密钥位于 `<data_dir>/keys/browser-auth.key`，权限必须为 `0600`。
9. token 交换或 ID Token 校验失败返回 `/login?error=sso_failed`；tenant 非 active 返回 `tenant_inactive`。

### 3.3 会话与登出

`GET /auth/session`（公开；前端调用同义的 `/api/auth/session`）返回：

```json
{
  "authenticated": true,
  "ssoConfigured": true,
  "mode": "enterprise",
  "principal": {
    "tenantId": "local",
    "role": "owner",
    "platformAdmin": true,
    "subject": "oidc|admin",
    "email": "admin@example.com",
    "displayName": "Admin"
  },
  "csrfToken": "<opaque>",
  "expiresAt": 1793520000000,
  "accessTokenExpiresAt": 1793520000000,
  "refreshAvailable": true
}
```

未登录返回 `authenticated:false`、`ssoConfigured`、`mode`（`local`/`enterprise`）与 `csrfToken:null`。Cookie 名为 `hpa_session`，HttpOnly、SameSite=Lax、Path=/、Max-Age=TTL，HTTPS 时 Secure。过期/撤销的 Cookie 会在该响应中被清除。

`POST /auth/refresh`（前端调用 `/api/auth/refresh`）：需要有效 Cookie + `X-HPA-CSRF`。服务端解密 refresh token，调用 token endpoint 的 `grant_type=refresh_token`；成功时轮换浏览器 session token、更新 Cookie、重新校验 tenant/member，并返回新的 `expiresAt`、`accessTokenExpiresAt`、`refreshAvailable`。同一 session 的 refresh version 使用 CAS；并发重复刷新或上游拒绝 `invalid_grant` 时整条 session 被撤销，旧身份不能继续使用。Web 在 access token 到期前 60 秒触发静默续期。

`POST /auth/logout`（前端调用 `/api/auth/logout`）：需要 Cookie + `X-HPA-CSRF`；本地 session 总是先撤销并清 Cookie。若配置了 end-session endpoint，响应额外返回经服务端构造的 `logoutUrl`，包含 `id_token_hint`、`client_id`、`post_logout_redirect_uri` 与一次性 logout `state`；前端跳转到 IdP 后，再经 `/auth/oidc/logout/callback` 校验 state 并回到 `/login?loggedOut=1`。

## 4. API 认证

`EnterpriseState.authenticate(headers)` 顺序：

1. `Authorization: Bearer`：现有 `hpa_ent_` 与 OIDC Bearer（不要求 CSRF）。
2. 无 Bearer 且有 `hpa_session` Cookie：查 session（未过期/未撤销），tenant active，成员目录 disabled 拒绝；返回 Principal。
3. 无凭证：本地模式匿名 local owner；企业模式 401。

Cookie 认证的 `POST`/`PUT`/`PATCH`/`DELETE` 必须带 `X-HPA-CSRF`，由企业路由中间件校验 `/api/*`；缺失/失配返回 403 `CSRF_REQUIRED`。Bearer 认证跳过 CSRF。`/auth/logout` 与 `/api/auth/logout` 在 handler 内再次校验。控制面 `/api/enterprise/*` 需要 session principal 的 `platformAdmin=true`；普通租户成员的 session 仍可按其 role 使用业务 API，但不能操作控制面。无效/过期 Cookie 按无凭证处理：企业模式 401，本地模式回落到匿名 local owner。

## 5. Session/Flow 存储（control.db）

```text
enterprise_auth_flows:
  state_hash TEXT PRIMARY KEY, code_verifier TEXT, nonce TEXT, redirect TEXT,
  created_at INTEGER, expires_at INTEGER

enterprise_sessions:
  id TEXT PRIMARY KEY, token_hash TEXT UNIQUE, csrf_token TEXT, tenant_id TEXT,
  subject TEXT, email TEXT, display_name TEXT, role TEXT, platform_admin INTEGER,
  created_at INTEGER, expires_at INTEGER, revoked_at INTEGER, last_seen_at INTEGER,
  refresh_token_ciphertext TEXT, refresh_token_version INTEGER,
  refresh_token_expires_at INTEGER, id_token_ciphertext TEXT,
  access_token_expires_at INTEGER, last_refresh_at INTEGER

enterprise_logout_flows:
  state_hash TEXT PRIMARY KEY, created_at INTEGER, expires_at INTEGER
```

- session token 只存 SHA-256；csrf token 存明文本（非认证凭证），只通过 `/auth/session` 发给同源页面。
- refresh token 与 ID token 使用 AES-256-GCM、随机 96-bit nonce 和 session/AAD 绑定后落库；不得导出到前端、日志或审计详情。
- access token 原文不保存；当前浏览器认证仍使用 ID Token 身份，不调用 UserInfo。
- 每次会话查询更新 `last_seen_at`；过期/撤销行不被接受。
- session TTL 默认 8h；刷新可续期至配置的 TTL，但不会超过 refresh token 到期时间。
- 安装密钥必须与应用数据分开备份；丢失密钥会使 refresh/ID token 无法解密，但不会绕过 session 撤销。生产环境应把相同边界映射到 KMS/Vault。
- 过期/撤销 session 不被接受；登录/登出写审计（`auth.login`/`auth.logout`，不带 token/cookie）；失败的登录也写 `auth.login` deny 与稳定错误码。
- 登录失败不区分“用户不存在/角色无效”等细节给浏览器，只给稳定错误码；服务端日志可记原因但不含 token。

## 6. 前端

- 应用启动 GET `/auth/session`；认证成功时把 CSRF token 注入 API 客户端，所有非 GET 请求加 `X-HPA-CSRF`；Cookie 由同源浏览器自动发送。
- `/login` 页面：SSO 按钮跳到 `/auth/oidc/login?redirect=<当前路径>`；显示 `sso_unavailable`/`state_invalid`/`redirect_invalid`/`member_disabled`/`tenant_inactive`/`access_denied`/`sso_failed` 等错误；本地模式/已有 Bearer 不强制登录。
- 已登录时显示用户/租户/角色/退出；企业控制页面只有 `platformAdmin` 可操作控制面；普通租户成员仍可使用其业务 UI。
- `/login` 与 Bearer token 登录并存；Bearer 仍存 localStorage（原有行为），浏览器 session 不写 localStorage。
- 登录页与用户菜单 ja/en/zh；1440/607 无横向溢出。
- API 客户端不读取 HttpOnly Cookie；不把 session token 写入 localStorage；CSRF 只在内存。

## 7. 安全测试

Rust 测试：

```bash
cargo test -p hpa-local --bin hpa          # oidc_verify + browser_auth 单元测试
cargo test -p hpa-local --test browser_sso # HTTP 登录/Cookie/CSRF/失败路径
cargo test -p hpa-local --test enterprise
cargo test -p hpa-local --test enterprise_ops
```

覆盖：

- PKCE：S256 RFC 向量；verifier/challenge 不匹配、缺 code_verifier、重复 code 失败。
- state：随机、一次性、过期、未知 state 失败；redirect 逃逸（`//evil`、`https://`、`/\evil`）拒绝。
- nonce 不匹配、iss/aud/exp/nbf/kid/alg 失败；RS256/ES256/EdDSA 正例与错签名负例。
- JWKS URL/authorization/token endpoint 的 HTTPS/allowlist/SSRF；不跟随重定向。
- Cookie：HttpOnly/SameSite/Path/Secure 配置；session token 哈希；登出/过期/撤销；旧 Cookie 登出后 API 401。
- Refresh：token 轮换、Cookie 再生成、旧 Cookie 失效、上游拒绝后撤销 session、refresh version CAS。
- Token at rest：明文不得出现在 SQLite；AES-GCM 篡改与 AAD 错配必须失败；密钥文件权限为 `0600`。
- RP logout：`id_token_hint`、client_id、redirect 同源/路径校验、一次性 state、state 重放拒绝、Secure Cookie 清理。
- CSRF：Cookie 写操作缺头 403；Bearer 写操作不受影响；登出缺少 CSRF 拒绝。
- 成员 disabled、tenant disabled、admin allowlist、redirect 逃逸与跨租户越权。
- 本地模式匿名、现有 Bearer/Worker/VS Code 不回归。

浏览器验收：

```bash
OIDC_PORT=18970 OIDC_ALG=EdDSA node scripts/oidc-mock.mjs
HPA_TOKEN_FILE=/tmp/hpa-final-ent.token OIDC_MOCK_PORT=18970 node scripts/phase9-sso-e2e.mjs
```

`scripts/phase9-sso-e2e.mjs` 使用 Playwright 完成 `/login` → mock 授权 → 回调 → `/enterprise` → refresh → RP logout → 回调 → 登录页，检查 Cookie、session principal、平台管理员、回调错误与 1440/607 截图；`OIDC_ALG` 可切换 `EdDSA`/`RS256`/`ES256`。

## 8. 截图

- `docs/assets/phase9/login-1440.png`、`login-607.png`：未登录的 `/login`。
- `docs/assets/phase9/enterprise-sso-1440.png`、`enterprise-sso-607.png`：SSO 登录后的 Enterprise Center 与用户菜单。
- `docs/assets/phase9/oidc-config-1440.png`、`oidc-config-607.png`：OIDC 配置与当前 session 面板。
- `docs/assets/phase9/callback-error-1440.png`：无效 state 回调返回 `/login?error=state_invalid`。

## 9. 非目标

- `client_secret`、PKCE downgrade/plain、第三方 Cookie、SameSite=None、多 IdP、云 SaaS。用户/组目录同步使用独立 [SCIM](scim.md) 端点。
- IdP session 是否立即全局退出取决于 IdP 对 `end_session_endpoint` 的实现；本地 session 撤销不依赖该跳转成功。
- 浏览器 session 不改变 API Bearer token 或 Worker token 语义。
