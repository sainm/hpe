# Canonical Workflow IR

> IR 是 Hisha 的唯一流程真相。Canvas、YAML、JSON 都是它的视图/序列化；执行、校验、版本、权限都以 IR 为准。
> Schema 版本：`hpa.dev/v1`（Phase 0 草案，PoC A 冻结）。

## 1. 顶层结构

```json
{
  "schemaVersion": "hpa.dev/v1",
  "id": "wf_order_sync",
  "name": "订单同步与审批",
  "description": "每日同步订单，金额超阈值转人工审批",
  "workflowVersion": "1.0.0",
  "inputs": [],
  "outputs": [],
  "variables": [],
  "nodes": [],
  "edges": [],
  "triggers": [],
  "policies": {},
  "errorHandling": {},
  "retry": {},
  "timeout": {},
  "subflows": [],
  "layout": {},
  "metadata": {}
}
```

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| `schemaVersion` | 是 | IR 结构版本；与 Workflow 业务版本无关 |
| `id` | 是 | 稳定标识；导入时保留 |
| `name` / `description` | 是 | 展示与搜索 |
| `workflowVersion` | 是 | 业务版本（semver 或单调编号）；发布时冻结 |
| `inputs` | 否 | 运行输入契约（JSON Schema + UI 提示） |
| `outputs` | 否 | 输出契约 |
| `variables` | 否 | 显式变量声明（作用域/类型/默认值） |
| `nodes` / `edges` | 是 | 根图 |
| `triggers` | 否 | 定时/Webhook/事件/MCP |
| `policies` | 否 | 并发/预算/审批/持久性/风险/保留 |
| `errorHandling` | 否 | 工作流级错误处理图/策略 |
| `retry` / `timeout` | 否 | 默认策略，可被节点覆盖 |
| `subflows` | 否 | 内联可复用子图 |
| `layout` | 否 | 画布元数据（不参与执行） |
| `metadata` | 否 | 标签/所有者/来源/生成信息 |

## 2. Graph / Node / Edge

### Graph

```json
{
  "nodes": [],
  "edges": []
}
```

根图与容器子图结构相同。子图必须是 DAG；循环由 `loop` 容器语义表达，不靠回边。

### Node

```json
{
  "id": "fetch_orders",
  "type": "http.request",
  "typeVersion": "1.0.0",
  "name": "拉取订单",
  "description": "",
  "config": {
    "method": "GET",
    "url": { "$template": "https://api.example.com/orders?date=${.input.date}" },
    "credentialRef": "cred_orders_api"
  },
  "inputMapping": {},
  "outputSchema": { "type": "object" },
  "retry": { "maxAttempts": 3, "backoff": { "kind": "exponential", "baseMs": 500, "maxMs": 10000 } },
  "timeoutMs": 30000,
  "errorPolicy": { "mode": "fail" },
  "permissions": { "network": ["api.example.com"] },
  "ui": { "x": 120, "y": 80, "width": 220, "height": 80 }
}
```

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| `id` | 是 | 图内唯一；稳定；不能以 `nodes.` 开头 |
| `type` | 是 | 节点类型（注册表） |
| `typeVersion` | 是 | 节点实现版本；发布冻结 |
| `config` | 是 | 类型专属配置（由节点 schema 校验） |
| `inputMapping` | 否 | 从作用域到节点输入的映射；缺省按同名 |
| `outputSchema` | 否 | 输出契约；运行时校验 |
| `retry` / `timeoutMs` / `errorPolicy` | 否 | 覆盖默认 |
| `permissions` | 否 | 节点所需能力（网络/文件/命令/连接器） |
| `ui` | 否 | 布局；不参与执行 |

### Edge

```json
{
  "id": "edge_1",
  "source": "fetch_orders",
  "sourcePort": "out",
  "target": "check_amount",
  "targetPort": "in"
}
```

- 条件/分支由源节点的端口决定（`true`/`false`/`case_x`/`default`），边不携带表达式。
- 端口必须在节点 schema 中声明；悬空边/未声明端口是编译错误。
- 并行/循环由容器节点表达，不靠多条出边隐式并行。

### Container Node

```json
{
  "id": "loop_orders",
  "type": "loop",
  "typeVersion": "1.0.0",
  "config": {
    "mode": "forEach",
    "items": { "$expr": ".nodes.fetch_orders.output.body" },
    "itemVariable": "order",
    "maxIterations": 500
  },
  "body": { "nodes": [], "edges": [] }
}
```

