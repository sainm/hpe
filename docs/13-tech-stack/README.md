# 技术栈方案

> 状态：Phase 0 技术选型基线（2026-11-01）。<br>
> 结论：**Rust Core + TypeScript Control Plane/Web + Python 可选 AI Worker**；内部 gRPC，外部 REST/OpenAPI，AI 工具 MCP，数据契约 JSON Schema。
> 品牌：展示名 Auralis；永久内部 ID **hpa**；历史别名 Hisha。规则见 [BRANDING.md](../../BRANDING.md)。
> 说明：本机未安装 Rust/Python 目标版本，未编译；版本/许可证来自 crates.io/npm/PyPI/GitHub 源码核查，详见 [证据](evidence.md)。

## 1. 最终组合

| 层 | 语言 | 版本 | 职责 |
| --- | --- | --- | --- |
| 执行内核 | Rust | 1.99.x（MSRV 1.97） | IR/编译器/校验器、Runtime、Local Runtime、Skill/MCP/Plugin/Script/Process/Resource/Security |
| Control Plane/Web | TypeScript | 5.9.3 + Node 26.13 LTS | NestJS API、React UI、Designer、Registry、版本/权限/审批、TS SDK；VS Code 集成为增量 app |
| AI（可选） | Python | 3.13 | LangGraph Agent Worker、模型适配、Python Skill 脚本 |
| 契约 | Proto/JSON Schema/OpenAPI | — | gRPC/IR/API/工具描述 |
| 样式 | CSS3/Tailwind/PostCSS/CSS Modules | Tailwind 4.3.3 | Design System/主题/响应式 |
| 存储 | Postgres/SQLite/Redis/S3 | — | 控制面/执行/队列/产物 |
| 观测 | OpenTelemetry | OTel Rust 0.33/TS SDK | Trace/Metrics/Logs |

## 2. 12 项交付映射

