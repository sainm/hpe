# MCP Server 与 Gateway

> Gateway 是 Hisha 对外的 MCP 入口；Local MCP Server 是本地运行的同一能力子集。实现为 Rust RMCP 3.8.0；TS Control Plane 只做注册/权限/审计管理。

## 1. Gateway 端点

- 远端：`https://hpa.example.com/mcp`（token 识别 tenant）或 `https://<tenant>.hpa.example.com/mcp`。
- 本地：`http://127.0.0.1:<port>/mcp` + 能力令牌。
- stdio：本地进程，无 HTTP。
- 仅 POST（2026-07-28）；旧版兼容层可有 GET/SSE。

## 2. 协议模式

### 2026-07-28（默认）

- 无 `initialize`/会话；每个请求带 `_meta`：
  ```json
  {
    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
    "io.modelcontextprotocol/clientInfo": { "name": "...", "version": "..." },
    "io.modelcontextprotocol/clientCapabilities": {}
  }
  ```
- 必须实现 `server/discover`；返回 supportedVersions/capabilities/serverInfo、`resultType: "complete"`、缓存字段。
- 所有结果带 `resultType`；MRTR 用 `input_required` + `inputRequests`。
- 无 GET 流/会话/断线重放；变更通知走 `subscriptions/listen`。
- 请求头 `Mcp-Method`/`Mcp-Name`；Origin 校验；本地绑定 loopback。

### 2025-11-25 及更早（兼容）

- `initialize`/`notifications/initialized`；`Mcp-Session-Id`；GET/SSE；断线重放。
- 仅用于旧客户端/旧服务器；会话状态在兼容层，不进入核心业务模型。
- 能力受限（无 discover/MRTR/resultType）；降级语义明确。

### 检测

```ts
function detectMode(req: McpRequest): "modern" | "legacy" | "unsupported" {
  if (req.body?.method === "initialize") return "legacy";
  if (req.meta?.protocolVersion === "2026-07-28") return "modern";
  return "unsupported";
}
```

- 现代请求不得依赖连接状态；同一连接可混合（不建议），按请求判定。
- 不支持的版本 → 明确错误 + supportedVersions。

## 3. 工具注册

工具来自 Capability Registry，不手写内部 REST 映射。

```json
{
  "name": "workflow.execute",
  "title": "执行 Workflow",
  "description": "执行已发布的 Workflow 版本，返回 executionId",
  "inputSchema": {
    "type": "object",
    "properties": {
      "workflowId": { "type": "string" },
      "version": { "type": "string" },
      "inputs": { "type": "object" }
    },
    "required": ["workflowId", "version"],
    "additionalProperties": false
  },
  "outputSchema": { "type": "object", "properties": { "executionId": { "type": "string" } } },
  "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true },
  "risk": "high",
  "approval": "always",
  "permissions": ["workflow:execute"],
  "ttlMs": 30000,
  "cacheScope": "private"
}
```

- 名称稳定、点号命名；不暴露内部 API 路径。
- 描述面向模型；schema 严格；参数大小/深度限制。
- 列表顺序稳定（便于缓存/提示缓存）。
- 高风险写操作：`approval: always` 或策略；服务端强制。

## 4. 工具调用

```json
{
  "jsonrpc": "2.0",
  "id": "call-1",
  "method": "tools/call",
  "params": {
    "name": "workflow.execute",
    "arguments": { "workflowId": "wf_...", "version": "1.2.0", "inputs": {} },
    "_meta": {
      "io.modelcontextprotocol/protocolVersion": "2026-07-28",
      "io.modelcontextprotocol/clientInfo": { "name": "Claude Code", "version": "..." },
      "io.modelcontextprotocol/clientCapabilities": {},
      "traceparent": "00-..."
    }
  }
}
```

结果：

- 普通：`resultType: "complete"` + `content`（text/resource）+ `structuredContent`（可选）。
- 需要输入（MRTR）：`resultType: "input_required"` + `inputRequests` + 重试指令。
- 长任务：tasks 扩展或 Hisha 异步句柄；返回 `executionId`/`taskId`；客户端轮询/流式。
- 审批：返回 waiting 句柄 + 审批链接；不在协议层阻塞；审批完成后可查询。
- 错误：JSON-RPC error + data（code/retryable/traceId）；不泄漏内部。

