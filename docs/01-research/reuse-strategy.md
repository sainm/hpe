# 复用策略

> 原则：复用成熟组件，但不把不同世界观强行拼在一起；平台的差异化必须自研。分类为 Direct Reuse / Adaptation / Integration / Design Reference / Do Not Use。

## 1. 复用清单

| 能力 | 选择 | 分类 | 具体动作 |
| --- | --- | --- | --- |
| 长期 durable execution | Temporal | Integration | Phase 3 PoC；`TemporalAdapter` 实现 `RuntimeAdapter`；IR 解释器 Workflow + 节点 Activity；不暴露 Temporal 类型 |
| Agent 状态图/中断 | LangGraph Python（可选 AI Worker） | Integration | 封装在 gRPC `AgentWorker`；Postgres/SQLite checkpointer；不让业务依赖 LangGraph 类型 |
| 模型网关 | LiteLLM | Integration | 通过 HTTP 调 MIT 部分；租户/密钥/预算映射；排除 EE |
| 可观测性 | OpenTelemetry | Direct Reuse | TS/Python SDK + Collector；定义 `hisha.*` 语义约定 |
| MCP 协议 | MCP Spec + 官方 SDK | Direct Reuse + Adaptation | SDK 做基础；自建 2026-07-28 无状态兼容层；支持旧版 |
| Skill 格式 | Agent Skills 标准 | Direct Reuse | 目录/frontmatter/渐进披露；`x-hisha` 扩展；导入/导出 |
| 工作流引擎 | 自研轻量运行时 | 自研 | IR 解释、检查点、恢复、审批、事件 |
| IR/编译器/校验器 | 自研 | 自研 | JSON Schema、静态分析、依赖/权限/策略校验 |
| 能力注册/权限 | 自研 | 自研 | 统一 Capability + 多入口绑定 + 每入口策略 |
| MCP Gateway | 自研 | 自研 | 租户端点、工具注册、OAuth、限流、审计、审批 |
| Skill Runtime/本地沙箱 | 自研 | 自研 | Loader/Resolver/Context/Executor/Sandbox/Artifacts |
| Hybrid Worker 协议 | 自研 | 自研 | 出站 mTLS、注册、心跳、任务投递、离线队列 |
| 连接器 | 自研 SDK + 少量内置 | 自研 | HTTP/DB/Git/File/Queue 等；其余市场 |
| 插件格式/签名 | 自研 | 自研 | manifest + SHA-256 + Ed25519/Sigstore + 权限 |
| UI 设计系统/画布 | 自研（React/XYFlow） | 自研 | 不复制 Dify/n8n/Flowise 的 UI 代码 |

## 2. 逐项目动作

### Direct Reuse

- **OpenTelemetry**：依赖 `@opentelemetry/sdk-node`、`@opentelemetry/auto-instrumentations-node`（版本以 lockfile 为准）；Collector 配置随部署交付。
- **Agent Skills 标准**：以 `agentskills.io/specification` 为规范；`SKILL.md` 字段、目录约定、校验。
- **MCP Spec**：以 2026-07-28 为目标；旧版作为兼容。规范文本引用遵守 CC-BY-4.0。
- **awesome-copilot**：固定快照作导入器/兼容性测试语料；不打包内容。
- **MCP Servers**：固定 tag 作 conformance/回归夹具；生产不内置。

### Integration

- **Temporal**：Phase 3 PoC；MIT；锁定 server/SDK 版本；只在 `durable` 适配器中使用。禁止把 Temporal 的 Workflow 代码/类型泄漏到 IR 或 API。
- **LangGraph Python**：`langgraph 1.2.x` + `langgraph-checkpoint-postgres/sqlite`；锁定版本；封装在 Python AI Worker/gRPC；没有 Python 时 Agent 禁用。
- **LiteLLM**：MIT 部分；Docker/Helm；通过 OpenAI 兼容 API；Pro/EE 功能不启用；预算/密钥双写由 Hisha 主导。
- **MCP 协议实现**：Rust RMCP 3.8.x（Apache-2.0，官方 Rust SDK）负责 Client/Server/Gateway 协议/传输/认证（2026-07-28 + 2025-11-25）；TS Control Plane 只做管理/审计，不实现协议。
- **Graphon（可选）**：如果未来需要 Python Worker/复杂的图执行，可作为 Python 侧执行内核；Apache-2.0；不进入 TS 主栈。

