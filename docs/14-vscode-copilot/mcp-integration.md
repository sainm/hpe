# MCP 集成与工具

> 复用既有 Auralis MCP Gateway/Local MCP Server（Rust RMCP），不新增 MCP 核心。VS Code Extension 只注册/管理 MCP Server，Copilot 作为客户端调用。

## 1. 注册方式

### 推荐：VS Code MCP Provider

- `package.json`：`contributes.mcpServerDefinitionProviders` id `hpa-local`。
- `vscode.lm.registerMcpServerDefinitionProvider('hpa-local', ...)`。
- `provideMcpServerDefinitions` 返回 `McpHttpServerDefinition`（Streamable HTTP），URI 为 Local MCP Server。
- `resolveMcpServerDefinition` 获取/刷新短期 token，设置 `Authorization` header；可弹认证。
- Runtime 状态/配置变化 → `onDidChangeMcpServerDefinitions`。
- 不通过 `McpStdioServerDefinition` 启动任意脚本；除非官方 Auralis bridge 且用户明确授权。

### 兼容：`.vscode/mcp.json`

仅旧版/特殊情况；顶层 `servers`（不是 `mcpServers`），不写 token，使用 `inputs`/环境变量；写前用户确认 + 工作区信任。

## 2. Tool 契约

每个 Tool 必须有：

- 唯一 name（点号命名；见别名）。
- 明确 description（给模型）。
- inputSchema/outputSchema（JSON Schema 2020-12，root object）。
- `annotations`（readOnly/destructive/idempotent/openWorld）。
- permission（Auralis scope）。
- riskLevel（low/medium/high/critical）。
- approvalPolicy（never/policy/always）。
- tenant/workspace scope；数据脱敏/大小/分页。
- `ttlMs`/`cacheScope`（列表/只读）。
- 错误码/重试语义。

## 3. 基线 Tools

| Tool | 用途 | Permission | Risk | Approval |
| --- | --- | --- | --- | --- |
| `workflow.list` | 列出 Workflow/版本摘要 | `workflow:read` | low | never |
| `workflow.search` | 搜索 Workflow | `workflow:read` | low | never |
| `workflow.execute` | 执行已发布 Workflow | `workflow:execute` | high | always/policy |
| `workflow.get_execution` | 查询执行状态/摘要 | `execution:read` | low | never |
| `agent.run` | 启动 Agent Session | `agent:run` | high | always/policy |
| `agent.get_status` | 查询 Agent Session | `agent:read` | low | never |
| `skill.list` | 列出可用 Skill | `skill:read` | low | never |
| `skill.search` | 搜索 Skill | `skill:read` | low | never |
| `skill.get` | 获取 Skill 元数据/内容引用 | `skill:read` | low | never |
| `execution.get` | 获取执行详情 | `execution:read` | low | never |
| `execution.logs` | 获取执行/节点日志 | `execution:logs` | medium | never（按 scope） |
| `artifact.list` | 列出执行产物 | `artifact:read` | low | never |
| `artifact.get` | 读取产物内容/引用 | `artifact:read` | medium | never（按 scope） |

> 以上是基线，不是全部；工具由 Capability Registry 动态注册。高风险写操作必须服务端审批；客户端确认不能替代。

## 4. 输入/输出示例

### workflow.list

```json
{
  "name": "workflow.list",
  "description": "列出当前工作区可访问的已发布 Workflow 摘要",
  "inputSchema": {
    "type": "object",
    "properties": {
      "query": { "type": "string", "maxLength": 200 },
      "status": { "type": "string", "enum": ["published", "deprecated", "disabled"] },
      "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 20 },
      "cursor": { "type": "string" }
    },
    "additionalProperties": false
  },
  "outputSchema": {
    "type": "object",
    "properties": {
      "items": { "type": "array" },
      "nextCursor": { "type": ["string", "null"] }
    },
    "required": ["items"]
  },
  "annotations": { "readOnlyHint": true, "openWorldHint": false, "idempotentHint": true },
  "permissions": ["workflow:read"],
  "riskLevel": "low",
  "approval": "never",
  "ttlMs": 30000,
  "cacheScope": "private"
}
```

### workflow.execute

