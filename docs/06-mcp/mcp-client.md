# MCP Client

> Hisha MCP Client 连接外部 MCP Server，把工具/资源/Prompt 纳入 Agent/Workflow/Registry。实现为 Rust RMCP 3.8.0（2026-07-28 + 2025-11-25）；不依赖单一 TS SDK 行为。

## 1. 配置

```json
{
  "id": "mcp_orders",
  "name": "Orders MCP",
  "transport": "http",
  "url": "https://mcp.example.com/mcp",
  "auth": {
    "kind": "oauth2",
    "resourceMetadataUrl": "https://mcp.example.com/.well-known/oauth-protected-resource",
    "clientId": "hisha",
    "scopes": ["orders.read"]
  },
  "timeoutMs": 30000,
  "allowedTools": ["orders.lookup", "orders.refund"],
  "riskOverrides": { "orders.refund": "high" },
  "network": { "allow": ["mcp.example.com:443"] }
}
```

- stdio：
  ```json
  {
    "transport": "stdio",
    "command": "npx",
    "args": ["-y", "@example/mcp-server"],
    "env": { "EXAMPLE_TOKEN": "${vault:cred_example}" }
  }
  ```
- 凭证只存 Vault 引用；不写配置文件明文。
- 每个 Server 绑定 tenant/workspace；工具权限独立。

## 2. 传输与协议

| 传输 | 用途 | 说明 |
| --- | --- | --- |
| stdio | 本地进程 | 客户端启动；JSON-RPC stdin/stdout；env 凭证 |
| Streamable HTTP | 远程 | 2026-07-28：仅 POST、无会话、`_meta`、MRTR、`subscriptions/listen` |
| Streamable HTTP（legacy） | 旧服务器 | 2025-11-25 及更早：initialize/会话/GET/断线重放 |
| HTTP+SSE（legacy） | 更旧 | 仅兼容层 |

连接流程：

1. 尝试 `server/discover`（2026-07-28）。
2. 失败/旧服务器 → `initialize` + `notifications/initialized`。
3. 记录协商版本/能力；按版本选择调用语义。
4. 获取 tools/resources/prompts 列表；缓存 `ttlMs`/`cacheScope`。
5. 订阅变更（`subscriptions/listen` 或旧通知）。
6. 健康检查/重连/退避。

## 3. 认证

- OAuth 2.1：RFC 9728 资源元数据 → AS 发现（RFC 8414/OIDC）→ 授权码/PKCE → token；RFC 8707 resource；CIMD 优先/DCR 兼容；`iss` 校验。
- PAT/服务账号：CLI/无 OAuth 场景；短期/可撤销。
- stdio：环境变量/本地凭证；不走 OAuth。
- Token 存 Vault；不日志；不 passthrough 给上游 API。
- 过期/撤销：刷新/重新授权；失败明确错误。

## 4. 发现

- **Tools**：name/title/description/inputSchema/outputSchema/annotations；按租户/权限过滤。
- **Resources**：uri/name/mimeType/权限；读取分页/大小限制。
- **Prompts**：name/arguments；参数校验。
- 变更通知：列表变化/资源更新；缓存失效。
- schema 缓存：按 server/version；不跨租户复用。

## 5. 调用

```ts
interface McpCall {
  serverId: string;
  tool: string;
  arguments: unknown;
  timeoutMs: number;
  idempotencyKey?: string;
  traceContext: TraceContext;
}
```

- 参数 schema 校验；大小/深度/字符串长度限制；拒绝原型污染。
- 超时/取消；重试仅对幂等/明确可重试错误；退避。
- 结果：`resultType`/content/structuredContent；大小限制；脱敏。
- 高风险：Policy/Approval；审批后调用。
- 长任务：tasks 扩展/异步句柄；轮询/流式。
- 错误映射：认证/权限/限流/超时/上游/协议；不泄漏内部。

## 6. 健康与重连

- 连接状态：connected/degraded/disconnected；UI 显示。
- 心跳/健康检查；指数退避重连；熔断。
- 工具列表变更 → 刷新缓存/通知。
- 服务器不可用：工具调用失败；不影响其他 Server/Workflow 节点。
- 连接池/并发限制；按 server/tenant。

## 7. 本地 stdio

- 启动子进程；工作目录/环境最小化；继承策略显式。
- stdout 仅 JSON-RPC；stderr 日志；不混入协议。
- 进程退出/超时：终止进程树；重启次数限制。
- 凭证：Vault 短期注入；不落盘。
- 适用于本地 MCP Server/参考服务器测试。

## 8. 安全

- 域名/IP 白名单；禁止内网元数据地址；重定向校验；DNS rebinding 防护。
- 不信任工具描述/结果；schema/参数校验；提示注入防护。
- 工具风险/审批；租户/工作区隔离。
- 资源 URI 不暴露文件系统路径；按租户过滤。
- 审计：server/tool/参数摘要/结果/审批/trace。

## 9. 测试

- 2026-07-28 discover/无状态/MRTR/subscriptions/tasks。
- 2025-11-25 initialize/会话/GET/断线重放。
- OAuth 发现/CIMD/DCR/`iss`/resource。
- stdio 进程生命周期/stderr/退出。
- 超时/重试/取消/熔断。
- schema/大结果/分页。
- SSRF/路径/原型污染。
- 多租户/权限/审计。

## 10. 相关文档

- [MCP 架构](mcp-architecture.md)
- [MCP Server/Gateway](mcp-server.md)
- [MCP 安全](mcp-security.md)
- [标准研究](../01-research/standards/mcp-2026-07-28.md)