容器类型：`loop`、`parallel`、`branch`（可选）、`subflow`。容器拥有 `body` 子图；容器失败/重试/超时语义由容器节点定义。

## 3. 变量与作用域

| 命名空间 | 读 | 写 | 说明 |
| --- | --- | --- | --- |
| `input.*` | 是 | 否 | 运行输入，启动后不可变 |
| `vars.*` | 是 | 仅 `setVariable` 节点 | 图/迭代作用域；子图隔离 |
| `nodes.<id>.output.*` | 是 | 仅运行时 | 节点输出；只读 |
| `env.*` | 是 | 否 | 环境变量白名单；非 secret |
| `secrets.<name>` | 否 | 否 | 不允许表达式直接读取；只能通过 `credentialRef` 注入连接器 |
| `loop.item` / `loop.index` | 是 | 否 | 容器作用域 |
| `parallel.branch` | 是 | 否 | 并行分支标识 |

规则：

- 变量必须先声明（inputs/variables）或由上游节点输出产生；静态分析验证。
- 子图不能写父图变量；通过容器输出显式返回。
- 禁止隐式全局状态；`vars` 的写入必须可见（`setVariable` 节点）。
- Secret 只以引用出现；表达式求值结果不得包含 secret 值。

## 4. 表达式与映射

```json
{
  "$expr": ".nodes.fetch_orders.output.body.orders[] | select(.amount > 1000)"
}
```

- 非字符串动态值：`{ "$expr": "<jq filter>" }`。
- 字符串插值：`{ "$template": "Hello ${.input.name}" }`。
- 字面量：直接字符串/数字/布尔/对象/数组。
- 条件/分支：节点 config 中的 `condition` 必须是 `$expr` 布尔表达式。
- 禁止 `eval`/任意 JS；jq 子集无 I/O/环境/时间访问。

详见 [表达式与数据映射](expression-and-mapping.md)。

## 5. 错误处理、重试、超时、补偿

### 节点级

```json
{
  "retry": {
    "maxAttempts": 3,
    "backoff": { "kind": "exponential", "baseMs": 500, "maxMs": 10000, "jitter": true },
    "retryOn": ["HTTP_5XX", "TIMEOUT"],
    "nonRetryable": ["AUTH_FAILED", "VALIDATION"]
  },
  "timeoutMs": 30000,
  "errorPolicy": {
    "mode": "fail",
    "fallbackPort": null,
    "compensationNodeId": "refund_order"
  }
}
```

`errorPolicy.mode`：`fail` / `skip` / `continue` / `fallback` / `compensate`。`fallback` 需要 `fallbackPort`；`compensate` 需要补偿节点。

### 工作流级

```json
{
  "errorHandling": {
    "mode": "graph",
    "graph": { "nodes": [], "edges": [] },
    "defaultMode": "fail"
  },
  "retry": { "maxAttempts": 1 },
  "timeout": { "executionMs": 3600000 }
}
```

### 补偿

- 补偿动作在节点/工作流失败后按逆序执行；best effort。
- 补偿节点自身失败记录但不改变原始错误；审计/告警。
- 不承诺外部系统原子性；幂等键 + 对账。

## 6. 人工审批

```json
{
  "id": "approve_refund",
  "type": "human.approval",
  "typeVersion": "1.0.0",
  "config": {
    "title": "退款审批",
    "formSchema": {
      "type": "object",
      "properties": { "decision": { "type": "string", "enum": ["approve", "reject"] }, "comment": { "type": "string" } },
      "required": ["decision"]
    },
    "assignees": [{ "kind": "role", "value": "finance_approver" }],
    "slaMs": 86400000,
    "escalation": { "afterMs": 43200000, "assignees": [{ "kind": "role", "value": "finance_lead" }] },
    "onTimeout": "reject"
  }
}
```

输出端口：`approved`、`rejected`、`timeout`。审批记录独立于执行事件，但通过 executionId/nodeId 关联。

## 7. 触发器

```json
{
  "triggers": [
    { "id": "trg_schedule", "type": "schedule", "config": { "cron": "0 9 * * *", "timezone": "Asia/Tokyo", "catchUp": "skip" } },
    { "id": "trg_webhook", "type": "webhook", "config": { "path": "/orders/sync", "secretRef": "cred_webhook" } }
  ]
}
```

