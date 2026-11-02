# YAML / JSON Schema 与保真

> Canonical IR 以 JSON 为语义载体；YAML 是等价序列化。**语义往返必须无损；文本往返不承诺无损**——这是设计约束，不是缺陷。

## 1. 三种视图

| 视图 | 用途 | 语义来源 | 文本保真 |
| --- | --- | --- | --- |
| Canvas | 可视化编辑/布局 | IR | 不适用 |
| YAML | 人手编辑/评审/版本控制 | IR | 原始文本哈希 + SourceMap；当前不做注释保留式发射 |
| JSON | API/存储/机器交换 | IR | 仅规范文本 |

## 2. Schema

- JSON Schema 2020-12；Workflow `$id: https://hpa.dev/schema/ir/v0.1/workflow.schema.json`。
- IR 自身、节点 config、输入/输出、事件、能力描述分别有 schema。
- YAML 使用同一 schema 的“JSON 兼容子集”，不定义第二套模型。
- 发布时生成 `workflow.schema.json` 与 `node-registry.schema.json`；SDK 从 schema 生成类型。

### YAML 允许子集

实现：Rust 使用 Saphyr 0.3.3（YAML 1.2 + span），TS 使用 `yaml` 2.9.1；两者以同一 fixture 做语义 parity，IR 仍是唯一语义源。Saphyr 替代已 deprecated 的 `serde_yaml`。

允许：map/seq/scalar（string/number/bool/null）、UTF-8、注释、文档分隔符（单文档）。

禁止/规范化：

- 自定义标签（`!!python/...`、`!Ref` 等）→ 拒绝。
- 重复键 → 拒绝。
- 非字符串 map 键 → 拒绝。
- 时间戳/日期 → 必须加引号，按字符串处理。
- 二进制 → base64 字符串。
- 锚点/别名 → 解析后展开；规范化输出不使用锚点（源文本保留在 CST）。
- 多文档 → 拒绝（一个文件一个 Workflow）。

## 3. 保真等级

### L0 语义往返（强制）

```
IR → YAML → IR'
IR → JSON → IR'
assert semanticEqual(IR, IR') == true
```

`semanticEqual` 忽略 `layout`/`ui` 与对象键顺序；保留所有语义字段、未知扩展字段、节点版本、权限、策略。

### L1 规范文本往返（强制）

对由 Hisha 生成的规范 YAML/JSON：

```
text → parse → emitCanonical → text'
assert text === text'
```

规范规则：

- UTF-8、LF、末尾换行；
- 2 空格缩进；序列缩进一致；
- 键顺序：`schemaVersion` 固定在前，其余按 schema 定义顺序，未知键按字典序；
- 字符串按需加引号（避免歧义：`yes/no/on/off/null/数字/时间/空串/特殊字符`）；
- 不使用锚点/别名/自定义标签；
- JSON：2 空格缩进、稳定键序、无注释、`\n` 结尾。

### L2 编辑保真（尽力）

- YAML 编辑使用 `yaml` 包的 Document/CST API，仅替换被修改的标量/节点，保留其他注释/空白/引号风格。
- 每次解析返回 `SourceMap`：IR 路径 ↔ 文本区间（UTF-8 byte offset/line/column），见 `hpa_core::yaml::yaml_to_json_with_source_map`。
- Canvas 编辑重新生成规范文本；对可锚定的注释（绑定到节点 id/字段路径）尽量保留；无法锚定的注释移入 sidecar `annotations`，UI 提示。
- 不承诺：YAML 锚点/别名/复杂多行折叠的原始风格；Canvas 编辑后的空白/排序。

### 不可保真项

- Canvas 布局/视口：写入 `layout`，语义比较忽略。
- 注释：文本资产，不属于 IR 语义；通过 SourceMap/CST/annotations 管理。
- 格式/空白/键顺序：非语义。

## 4. Source Map

```json
{
  "schemaVersion": "hpa.sourcemap/v1",
  "textHash": "sha256:...",
  "language": "yaml",
  "entries": [
    {
      "irPath": "/nodes/0/config/url",
      "range": { "start": 420, "end": 468, "line": 18, "column": 12 },
      "kind": "scalar"
    }
  ],
  "comments": [
    { "text": "订单 API 只读", "range": { "start": 380, "end": 410 }, "anchor": "/nodes/0" }
  ]
}
```