## 5. Resources

```text
hpa://tenant/{tenantId}/executions/{executionId}
hpa://tenant/{tenantId}/artifacts/{artifactId}
hpa://tenant/{tenantId}/workflows/{workflowId}/versions/{version}
hpa://tenant/{tenantId}/skills/{skillId}/versions/{version}
hpa://tenant/{tenantId}/projects/{projectId}
hpa://tenant/{tenantId}/knowledge/{knowledgeId}
```

- 读取前校验 tenant/workspace/权限；跨租户拒绝。
- 大对象分页/范围；不返回文件系统路径。
- 订阅：`resourceSubscriptions`（2026-07-28）或旧 `resources/subscribe`。
- 缓存：`ttlMs`/`cacheScope`；敏感资源 `private`。

## 6. Prompts

- 模板：name/description/arguments schema；版本化。
- 示例：System Investigation、Code Review、Data Analysis、Document Generation、Test Planning。
- Prompt 中可引用 Hisha 资源/工具，但不得包含 secret；参数需校验/转义。
- 租户/工作区可见性；公共模板只读。

## 7. 认证/授权

- OAuth 2.1（2026-07-28 规范）：资源元数据、AS 发现、PKCE、resource indicators、CIMD/DCR、`iss` 校验。
- PAT/服务账号：CLI/无 OAuth；短期/可撤销。
- Token → tenant/workspace/user/scopes；工具级授权；高风险审批。
- 不 token passthrough；上游凭证由 Hisha Vault 注入。
- 本地 stdio：能力令牌；文件权限 0600；可撤销。

## 8. 租户隔离

- 每个请求解析 tenant；工具/资源查询强制 tenant_id；RLS 兜底。
- 列表只返回租户可见工具/资源；不泄漏其他租户名称/ID。
- 缓存键包含 tenant/client/tool/version；防止跨租户命中。
- 审计 tenant/client/tool/approval/trace。

## 9. 限流/配额/审计

- 按 tenant/client/tool/工具风险限流；配额/并发；429 + retry-after。
- 熔断：上游故障不拖垮 Gateway。
- 审计：谁（client/user）、何时、哪个工具/资源、参数摘要、结果摘要、审批、trace；不记录 secret。
- 指标：调用量/错误率/延迟/审批等待/限流命中。

## 10. Local MCP Server

- stdio 优先；HTTP loopback + 能力令牌。
- 本地 Registry/工具/资源/Prompt；离线可用。
- 支持 2026-07-28 discover；旧客户端兼容 initialize。
- 工具默认只读；写操作需本地审批。
- 与云端 Gateway 共享工具定义，但租户=本地工作区。

## 11. 部署

- Gateway 无状态（2026-07-28）可水平扩展；旧版会话存 Redis（兼容层）。
- Postgres：工具/资源/Prompt 元数据、审计；对象存储：大资源。
- 本地：单进程；SQLite/文件。
- 缓存：Redis/内存；按 tenant/tool/version。
- 安全：WAF/边缘限流；TLS；Origin 校验。

## 12. 测试/Conformance

- 2026-07-28：discover、无状态、`_meta`、resultType、MRTR、subscriptions、tasks、缓存字段、错误码。
- 旧版：initialize/会话/GET/断线重放。
- OAuth：发现/CIMD/DCR/resource/iss/PKCE。
- 工具 schema/大结果/分页/错误。
- 租户隔离/权限/审批/限流/审计。
- 真实客户端：Claude Code、VS Code/Copilot、Codex（PoC B）。

## 13. 相关文档

- [MCP 架构](mcp-architecture.md)
- [MCP Client](mcp-client.md)
- [MCP 安全](mcp-security.md)
- [外部 AI 集成](external-ai-integration.md)
- [标准研究](../01-research/standards/mcp-2026-07-28.md)