- 触发器配置只存引用，不存 secret。
- 定时器时区/DST 明确；补跑策略显式。
- Webhook 签名/重放保护由运行时实现。

## 8. 策略

```json
{
  "policies": {
    "durability": "standard",
    "concurrency": { "key": "workflow", "limit": 4 },
    "budget": { "maxCostUsd": 5, "maxTokens": 100000 },
    "riskLevel": "medium",
    "dataRetentionDays": 30,
    "approval": { "requireForRisk": ["high"] }
  }
}
```

- `durability`: `standard`（轻量） / `durable`（Temporal，Phase 3+）。
- `riskLevel`: low/medium/high/critical；影响审批/配额。
- 发布时冻结策略快照；执行不能降级策略。

## 9. 子流程

```json
{
  "subflows": [
    {
      "id": "sub_validate",
      "name": "校验订单",
      "inputs": [],
      "outputs": [],
      "graph": { "nodes": [], "edges": [] }
    }
  ],
  "nodes": [
    { "id": "call_validate", "type": "subflow", "typeVersion": "1.0.0", "config": { "subflowId": "sub_validate" } }
  ]
}
```

- 内联子流程随 Workflow 版本冻结。
- 外部子流程用 `workflowRef: { workflowId, version }`；发布时解析并记录具体版本。
- 递归深度/循环引用编译期拒绝。

## 10. 布局

```json
{
  "layout": {
    "viewport": { "x": 0, "y": 0, "zoom": 1 },
    "groups": [{ "id": "g1", "label": "订单处理", "color": "blue", "nodeIds": ["fetch_orders"] }]
  },
  "nodes": [{ "id": "fetch_orders", "ui": { "x": 120, "y": 80 } }]
}
```

布局只影响 UI；导出到 YAML/JSON 时保留；不影响执行/比较（语义比较忽略 `ui`/`layout`）。

## 11. 版本与兼容

- `schemaVersion` 变更：提供迁移器；旧版本可读。
- `typeVersion` 变更：节点注册表保留旧实现；发布冻结。
- Workflow 发布：不可变快照；Execution 绑定快照。
- 导入外部 IR/DSL：转换为 Hisha IR 时记录 `metadata.source` 与丢失项；不假装无损。

## 12. 校验不变量

1. ID 唯一；边引用存在。
2. 根图/子图无环（循环只通过容器）。
3. 所有必需输入/变量已绑定；类型兼容。
4. 端口存在；分支覆盖 default（如要求）。
5. 容器有合法 body/join/limit。
6. Secret 只以引用出现。
7. 权限/依赖/策略可满足。
8. 子流程/节点版本存在。
9. 表达式可解析/类型可推断。
10. 布局字段不影响语义校验。

## 13. 工具链契约

| 组件 | 输入 | 输出 | 失败码 |
| --- | --- | --- | --- |
| JSON/YAML Schema | IR/节点 config | 结构错误列表 | `IR_SCHEMA_*` |
| IR Validator | IR + Node Registry + Policy | 校验报告（error/warning） | `IR_VALIDATION_*` |
| Compiler | 已校验 IR | 执行计划（节点/边/作用域/表达式 AST） | `IR_COMPILE_*` |
| Static Analyzer | IR + 变量/端口/子流程 | 不可达/未绑定/类型/循环/权限依赖 | `IR_STATIC_*` |
| Dependency Resolver | 能力/节点/连接器/插件/模型/MCP 引用 | 锁定版本/冲突/缺失 | `DEP_*` |
| Expression Engine（jaq） | `$expr`/`$template` | AST/求值结果 | `EXPR_*` |

- Validator 与 Static Analyzer 分离：前者结构/语义规则，后者跨节点/依赖分析。
- Dependency Resolver 在发布时锁定版本；运行时不再解析范围。
- Compiler 不执行用户代码；只生成计划。
- 所有组件版本化；与 `schemaVersion` 绑定。

## 14. 相关文档

- [YAML/JSON Schema 与保真](yaml-json-schema.md)
- [节点规范](node-specification.md)
- [执行引擎](execution-engine.md)
- [表达式与数据映射](expression-and-mapping.md)
- [AI Workflow 生成](ai-workflow-generation.md)
