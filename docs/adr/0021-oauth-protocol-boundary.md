# ADR-0021：OAuth 扩展由企业 IdP/网关承担

- 状态：Accepted（2026-11-01）
- 决策范围：HPA 内置 OAuth 2.1 Authorization Server

## 背景

HPA 内置 AS 已支持 Authorization Code + PKCE(S256)、DCR、同意、refresh token 轮换/复用检测、吊销、内省以及 EdDSA/ES256 JWKS。PAR、CIMD、DPoP、JARM 和 RS256 都会改变客户端注册、密钥生命周期或请求完整性边界；在没有真实企业客户端与 HSM/KMS 需求前，引入“半实现”会制造不可验证的安全承诺。

## 决策

1. HPA 内置 AS 明确只面向使用 Code+PKCE、ES256/EdDSA 的 MCP/OAuth 客户端。
2. 需要 PAR、CIMD、DPoP、JARM、RS256、证书绑定或复杂 federation 的部署，必须前接企业 OAuth/OIDC Gateway 或 IdP。
3. HPA 不把未知 `request_uri`、DPoP header、JARM response mode 静默降级为普通 Authorization Code；相关请求 fail-closed。
4. 内置 AS 不签发 ID Token，不做 UserInfo；用户认证继续由 Browser SSO IdP 或企业网关完成。
5. 改变此边界前必须有新 ADR、正式 capability advertisement、HSM/KMS 方案、负向矩阵和 conformance 证据。

## 结果

- 当前支持矩阵准确，不虚假宣告 PAR/DPoP/JARM。
- 企业集成通过成熟网关获得高级协议能力。
- 真实网关仍是 `BLOCKED_EXTERNAL`，直到提供配置与客户端凭证。