### Adaptation

- **Activepieces Piece/Waitpoint**：借鉴分层与暂停语义，重写 Hisha Connector/Runtime；不复制代码。
- **Flowise MCP 双向入口**：借鉴“平台既是 MCP Server 又是 MCP Client”的产品化；不复制 agentflow。
- **Kestra Flow-as-Tool/Worker Selector**：借鉴 MCP 工具暴露与 Worker 路由；可做只读 YAML 导入器。
- **Node-RED 本地体验**：借鉴单进程/本地目录/离线；执行模型重写。

### Design Reference

Dify、n8n、Kestra、Flowable、Node-RED、Flowise、Airflow、Dagster：只读研究，文档标注来源；不复制代码/UI/节点实现。具体可借鉴点见各项目报告 §15。

### Do Not Use

- n8n 代码/节点/前端（SUL）。
- Dify 代码（商业多租户/LOGO 限制）。
- Activepieces/Flowise/LiteLLM 的 EE/enterprise 目录。
- Anthropic docx/pdf/pptx/xlsx 与 doc-coauthoring。
- MCP 参考服务器作为生产实现。
- 任何 AGPL/SSPL/未知许可证依赖进入核心。
- 任何来源不明的 Skill/Plugin 无审核进入市场。

## 3. 建 vs 买

| 能力 | 自研 | 集成 | 理由 |
| --- | --- | --- | --- |
| Canonical IR/编译器/校验器 | 是 | — | 产品核心，决定 Canvas/YAML/JSON 一致 |
| 轻量运行时 | 是 | — | 本地优先、IR 语义、可测试 |
| durable 长任务 | 否 | Temporal | 避免重造事件溯源 |
| Agent 状态图 | 否 | LangGraph | 成熟 interrupt/checkpoint；但封装 |
| 模型网关 | 否 | LiteLLM | provider 适配/预算成熟 |
| 可观测性 | 否 | OTel | 标准/生态成熟 |
| MCP 协议基础 | 是 | RMCP 3.8.x（Rust） | 官方 Rust SDK；2026-07-28 + 旧版；Hisha 加租户/策略/审批 |
| Skill 运行时/沙箱 | 是 | — | 差异化与安全边界 |
| 能力注册/权限 | 是 | — | 统一入口/治理是核心 |
| UI/UX | 是 | — | 现代开发者工作台是产品差异 |
| 连接器 | 是（SDK + 核心） | 市场 | 数量不可能自研全；核心自建 |
| 插件/市场 | 是 | — | 供应链/签名/权限 |

## 4. 依赖治理

- 所有直接复用依赖锁定精确版本 + lockfile；
- 每个依赖记录：用途、许可、维护者、升级策略、退出方案；
- 核心路径禁止传递依赖进入（例如禁止把 LangChain 全家桶扩散到 Control Plane）；
- 每季度做一次升级/许可证/SBOM 审计；
- 任何新依赖需 ADR 或至少 PR 中的 license/替代方案说明。

## 5. 反模式

- 为了“统一技术栈”把 Kestra/Temporal/Airflow 同时引入。
- 为了“快速上线”复制 n8n 节点或 Dify 前端。
- 把 LangGraph 图当 Workflow IR，或把 Workflow 当 Agent 循环。
- 把 MCP SDK 当成“已支持 2026-07-28”。
- 把 Skill 当可执行插件，或让 Skill 脚本拥有不受限权限。
- 把 EE 代码/专有 Skill 放进仓库后再做“清理”。

## 6. 相关决策

- [ADR-0012 开源复用与许可](../adr/0012-open-source-reuse-and-licensing.md)
- [ADR-0002 执行引擎策略](../adr/0002-execution-engine-strategy.md)
- [ADR-0004 Agent Runtime](../adr/0004-agent-runtime-langgraph.md)
- [ADR-0006 MCP 协议策略](../adr/0006-mcp-protocol-strategy.md)
