# MCP 安全

> MCP 把内部能力暴露给外部模型/客户端，是高风险边界。安全必须在服务端强制，不能依赖客户端确认。

## 1. 威胁模型

| 威胁 | 场景 | 控制 |
| --- | --- | --- |
| 未授权工具调用 | 外部客户端拿 token 调高权限工具 | 工具级授权、风险/审批、租户 |
| Token passthrough | 用客户端 token 直接调上游 | 禁止；上游使用 Hisha Vault 凭证 |
| 跨租户读取 | 资源 URI/工具缓存串租户 | tenant 绑定、RLS、缓存键 |
| 工具滥用 | 删除/转账/发布/执行脚本 | 审批、限流、幂等、审计 |
| Prompt injection | 工具结果/资源内容操纵模型 | 不可信内容、allowlist、审批 |
| SSRF | 工具访问内网/元数据 | 域名/IP 白名单、重定向校验 |
| 供应链 | 恶意 MCP Server/工具 | 注册审核、签名、来源信任 |
| 资源耗尽 | 大参数/大结果/高频调用 | 大小/深度限制、限流、配额 |
| 协议降级 | 旧版漏洞/绕过 | 版本策略、旧版仅兼容、审计 |
| 审计缺失 | 无法追责 | 全链路审计/trace |

## 2. 认证

- OAuth 2.1：RFC 9728 资源元数据、RFC 8414/OIDC 发现、PKCE、RFC 8707 resource、CIMD/DCR、RFC 9207 `iss`。
- PAT/服务账号：短期/可撤销/最小 scope；用于 CLI。
- stdio：环境变量/本地能力令牌；不走 OAuth。
- Token 存 Vault；不日志；不进入模型上下文。
- 过期/撤销：立即拒绝；刷新/重新授权。

## 3. 授权

- Token → tenant/workspace/user/scopes；工具级检查。
- 风险分级：read/write/destructive/admin；写/管理默认审批。
- 资源级：只返回租户可见；URI 中的 tenant 必须匹配 token。
- 本地：能力令牌绑定工作区/工具/过期时间。
- 不因客户端 UI 确认而跳过服务端审批。

## 4. Token passthrough

- 禁止把客户端 token 转发给上游 API/工具。
- Hisha 用客户端 token 识别调用者，再用 Vault 中的上游凭证调用外部系统。
- 上游凭证按租户/连接器隔离；不共享。
- 代理/日志不记录 token。

## 5. 工具/资源安全

- 工具 schema 严格；参数大小/深度/字符串长度；拒绝未知字段（按策略）。
- 路径/URL 参数规范化；拒绝 `..`/协议走私/内网地址。
- 结果大小限制；大对象资源引用；不返回文件系统路径。
- 资源 URI 解析后校验 tenant/权限；不信任客户端提供的 tenant。
- 缓存键包含 tenant/tool/version；`cacheScope: private` 用于租户数据。

## 6. SSRF/网络

- 出站域名/IP 白名单；禁止 link-local/云元数据/回环（本地工具例外）。
- DNS 解析后校验 IP；防 DNS rebinding；重定向逐跳校验。
- HTTP 代理审计；超时/大小限制；禁止任意协议（file/gopher）。
- 工具执行环境网络默认关闭；仅声明的端点。

## 7. 审批

- 高风险：删除/发布/转账/执行脚本/数据导出/权限变更。
- 审批信息：调用者/租户、工具、参数摘要、影响、风险、trace。
- 审批人：角色/用户/组；SLA/升级；拒绝原因。
- 审批结果绑定 toolCallId；重复/过期处理。
- 即使客户端支持 elicitation/确认，Hisha 仍需服务端审批。

## 8. 限流/配额

- 按 tenant/client/tool/风险限流；并发/队列。
- 配额：调用数/成本/存储/执行时长。
- 429 + retry-after；熔断；防止 noisy neighbor。
- 大结果/长任务异步化；不阻塞连接。

## 9. 审计/监控

- 审计：client、tenant、user、tool、参数摘要、结果摘要、审批、trace、版本。
- 不记录 secret/完整凭证；参数敏感字段脱敏。
- 监控：错误率/延迟/调用量/审批等待/限流/跨租户拒绝。
- 告警：异常调用模式、越权尝试、SSRF、工具失败率。

## 10. 供应链/注册

- 外部 MCP Server 注册需审核/信任等级；工具描述/schema 审查。
- 本地 Server/参考服务器不作为生产默认。
- 版本/来源/完整性记录；禁用/下架。
- 不自动信任服务器声明的工具；Hisha 重新生成/校验 schema。

## 11. 协议安全

- 2026-07-28：无会话 → 每请求认证/授权；不依赖连接状态。
- Origin 校验；本地绑定 loopback；TLS。
- 旧版兼容层隔离；限制旧版能力；监控降级使用。
- 版本不支持 → 明确错误；不静默降级到不安全语义。

## 12. 事件响应

1. 撤销 token/禁用工具/下架 Server。
2. 隔离租户/执行。
3. 取证：审计/事件/trace/参数摘要。
4. 通知受影响方。
5. 修复/回归/更新威胁模型。

## 13. 测试

- 认证/授权/租户隔离/缓存串租户。
- Token passthrough/日志泄漏。
- SSRF/重定向/DNS rebinding/协议走私。
- 工具滥用/审批绕过/重复审批。
- 大参数/大结果/限流/熔断。
- Prompt injection/工具结果操纵。
- 协议版本/降级/Origin。
- 审计完整性。

## 14. 相关文档

- [MCP 架构](mcp-architecture.md)
- [MCP Server/Gateway](mcp-server.md)
- [MCP Client](mcp-client.md)
- [安全架构](../02-architecture/security-architecture.md)
- [标准研究](../01-research/standards/mcp-2026-07-28.md)