- SourceMap 是 sidecar，不写入 IR；导出时可选。
- 文本修改后 hash 不匹配 → SourceMap 失效，重新解析。
- 合并冲突：按 IR 路径/节点 ID + 文本区间双键；无法合并则要求人工。

## 5. 规范化与比较

- `canonicalize(ir)`：移除 `ui`/`layout` 的坐标（保留语义布局？布局无语义），稳定排序，规范化数字/字符串。
- `semanticHash(ir)`：SHA-256 of canonical JSON；用于发布/去重/审计。
- `textualHash(source)`：SHA-256 of raw text；用于 SourceMap/审计。
- Diff：
  - 语义 diff：节点/边/字段增删改；忽略布局。
  - 文本 diff：行级；用于评审。
  - UI 同时展示：节点高亮 + YAML 行。

## 6. YAML 示例

```yaml
schemaVersion: hpa.dev/v1
id: wf_order_sync
name: 订单同步与审批
description: 每日同步订单，金额超阈值转人工审批
workflowVersion: 1.0.0
inputs:
  - name: date
    type: string
    required: true
variables: []
nodes:
  - id: start
    type: start
    typeVersion: 1.0.0
    config: {}
  - id: fetch_orders
    type: http.request
    typeVersion: 1.0.0
    config:
      method: GET
      url:
        $template: "https://api.example.com/orders?date=${.input.date}"
      credentialRef: cred_orders_api
    retry:
      maxAttempts: 3
  - id: check_amount
    type: condition
    typeVersion: 1.0.0
    config:
      condition:
        $expr: "[.nodes.fetch_orders.output.body.orders[] | select(.amount > 1000)] | length > 0"
  - id: approve_refund
    type: human.approval
    typeVersion: 1.0.0
    config:
      title: 高额订单审批
      assignees:
        - kind: role
          value: finance_approver
edges:
  - { id: e1, source: start, sourcePort: out, target: fetch_orders, targetPort: in }
  - { id: e2, source: fetch_orders, sourcePort: out, target: check_amount, targetPort: in }
  - { id: e3, source: check_amount, sourcePort: "true", target: approve_refund, targetPort: in }
triggers: []
policies:
  durability: standard
layout:
  viewport: { x: 0, y: 0, zoom: 1 }
```

对应 JSON 只需把表达式对象保留，键顺序按规范。

## 7. 导入外部格式

| 来源 | 方式 | 保真/风险 |
| --- | --- | --- |
| Kestra YAML | 只读导入器 | 语义子集；Pebble 表达式需转换/人工修复；布局丢失 |
| n8n JSON | 只读导入器（如做） | 节点映射有限；表达式不兼容；许可只读 |
| BPMN | 不导入 | 仅人工建模；避免 BPMN/IR 双模型 |
| 其他 Hisha JSON | 直接读 | 版本迁移 |

导入结果：

- 标记 `metadata.source = { kind, version, uri, importedAt }`。
- 生成“丢失/需人工”报告；不静默丢弃。
- 导入不等于发布；必须通过五类校验。

## 8. Schema 发布

- 每个 `schemaVersion` 发布 JSON Schema + 文档 + 迁移器。
- Schema 仓库/包：`@hpa/schema`。
- 破坏性变更：新 `schemaVersion`，旧版本至少支持 2 个大版本；运行中的 Execution 使用旧 schema。
- 节点配置 schema 独立版本；Node Type 版本引用 schema 版本。

## 9. 测试

- 固定 fixture：分支/循环/并行/审批/子流程/错误/重试/大对象/Unicode/CRLF/注释/锚点/空节点。
- 属性测试：随机生成合法 IR → YAML/JSON → IR 语义相等。
- 文本规范测试：canonical emit → parse → emit 稳定。
- SourceMap：编辑单字段后其他注释/格式不变。
- 负面：重复键/自定义 tag/非字符串键/未绑定变量/悬空边 → 明确错误。
- 跨平台：Windows CRLF、macOS/Linux LF、UTF-8 非 BMP 字符。

## 10. 相关文档

- [Workflow IR](workflow-ir.md)
- [节点规范](node-specification.md)
- [执行引擎](execution-engine.md)
- [PoC A](../11-roadmap/poc-plan.md)
