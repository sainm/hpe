# ADR-0002：轻量 Rust 运行时 + Temporal 长期适配

- 状态：已接受（Temporal 待 PoC；语言部分由 ADR-0014 明确）
- 日期：2026-11-01（原 Phase 0；技术栈更新）
- 决策者：架构负责人

## 背景

Hisha 需要本地优先、IR 语义可控的执行；同时需要长期 durable execution（长任务/审批/崩溃恢复）。技术栈约束指定 Rust 为核心执行语言。

## 决策

- **v1**：自研 **Rust** 轻量运行时（`hpa-runtime` + `hpa-local` + `hpa-worker`），负责 IR 解释、分支/循环/并行/重试/超时/审批、检查点、事件、恢复。承诺“检查点 + 至少一次 + 可恢复”，不承诺确定性重放。
- **Phase 3+**：Temporal 作为 `durable` 适配器（通用 IR 解释器 + 节点 Activity + Signal/Update）；**Rust 核心不直接实现 Temporal Worker**；通过薄适配 Worker（TS/Go/Python SDK）或 Temporal gRPC PoC 决定。先 PoC，后决定是否默认/可选。
- **Kestra**：不作为核心引擎；只做设计参考/可选 YAML 导入源。
- **LangGraph**：只做可选 Python Agent Worker（ADR-0004），不进入 Workflow 引擎。

## 备选

- Kestra 内核：JVM/插件 ABI/Flow 模型/本地部署与 Hisha 不匹配。
- Temporal-only：本地打包 server/DB；IR 解释器仍需自研；对短流程过重。
- 纯自研 durable：重造事件溯源/长任务。
- TypeScript 轻量运行时：旧方案，开发快但执行/沙箱/单二进制不满足新约束（由 ADR-0014 替代）。

## 后果

- `RuntimeAdapter` 契约 + Rust/Python/TS 契约测试；Temporal 适配器独立。
- 轻量运行时恢复边界必须在 UI/文档中明确，避免虚假 durable 承诺。
- Temporal 增加运维/版本/成本；只在 PoC 通过且客户需求明确时启用。
- 不引入 Kestra/Temporal 同时作为核心。

## 证据

[架构对比](../01-research/architecture-comparison.md)、[执行引擎](../03-workflow/execution-engine.md)、[Rust 技术栈](../13-tech-stack/rust-stack.md)、[ADR-0014](0014-rust-core-runtime.md)。
