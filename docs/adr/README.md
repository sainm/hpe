# ADR 索引

| ADR | 标题 | 状态 |
| --- | --- | --- |
| [0001](0001-canonical-workflow-ir.md) | 采用统一 Canonical Workflow IR | 已接受 |
| [0002](0002-execution-engine-strategy.md) | 轻量 Rust 运行时 + Temporal 长期适配 | 已接受（Temporal 待 PoC） |
| [0003](0003-modular-monolith.md) | Control Plane 采用 Modular Monolith | 已接受 |
| [0004](0004-agent-runtime-langgraph.md) | Agent Runtime 以 LangGraph Python 为可选内核 | 已接受（Phase 4） |
| [0005](0005-skill-standard.md) | Skill 采用 Agent Skills 标准 | 已接受 |
| [0006](0006-mcp-protocol-strategy.md) | MCP 以 2026-07-28 为目标 + 旧版兼容 | 已接受 |
| [0007](0007-local-runtime-stack.md) | Local Runtime 采用 Rust 单二进制 | 已废弃（由 0014 替代） |
| [0008](0008-plugin-format.md) | 插件格式与签名 | 已接受 |
| [0009](0009-model-gateway-litellm.md) | 模型网关集成 LiteLLM | 已接受 |
| [0010](0010-data-store.md) | PostgreSQL + Redis + 对象存储 | 已接受 |
| [0011](0011-ui-stack.md) | React + Vite + XYFlow + Tailwind/shadcn | 已接受 |
| [0012](0012-open-source-reuse-and-licensing.md) | 开源复用与许可边界 | 已接受（法务复核） |
| [0013](0013-observability.md) | OpenTelemetry 作为可观测性标准 | 已接受 |
| [0014](0014-rust-core-runtime.md) | Rust 作为 Core Runtime | 已接受（替代 0002/0004/0007 语言部分） |
| [0015](0015-cross-language-contracts.md) | 跨语言契约与通信 | 已接受 |
| [0016](0016-script-plugin-execution.md) | Script 与 Plugin 执行 | 已接受 |
| [0017](0017-css-architecture.md) | CSS 架构 | 已接受 |
| [0018](0018-expression-engine.md) | 表达式引擎采用 jq/jaq 子集 | 已接受 |
| [0019](0019-vscode-copilot-integration.md) | VS Code + GitHub Copilot 作为增量集成入口 | 已接受（增量模块） |
| [0020](0020-permanent-internal-id-hpa.md) | hpa 作为永久内部 ID | 已接受 |

## 说明

- 状态：提议/已接受/已废弃/被替代。变更需新建 ADR，不直接改历史结论。
- 日期：2026-11-01。
- 证据：项目报告、研究 README、版本/许可证。
