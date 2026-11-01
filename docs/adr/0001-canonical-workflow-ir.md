# ADR-0001：采用统一 Canonical Workflow IR

- 状态：已接受
- 日期：2026-11-01
- 决策者：架构负责人（Phase 0）

## 背景

Canvas、YAML、JSON 三种视图如果各自维护模型，会产生语义漂移、版本/权限/执行不一致；开源项目的流程模型通常与自身引擎/UI 绑定，不能直接作为 Hisha 的模型。产品要求三视图可互相转换且语义一致。

## 决策

采用一个 Canonical IR（`hpa.dev/v1`）：

- Canvas/YAML/JSON 都是 IR 的视图/序列化。
- IR 由 JSON Schema 定义；YAML 使用 JSON 兼容子集。
- 节点类型/版本、端口、容器、变量作用域、表达式、错误/重试/超时、审批、子流程、布局、策略显式建模。
- 发布生成不可变 WorkflowVersion；执行绑定版本。
- 语义往返无损；文本往返只保证规范文本，注释/格式通过 CST/SourceMap 尽力保留。

## 备选

1. Canvas 模型 + YAML/JSON 转换器：短期快，长期双模型/漂移。
2. 采用 Kestra Flow/BPMN/n8n JSON 作为 IR：受其语义/生态/许可约束，无法覆盖 Hisha 的 Agent/Skill/MCP/审批。
3. 纯 YAML 作为唯一源：Canvas 布局/交互/版本困难。

## 后果

- 需要编译器/校验器/静态分析/迁移器（核心自研）。
- 节点专属字段必须通过 `config`/扩展字段隔离，避免污染 IR。
- PoC A 必须先验证往返/执行；未通过不进入 Phase 1。
- YAML/JSON 文本保真需明确分级（L0/L1/L2），不做虚假承诺。

## 证据

[Workflow IR](../03-workflow/workflow-ir.md)、[YAML/JSON](../03-workflow/yaml-json-schema.md)、[PoC A](../11-roadmap/poc-plan.md)。
