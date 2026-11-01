# 节点规范

> 节点是 Workflow 的执行步骤。节点定义是平台契约：类型、版本、schema、权限、运行时、文档必须齐备；缺一不可发布。

## 1. 节点定义

```ts
interface NodeDefinition {
  type: string;                 // 例：http.request
  typeVersion: string;          // semver
  displayName: string;
  description: string;
  category: "trigger" | "control" | "data" | "integration" | "ai" | "human" | "system";
  icon?: string;
  keywords?: string[];
  configSchema: JsonSchema;
  inputSchema: JsonSchema;
  outputSchema: JsonSchema;
  ports: PortDefinition[];
  permissions: PermissionRequest[];
  defaults?: { timeoutMs?: number; retry?: RetryPolicy; errorPolicy?: ErrorPolicy };
  riskLevel: "low" | "medium" | "high" | "critical";
  sideEffects: "none" | "read" | "write" | "external";
  idempotency: "supported" | "required" | "unsupported";
  runtime: RuntimeRequirement;
  documentation: string;
  examples?: NodeExample[];
  migrations?: NodeMigration[];
}
```

### 端口

```json
{
  "inputs": [{ "name": "in", "schema": { "type": "object" } }],
  "outputs": [
    { "name": "out", "schema": { "type": "object" } },
    { "name": "true", "schema": { "type": "object" } },
    { "name": "false", "schema": { "type": "object" } }
  ]
}
```

- 端口名称稳定；语义变更必须升 `typeVersion`。
- 动态端口（如 switch cases）必须由 config 生成并在编译期校验。

## 2. 核心节点

| 类型 | 类别 | 输入 | 输出端口 | 权限/副作用 | 说明 |
| --- | --- | --- | --- | --- | --- |
| `start` | system | 运行输入 | `out` | 无 | 入口；定义输入契约 |
| `end` | system | 任意 | — | 无 | 结束；定义输出 |
| `condition` | control | 任意 | `true`/`false` | 无 | jq 布尔表达式 |
| `switch` | control | 任意 | `case_*`/`default` | 无 | 多路分支；必须覆盖 default（可配置） |
| `loop` | control | items/while | `out`/`empty`/`error` | 无 | 容器；maxIterations 必填 |
| `parallel` | control | 任意 | `out`/`partial`/`error` | 无 | 容器；join all/any/n_of |
| `join` | control | 任意 | `out` | 无 | 显式汇合（可选） |
| `http.request` | integration | 请求 | `out`/`error` | 网络读/写 | 方法/URL/头/体/凭证；SSRF/超时/重试 |
| `script` | data | 任意 | `out`/`error` | 文件/命令 | 受限脚本；语言可扩展；沙箱 |
| `database.query` | integration | SQL/参数 | `out`/`error` | DB 读/写 | 连接引用；只读/写策略 |
| `file.read`/`file.write` | data | 路径 | `out`/`error` | 文件 | 工作区受限；路径校验 |
| `transform` | data | 任意 | `out` | 无 | jq 映射/过滤/聚合 |
| `setVariable` | data | 任意 | `out` | 无 | 显式变量写入；受作用域限制 |
| `delay` | control | 任意 | `out` | 无 | 最长/恢复策略；durable 需适配器支持 |
| `agent.run` | ai | 任务 | `out`/`error` | 模型/工具 | 调用 AgentVersion；预算/审批 |
| `skill.run` | ai | 输入 | `out`/`error` | 模型/脚本 | 加载 SkillVersion；本地/云 |
| `mcp.callTool` | ai | 参数 | `out`/`error` | MCP 网络 | 工具引用；schema/审批 |
| `human.approval` | human | 表单 | `approved`/`rejected`/`timeout` | 无 | 审批；SLA/升级 |
| `subflow` | system | 输入 | `out`/`error` | 继承 | 子流程引用/内联 |
| `compensate` | system | 任意 | `out` | 继承 | 补偿动作 |
| `notification` | integration | 消息 | `out`/`error` | 网络 | Email/Webhook/In-app |
| `mcp.trigger` | trigger | — | `out` | MCP | 由 MCP Gateway 触发的入口 |

> 触发器（schedule/webhook/event/mcp）是 Workflow 级配置，不作为普通首节点；UI 可呈现为入口卡片。

## 3. 节点执行生命周期

