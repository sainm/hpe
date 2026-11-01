# ADR-0014：Rust 作为 Core Runtime

- 状态：已接受（替代 ADR-0002/0004/0007 中的语言部分）
- 日期：2026-11-01
- 决策者：架构负责人（Phase 0 技术栈）

## 背景

上一轮 Phase 0 采用 TypeScript 轻量运行时。新的技术栈约束要求 Rust 作为核心执行基础设施语言：Workflow Runtime、IR/Compiler/Validator、Local Runtime、Skill Runtime、MCP Runtime、Worker、Plugin、Script/Process/Resource/Security。需要在不重复实现 Temporal/LangGraph/LiteLLM 的前提下建立清晰边界。

## 决策

- **Rust** 实现 Core Runtime/Execution Plane：`crates/hpa-*`；Local Runtime 为 Rust 单二进制（嵌入式 UI/SQLite）；Worker 为 Rust。
- **TypeScript** 保留 Control Plane（NestJS）与 Web（React）；通过 gRPC 调 Rust，不执行 Workflow/脚本。
- **Python** 仅作为可选 AI Worker（LangGraph），通过 gRPC 与 Rust 集成；无 Python 时确定性功能可用。
- MCP 由 Rust RMCP 3.8.0 实现（2026-07-28 + 2025-11-25）；TS 只做注册/管理/UI。
- Local Runtime 实现本地控制面子集（单用户/SQLite）；Cloud Control Plane 是 NestJS；共享 OpenAPI/IR 契约。
- MVP 只做 Rust Local + React Workflow；NestJS/Python/MCP/WASM/Temporal 后续增量。

## 备选

1. TypeScript 轻量运行时（旧方案）：开发快，但执行/沙箱/资源/单二进制/跨平台不满足新约束。
2. Go 运行时：单二进制/并发好，但 MCP/WASM 生态与类型/团队契合度不如 Rust。
3. Rust 全栈（含 Control Plane）：复杂度/开发速度/生态不适合。
4. Python 核心：GIL/部署/性能/可选性不符合。

## 后果

- Rust 工具链 1.99.x/MSRV 1.97；本机未安装，需用户安装。
- 需要 gRPC/Proto/JSON Schema 契约；TypeScript/Python 生成代码。
- 本地/云端双控制面（Rust 子集/NestJS）需要契约测试与功能矩阵。
- 编译时间/人才/维护成本上升；通过 MVP 切片、sccache、模块化缓解。
- 必须修订 ADR-0002/0004/0007 与架构/路线图文档。

## 证据

[Rust 技术栈](../13-tech-stack/rust-stack.md)、[语言策略](../13-tech-stack/language-strategy.md)、[证据](../13-tech-stack/evidence.md)、[Local Runtime](../13-tech-stack/local-runtime-rust.md)。