| # | 交付 | 文档 |
| --- | --- | --- |
| 1 | 最终语言组合及理由 | [语言策略](language-strategy.md) |
| 2 | 各语言模块职责 | [语言策略 §3-4](language-strategy.md#3-模块职责) |
| 3 | 完整技术栈清单 | [Rust](rust-stack.md)、[TypeScript](typescript-stack.md)、[Python](python-stack.md)、[CSS](css-architecture.md) |
| 4 | 核心技术候选比较 | [Rust §3](rust-stack.md#3-关键-crate-评估)、[TypeScript §8](typescript-stack.md#8-候选比较)、[Python §3](python-stack.md#3-与-rust-的集成)、[CSS §1.2](css-architecture.md#12-候选比较) |
| 5 | 跨语言通信架构 | [跨语言通信](cross-language.md) |
| 6 | Script/Plugin 执行架构 | [Script/Plugin](script-plugin-execution.md) |
| 7 | Local Runtime 技术方案 | [Local Runtime](local-runtime-rust.md) |
| 8 | Windows/macOS/Linux 方案 | [跨平台](cross-platform.md) |
| 9 | Monorepo 目录 | [Monorepo](monorepo-build-release.md#1-仓库结构) |
| 10 | 开发/构建/测试/发布 | [Monorepo](monorepo-build-release.md) |
| 11 | 技术/依赖/维护风险 | [风险](risks.md) |
| 12 | MVP 最小技术栈 | [MVP](mvp-stack.md) |

补充：[CSS 架构](css-architecture.md)、[证据](evidence.md)。

## 3. 关键决策

1. **Rust 核心但不全 Rust**：执行/IR/沙箱/资源/安全在 Rust；管理/UI/AI 在 TS/Python。
2. **TS Control Plane 不执行流程**：NestJS 通过 gRPC 调 Rust `RuntimeService`；前端不做执行。
3. **Python 可选**：LangGraph AI Worker 独立进程；没有 Python 时确定性 Workflow/Skill 仍可用。
4. **MCP 由 Rust RMCP 实现**：RMCP 3.8.0 已支持 2026-07-28 + 2025-11-25；TS 只做管理/UI。这修正了上一轮“TS SDK 不支持 2026-07-28 需自建协议层”的结论。
5. **内部 gRPC，外部 REST/OpenAPI，AI 工具 MCP**：不混用；浏览器只用 REST/SSE。
6. **IR 语言无关**：JSON Schema 是源；Rust 校验/编译；TS 类型生成；不绑定 Rust 类型。
7. **Local Runtime 是 Rust 单二进制**：嵌入 Web UI/SQLite；本地控制面子集；不依赖 NestJS/云。
8. **CSS 架构**：Tailwind 4 + CSS Variables + CSS Modules + PostCSS；无运行时 CSS-in-JS。
9. **MVP 只做本地 Workflow**：Rust + React + SQLite；NestJS/Python/MCP/WASM/Temporal 后续增量。

## 4. 技术栈一览

### Rust

- tokio 1.54.1、axum 0.8.9、serde 1.0.229、saphyr 0.3.3、jaq-core 3.1.2、schemars 1.2.3、jsonschema 0.60.2、sqlx 0.9.0、tonic 0.14.9、tracing 0.1.44、wasmtime 50.0.1、rmcp 3.8.0。
- opentelemetry 0.33.1 + tracing-opentelemetry 0.34.0 + opentelemetry-otlp 0.33.1。
- 支撑：reqwest/rustls、jsonwebtoken、keyring、directories、sysinfo、nix/landlock/seccompiler/windows-sys、rust-embed、thiserror、clap、uuid/time。
- 工具：rustfmt/clippy/nextest/cargo-deny/cargo-audit/sccache/cargo-dist。

### TypeScript

- Node 26.13 LTS、TypeScript 5.9.3（7.0.2 评估）、Vite 8.3.5、React 19.3.1、NestJS 12.3.2、Prisma 7.10.1。
- VS Code/Copilot 增量集成：`@types/vscode 1.140.0`、`engines.vscode ^1.143.0`（1.143.1 实测）、`@vscode/vsce 4.1.0`、esbuild 0.28.2；见 [VS Code 模块](../14-vscode-copilot/README.md)。
- XYFlow 12.12、Monaco 0.57、TanStack Query 5.105、Zustand 5.0.16、Zod 4.6.7、RHF 7.91。
- Tailwind 4.3.3、PostCSS 8.5.32、CSS Modules、Radix 1.8.1、shadcn 4.23、CVA/clsx/tailwind-merge。
- i18next 26.5、openapi-typescript 7.13、openapi-fetch 0.17、ts-proto 2.18、@grpc/grpc-js 1.14。

### Python

- Python 3.13、LangGraph 1.2.19、Pydantic 2.14.2、FastAPI 0.162.0（可选）、LiteLLM 1.106.6（独立网关）、grpcio 1.85.1、protobuf 7.35+。
- LangGraph checkpoint Postgres 3.2.0/SQLite 3.1.3；uv 0.13.6、pytest 9.2、ruff 0.17、mypy 2.4。

## 5. 通信/执行选择

| 场景 | 方案 |
| --- | --- |
| 浏览器 ↔ Control Plane | REST/OpenAPI 3.1 + SSE |
| NestJS ↔ Rust | gRPC/Protobuf（Tonic/ts-proto） |
| Rust ↔ Python | gRPC/Protobuf（tonic/grpcio） |
| Rust ↔ 外部 MCP | RMCP（2026-07-28 + legacy） |
| 异步任务/事件 | Postgres outbox + Redis Streams（MVP）；NATS 后续 |
| 数据契约 | JSON Schema 2020-12（IR/工具/插件） |
| 外部 AI 客户端 | MCP Gateway + 每客户端配置 |

## 6. 不采用

- n8n/Dify/EE/专有 Skill 代码（许可）。
- 用 TypeScript/Node 作为 Workflow/脚本主执行器。
- 用 Python/LangGraph 作为平台唯一执行引擎。
- 同时引入 Kestra + Temporal + 自研多引擎。
- 运行时 CSS-in-JS（styled-components/Emotion）。
- 微服务化/多语言 FFI/共享内部对象。

## 7. 状态/限制

- 本机：Rust/Cargo/uv/pnpm 未安装；Node 26.0；Python 3.9.6。未编译/运行。
- 未做三平台构建/签名/公证；未做性能基准；未做全量传递许可证扫描。
- Mermaid/CSS 未浏览器渲染；PoC 未执行。
- 任何版本/许可结论在引入前复验；商业发布前法务复核。

## 8. 相关

- [证据](evidence.md)
- [MVP](mvp-stack.md)
- [风险](risks.md)
- [系统架构](../02-architecture/system-architecture.md)
- [ADR-0014 Rust Core](../adr/0014-rust-core-runtime.md)
- [ADR-0015 跨语言契约](../adr/0015-cross-language-contracts.md)
- [ADR-0016 Script/Plugin](../adr/0016-script-plugin-execution.md)
- [ADR-0017 CSS](../adr/0017-css-architecture.md)
- [ADR-0018 表达式引擎](../adr/0018-expression-engine.md)