1. 解析 `type` + `typeVersion`（发布时已冻结）。
2. 校验 `config`（节点 schema）；未知字段按策略拒绝/警告。
3. 计算输入：`inputMapping` + 上游输出；jq 求值；类型/必填校验。
4. Policy：权限、审批、配额、风险。
5. 执行：节点 handler；外部调用带幂等键/超时/重试。
6. 输出：schema 校验；写 `NodeExecution`/事件。
7. 错误：分类 → 重试/错误策略/补偿；写事件。
8. 清理：临时文件/进程/凭证。

## 4. 版本与迁移

- 版本语义：patch（bug/文档）、minor（向后兼容新增配置）、major（语义/端口变更）。
- 注册表保留所有版本；UI 默认最新；发布冻结。
- 弃用：标记 `deprecated` + 迁移指引；至少保留两个 minor 周期。
- 迁移：`migrations: [{ from, to, migrate(config) }]`；发布时提示；不可自动迁移需人工。
- 运行中 Execution 继续用旧版本；不因新版本发布而改变。

## 5. 权限

节点声明所需能力：

```json
{
  "permissions": {
    "network": ["api.example.com:443"],
    "filesystem": { "read": ["./inputs"], "write": ["./artifacts"] },
    "commands": ["python3"],
    "connectors": ["conn_orders"],
    "models": ["model_fast"],
    "mcpServers": ["mcp_orders"]
  }
}
```

- 发布时与租户/工作区授权求交；缺权限拒绝。
- 运行时再次检查（防止策略变化/越权）。
- Agent Tool/MCP Tool 入口独立策略；不因 Node 授权而自动获得 Tool 授权。

## 6. 节点示例

### HTTP

```json
{
  "type": "http.request",
  "typeVersion": "1.0.0",
  "config": {
    "method": "POST",
    "url": { "$template": "https://api.example.com/orders" },
    "headers": { "Content-Type": "application/json" },
    "body": { "$expr": ".input.order" },
    "credentialRef": "cred_orders_api",
    "expectedStatus": [200, 201],
    "maxResponseBytes": 1048576
  },
  "retry": { "maxAttempts": 3, "retryOn": ["HTTP_5XX", "TIMEOUT"] },
  "timeoutMs": 30000
}
```

### Condition

```json
{
  "type": "condition",
  "typeVersion": "1.0.0",
  "config": { "condition": { "$expr": ".nodes.fetch_orders.output.body.total > 1000" } }
}
```

### Loop

```json
{
  "type": "loop",
  "typeVersion": "1.0.0",
  "config": {
    "mode": "forEach",
    "items": { "$expr": ".nodes.fetch_orders.output.body.orders" },
    "itemVariable": "order",
    "maxIterations": 500,
    "concurrency": 1
  },
  "body": { "nodes": [], "edges": [] }
}
```

### Parallel

```json
{
  "type": "parallel",
  "typeVersion": "1.0.0",
  "config": { "join": "all", "failFast": false, "maxConcurrency": 4 },
  "body": { "nodes": [], "edges": [] }
}
```

### Agent

```json
{
  "type": "agent.run",
  "typeVersion": "1.0.0",
  "config": {
    "agentRef": { "agentId": "agent_support", "version": "3.2.0" },
    "input": { "$expr": ".input.ticket" },
    "budget": { "maxSteps": 12, "maxToolCalls": 20, "maxCostUsd": 0.5 },
    "approvalPolicy": "inherit"
  }
}
```

### MCP Tool

```json
{
  "type": "mcp.callTool",
  "typeVersion": "1.0.0",
  "config": {
    "serverRef": "mcp_orders",
    "tool": "orders.lookup",
    "arguments": { "orderId": { "$expr": ".loop.item.id" } },
    "approval": { "requireFor": ["write"] }
  }
}
```

### Human Approval

见 [IR §6](workflow-ir.md#6-人工审批)。

## 7. 文档与 UX

- 节点定义必须提供：说明、输入/输出示例、权限说明、错误码、版本变更。
- UI 由 `configSchema` 生成表单；自定义 UI 可提供，但必须回落到 schema 表单。
- 节点搜索关键词/分类/图标；收藏/最近使用。
- 错误信息包含节点 id、字段路径、修复建议。

## 8. 测试

每个节点类型必须：

- Schema 正/负用例；
- 输入映射/输出校验；
- 权限拒绝；
- 超时/重试/错误策略；
- 幂等（如支持）；
- 沙箱/SSRF（如适用）；
- 版本迁移；
- 文档示例可执行。

## 9. 相关文档

- [Workflow IR](workflow-ir.md)
- [执行引擎](execution-engine.md)
- [表达式与数据映射](expression-and-mapping.md)
- [统一能力注册](../08-platform/capability-registry.md)
- [权限模型](../08-platform/permission-model.md)
