# ADR-0004：Agent Runtime 以 LangGraph Python 为可选内核

- 状态：已接受（Phase 4；Python 可选）
- 日期：2026-11-01（技术栈更新）
- 决策者：架构负责人

## 背景

Agent 需要状态图、checkpoint、interrupt/resume、工具调用、多 Agent；LangGraph Python 是 MIT、成熟。技术栈约束指定 Python 为可选 AI 集成语言，Rust 为核心执行；不允许 AI Framework 成为平台唯一执行引擎。

## 决策

- 使用 **Python 3.13 + LangGraph 1.2.19** 作为可选 AI Worker 的状态图内核；Postgres/SQLite checkpointer。
- Hisha 负责：AgentVersion/配置、多租户、工具/权限、预算、审批、模型网关、事件、UI。
- Rust Core 通过 gRPC `AgentWorker` 调用 Python；Python 通过 `ToolService` gRPC 回调 Rust；不直接访问 Control Plane DB/租户。
- LangGraph/Pydantic 类型不跨边界；事件/请求/结果用 Proto/JSON Schema。
- 没有 Python AI Worker 时 Agent 功能禁用；确定性 Workflow/Skill 不受影响。
- 不再采用 LangGraph JS 作为内核（旧方案）；TS 只做 Agent Designer/管理 API。

## 备选

- LangGraph JS：与 TS 同栈，但技术栈约束指定 Python AI；且 Python 生态更成熟。
- 自研 Agent 状态机：可控但重复 checkpoint/interrupt/streaming。
- Dify Agent/n8n Agents：许可/栈/耦合不匹配。
- 纯 Prompt loop：无状态/恢复/多 Agent 能力不足。

## 后果

- 额外 Python 进程/依赖/ABI；通过容器/uv lock/可选部署缓解。
- LangGraph 升级/checkpoint 迁移需契约/集成测试。
- 多租户/预算/审批仍需 Rust/Control Plane；不是“引入即完成”。
- AI Worker 崩溃/重启需恢复；checkpoint schema 独立。

## 证据

[LangGraph 报告](../01-research/projects/langgraph.md)、[Agent Runtime](../04-agent/agent-runtime.md)、[Python 技术栈](../13-tech-stack/python-stack.md)、[ADR-0014](0014-rust-core-runtime.md)。
