# 表达式与数据映射

> Hisha Expression Profile 使用 **jq 子集**（Rust `jaq` 3.1.2）作为映射/条件语言；任意代码只允许在受限 `script` 节点中执行。
> 不使用 JSONata：Rust JSONata 实现不成熟；JS JSONata 无法在 Rust Core 内安全/一致执行。

## 1. 选型

| 候选 | 优点 | 问题 | 结论 |
| --- | --- | --- | --- |
| jq（jaq 3.1.2，MIT） | 纯 Rust、Active、表达力强、JSON 原生 | 需禁用 I/O/环境/时间等内建；语法对非开发者有学习成本 | **采用** |
| JSONata（JSONata 2.2.2 JS） | 声明式、易读 | Rust 实现不成熟（`jsonata-rs` 0.3.4 停更）；JS 运行时不能进 Rust Core | 不采用 |
| JMESPath 0.5（Rust，MIT） | 简单查询、成熟 | 变换/聚合能力弱 | 备选（只读查询） |
| CEL | 强类型/安全 | Rust/JS 生态/工具链较弱 | 观察 |
| JavaScript 表达式 | 灵活 | 任意代码/沙箱复杂/跨语言不一致 | 只用于 `script` |

## 2. 语法

### 非字符串动态值

```json
{ "$expr": ".nodes.fetch_orders.output.body.orders[] | select(.amount > 1000)" }
```

### 字符串插值

```json
{ "$template": "订单 ${.input.orderId} 金额 ${.nodes.fetch.output.total}" }
```

规则：

- `$expr` 的求值根是 Context 对象（见 §3）；表达式是 jq filter。
- `$template` 的 `${...}` 内是 jq filter；结果转字符串；`\${` 转义。
- 字面量不加包装；对象/数组可递归包含 `$expr`/`$template`。
- 禁止同时出现 `$expr` 与 `$template`。
- `$expr` 必须产出**单个 JSON 值**；多输出需用 `[...]` 收集，否则运行时错误。
- 表达式中不能访问 `secrets.*`；secret 只能通过 `credentialRef` 注入连接器。

## 3. Context

```json
{
  "input": { "date": "2026-11-01", "orderId": "..." },
  "vars": {},
  "nodes": { "fetch_orders": { "output": { "body": {} } } },
  "env": { "LANG": "ja_JP.UTF-8" },
  "execution": { "id": "exec_...", "now": "2026-11-01T00:00:00Z", "tenantId": "ten_..." },
  "loop": { "item": {}, "index": 0 },
  "parallel": { "branch": "b1" }
}
```

| 名称 | 说明 |
| --- | --- |
| `.input` | 运行输入（不可变） |
| `.vars` | 工作流变量（仅 setVariable 写） |
| `.nodes` | 节点输出：`.nodes.<id>.output.<field>` |
| `.env` | 环境变量白名单（只读，非 secret） |
| `.execution` | 执行 ID/固定时间/租户 |
| `.loop` | 容器内 item/index |
| `.parallel` | 并行分支标识 |

未声明/未绑定的路径在编译期报错；运行时缺失字段为 `null`（严格模式可报错）。

## 4. 条件

```json
{ "$expr": "(.nodes.check.output.ok == true) and (.input.orderId != null)" }
```

- 条件必须返回布尔；`null`/false 为假，其他值在严格模式下报错（不隐式 truthy）。
- 复杂条件建议先 `transform` 为布尔字段，便于调试。

## 5. 常用映射

### 字段映射

```json
{
  "orderId": { "$expr": ".input.order.id" },
  "amount": { "$expr": ".input.order.total * 100" },
  "currency": { "$expr": ".input.order.currency // \"JPY\"" },
  "items": { "$expr": "[.input.order.items[] | {sku: .sku, qty: .qty}]" }
}
```

### 过滤/排序/聚合

```json
{
  "highValue": { "$expr": "[.nodes.fetch.output.orders[] | select(.amount > 1000)] | sort_by(.amount) | reverse" },
  "count": { "$expr": ".nodes.fetch.output.orders | length" },
  "total": { "$expr": "[.nodes.fetch.output.orders[].amount] | add // 0" },
  "byStatus": { "$expr": ".nodes.fetch.output.orders | group_by(.status) | map({status: .[0].status, count: length})" }
}
```

### 模板

```json
{
  "subject": { "$template": "[${.execution.tenantId}] 订单 ${.input.orderId} 审批" },
  "body": { "$template": "金额：${.nodes.fetch.output.total} ${.input.currency}" }
}
```

## 6. 受限内建

允许：jq 标准纯函数（字符串/数字/数组/对象/排序/分组/正则，受限制）。

禁止/替换：

- `input`、`inputs`、`$ENV`、`env`、`now`、`debug`、`stderr`、`halt`、`halt_error`、`input_filename`、模块加载/`include`/`import`。
- 文件/网络/进程访问：jq 本身无此能力；禁止通过自定义函数注入。
- 高开销正则/递归：限制输入大小、递归深度、执行时间、输出大小。
- 非确定性：使用 `.execution.now`（执行开始时间）；禁止 `now`；随机数不提供。

可选 Hisha 函数（纯函数，版本化）：`hisha_uuidv7`、`hisha_hash`、`hisha_base64`、`hisha_date_add`。实现于 Rust 沙箱，登记到表达式函数注册表。

## 7. 类型与错误

- 数字：JSON number；精度敏感用字符串 + 显式转换。
- 日期：ISO 8601 字符串；`hisha::date_add` 等纯函数；时区显式。
- 二进制：base64 字符串或 Artifact 引用；表达式不自动加载大文件。
- 错误码：`EXPR_PARSE`、`EXPR_TYPE`、`EXPR_MISSING`、`EXPR_LIMIT`、`EXPR_FORBIDDEN`。
- 模板缺失值：默认空字符串 + warning；严格模式报错（按节点配置）。

## 8. 安全

- 表达式编译期解析/校验；运行时在 jaq 解释器执行，无宿主对象/文件/网络。
- 禁用 I/O/环境/时间/调试/模块内建；限制表达式长度、AST 深度、正则长度、输入/输出大小、执行时间。
- 不把 secret 放入 Context；连接器凭证运行时注入。
- 日志不记录完整输入/输出（脱敏/摘要）。
- TS 前端不内嵌 jq 执行器；预览/校验调用 Rust API，避免两套语义。

## 9. 与脚本节点的边界

| 需求 | 用表达式 | 用 script |
| --- | --- | --- |
| 字段映射/过滤/聚合 | 是 | 否 |
| 条件/分支 | 是 | 否 |
| 复杂算法/库依赖 | 否 | 是（沙箱） |
| 文件/网络 | 否 | 是（受权限） |
| 需要任意控制流 | 否 | 是（受限制） |

`script` 节点的输入/输出仍通过 schema/映射，不直接访问工作流内部状态。

## 10. 测试

- 解析/类型/缺失/边界（null/空数组/Unicode/大数）。
- 确定性：同一输入/`.execution.now` 结果一致；禁止 `now`。
- 性能：10k 项过滤/聚合 < 100ms（目标）。
- 安全：I/O/环境/模块/正则 DoS/超长表达式被拒绝。
- 跨语言：Rust jaq 与 TS 端仅通过 Schema/预览 API 一致，不双实现。

## 11. 相关文档

- [Workflow IR](workflow-ir.md)
- [节点规范](node-specification.md)
- [执行引擎](execution-engine.md)
- [Rust 技术栈](../13-tech-stack/rust-stack.md)
- [安全架构](../02-architecture/security-architecture.md)