```json
{
  "name": "workflow.execute",
  "description": "执行已发布的 Workflow 版本，返回 executionId；高风险操作可能需要审批",
  "inputSchema": {
    "type": "object",
    "properties": {
      "workflowId": { "type": "string" },
      "version": { "type": "string" },
      "inputs": { "type": "object" },
      "idempotencyKey": { "type": "string", "maxLength": 200 }
    },
    "required": ["workflowId"],
    "additionalProperties": false
  },
  "outputSchema": {
    "type": "object",
    "properties": {
      "executionId": { "type": "string" },
      "status": { "type": "string" }
    },
    "required": ["executionId", "status"]
  },
  "annotations": { "readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": true },
  "permissions": ["workflow:execute"],
  "riskLevel": "high",
  "approval": "always"
}
```

### artifact.get

```json
{
  "name": "artifact.get",
  "description": "读取执行产物；大文件返回资源引用/下载 URL，不内联超大内容",
  "inputSchema": {
    "type": "object",
    "properties": {
      "artifactId": { "type": "string" },
      "encoding": { "type": "string", "enum": ["text", "download"], "default": "text" }
    },
    "required": ["artifactId"],
    "additionalProperties": false
  },
  "outputSchema": { "type": "object" },
  "annotations": { "readOnlyHint": true, "openWorldHint": false, "idempotentHint": true },
  "permissions": ["artifact:read"],
  "riskLevel": "medium",
  "approval": "never"
}
```

## 5. 工具名与别名

- 规范名使用点号（`workflow.list`），与 Auralis MCP Gateway/Capability Registry 一致。
- 若某客户端/工具链拒绝点号，MCP Server 可提供下划线别名（`workflow_list`），但只暴露一套，避免模型重复选择。
- 别名映射在 Rust MCP Server/Capability Registry 完成；扩展不改工具名。
- Description 面向模型，包含“何时用/何时不要用/审批/结果大小”。

## 6. 执行与审批

1. Copilot 调 `workflow.execute`/`agent.run`。
2. Auralis MCP Server 校验 token/tenant/scope/tool schema/risk。
3. 高风险 → 创建 Approval；返回 `executionId` + `status: WAITING_APPROVAL`（或 MCP `input_required` 的 Auralis 语义，按实现）。
4. 扩展在 Tree/Status 显示 waiting；提供打开 Auralis Approval Center 的深链。
5. 审批通过 → Runtime 执行；拒绝/超时 → 返回错误/状态。
6. Copilot/扩展用 `workflow.get_execution`/`execution.get` 轮询/读取结果；大结果用 Artifact。
7. 客户端提示不是审批；服务端审批不可绕过。

## 7. 认证/租户

- 本地：loopback + 短期 token（Local Runtime 签发）；MCP provider `resolve` 注入 header。
- 远程：OAuth 2.1/PAT；MCP Gateway 端点；token 绑定 tenant/workspace/scopes。
- 禁止 token passthrough；上游凭证由 Runtime Vault/Connector 注入。
- 所有工具调用带 tenant/workspace 上下文；跨租户拒绝并审计。
- Token 存 SecretStorage；不写 settings/workspace；不日志。

## 8. 结果/错误

| 情况 | 返回 |
| --- | --- |
| 成功 | `resultType: complete` + structuredContent/content |
| 需要审批 | `executionId`/`approvalId` + status（不阻塞） |
| 参数错误 | JSON-RPC `-32602` + fields |
| 权限/租户 | `-32003`/403；不泄漏内部 |
| 限流 | `-32000`/429 + retry-after |
| 超时 | tool error + retryable |
| 结果过大 | Artifact/Resource 引用；分页 |

- 日志/产物脱敏；不返回文件系统路径/secret；大对象分页。
- 错误包含 `code`/`message`/`retryable`/`traceId`/`remediation`（按 MCP 能力）。

## 9. 不做的 MCP 集成

- 不新写 MCP 协议/RMCP。
- 不把内部 REST 全量映射为工具。
- 不让扩展直接调用 Runtime 数据库/脚本。
- 不通过 Custom Instructions 绕过审批。
- 不假设 VS Code MCP 配置适配 Copilot CLI/Codex。

## 10. 验收

- Copilot 通过 MCP provider 发现基线 Tools（至少 workflow.list/execute/get_execution）。
- Tool schema 正确；参数校验/错误可理解。
- 高风险调用触发 Auralis 服务端审批；客户端不能绕过。
- 租户隔离/权限/限流/审计。
- 大结果/产物引用/分页。
- 本地/远程 provider 切换；断线/重连。
- 工具名在 VS Code 当前版本可用；别名不重复。

## 11. 相关

- [Copilot 集成](copilot-integration.md)
- [Local Runtime 桥接](local-runtime-bridge.md)
- [MCP 架构](../06-mcp/mcp-architecture.md)
- [MCP Server/Gateway](../06-mcp/mcp-server.md)
- [官方证据](evidence.md)
