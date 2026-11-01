# 开源项目对比矩阵

> 版本与哈希见 [研究 README](README.md)。评分为工程判断：强/中/弱/无，不是营销结论。

## 1. 总表

| 项目（版本） | 定位 | 流程模型 | 执行/持久化 | 扩展机制 | 前端 | 部署重心 | 许可证类别 | 对 Hisha 的价值 | 维护成本 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Dify 1.18.0 | AI 应用/Workflow 平台 | 画布 + 应用 DSL；节点含 AI/知识/人工 | graphon 帧执行 + Celery；暂停恢复 | 插件 daemon | Next.js/React/React Flow/Monaco | Docker Compose | 修改版 Apache | 设计参考；低代码 AI UX、Agent/Workflow 分体 | 高 |
| n8n 2.45.7 | 通用 SaaS 自动化 | 节点 JSON + 连线；表达式 | 节点执行 + Bull/Redis 队列；非确定性 | npm 节点包/凭据 | Vue 3 + 自研画布 | npm/Docker | SUL + EE | Do Not Use Code；节点/版本/队列参考 | 极高（法律） |
| Kestra 2.0.7 | YAML 数据/运维编排 | YAML Flow/Task/Trigger | Executor/Worker/Scheduler/Queue；replay from task | Maven 插件 | Vue 3 + Vue Flow + Monaco | Docker/Helm | Apache-2.0 | 设计参考；可选 Kestra 导入/适配 | 高 |
| Activepieces 0.92.3 | SaaS 自动化 + Pieces | Flow JSON + 步骤 | Fastify/Worker + isolated-vm 沙箱 + Waitpoint | Piece SDK | React 19 + XYFlow 12 | Docker Compose | MIT + EE | 设计参考；Piece/Waitpoint/MCP 工具面 | 中高 |
| LangGraph 1.2.19 | Agent 状态图库 | 代码图 + channels | Pregel 超步 + checkpoint/interrupt | 节点/工具/checkpointer | 无 | 库/宿主 | MIT | **Agent Runtime 内核** | 中 |
| Temporal 1.32.1 | Durable execution | 代码 Workflow/Activity | 事件溯源 + 确定性重放 + Signals | SDK/Activity/Nexus | 无（Web 独立） | 服务端 + DB | MIT | **长期 durable 后端** | 高 |
| Flowable 8.0.0 | BPMN/CMMN/DMN 引擎 | BPMN XML | Token 执行 + JobExecutor + 人工任务 | Java/Spring | 商业 Design/Work | Java/Spring/K8s | Apache-2.0 | 设计参考；审批/任务生命周期 | 高 |
| Node-RED 5.0.9 | 本地事件流 | Flow JSON + msg 传递 | 单进程事件驱动；无检查点 | npm 节点包 | Vanilla JS/Ace | npm/Docker | Apache-2.0 | 设计参考；本地轻量体验 | 低（但不满足语义） |
| Flowise 3.1.4 | AI 画布/Agent | Chatflow/Agentflow JSON | 节点执行 + BullMQ；AI 中心 | TS 节点/市场 | React 18/MUI/reactflow 11 | Docker | Apache + EE | 设计参考；AI 节点/MCP 双向 | 中高 |
| Airflow 3.3.2 | 数据 DAG 调度 | Python DAG | Scheduler + Task SDK/API + Executor | Provider | React 19/Chakra/XYFlow | Docker/Helm | Apache-2.0 | 设计参考；调度/Provider/HITL | 高 |
| Dagster 1.13.28 | 资产编排 | Python assets | Run coordinator/launcher/executor + 事件日志 | Python 资源/IO | React/GraphQL | Helm/K8s | Apache-2.0 | 设计参考；血缘/分区/质量 | 高 |
| LiteLLM 1.106.6 | LLM 网关 | 无 | Proxy/Router/预算/限流/MCP Gateway | Provider/hooks | React 19 | Docker/Helm | MIT + EE | **Model Gateway 集成** | 中 |
| OpenTelemetry 1.62.0 | 可观测性规范 | 无 | SDK/Collector/OTLP | 信号/导出器 | 无 | 规范/SDK | Apache-2.0 | **直接采用** | 低-中 |
| MCP Servers 2.2.0 | MCP 参考服务器 | 无 | stdio/HTTP handlers | MCP SDK | 无 | 本地/HTTP | MIT→Apache 过渡 | 测试夹具；非生产 | 低 |
| Anthropic Skills main | Agent Skills 标准/示例 | 无（知识包） | 宿主加载；脚本由宿主执行 | Skill 文件夹/市场 | 无 | 文件/市场 | 混合（14 Apache/4 专有/1 无） | **标准直接采用**；示例部分可用 | 低 |
| awesome-copilot main | Copilot 内容生态 | 无（内容） | 无 | agents/skills/plugins | 无 | 仓库/市场 | MIT | 兼容性测试语料 | 低 |

## 2. 按能力域比较

### 2.1 Workflow 引擎

| 维度 | Kestra | Temporal | 自研轻量运行时 | LangGraph | Airflow | Dagster | Flowable | Node-RED |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 上层 IR 兼容 | 中（自有 YAML） | 低（代码 Workflow） | 高（为 IR 而生） | 低（Agent 图） | 低（Python DAG） | 低（资产） | 低（BPMN） | 低（msg 图） |
| 条件/循环/并行 | 强 | 代码实现 | 强（IR 容器） | 强（图） | 中（DAG） | 中（资产） | 强（BPMN） | 中（msg） |
| 长任务/恢复 | 中（DB 状态 + replay） | 强（事件溯源） | 中（检查点；非确定性重放） | 中（checkpoint） | 中 | 中 | 中 | 弱 |
| 人工审批 | 中（Pause） | 强（Signal/Update） | 强（IR 审批节点） | 强（interrupt） | 中（HITL） | 弱 | 强（用户任务） | 弱 |
| 本地部署 | 中（JVM） | 中重（server+DB） | 强（单进程） | 强（库） | 重 | 重 | 重 | 强 |
| 多租户 | 中（Namespace/Tenant） | 中（Namespace） | 自建 | 自建 | 弱 | 弱 | 中 | 弱 |
| 扩展语言 | Java | 多语言 SDK | TS | Python/TS | Python | Python | Java | JS |
| 许可证 | Apache-2.0 | MIT | 自有 | MIT | Apache-2.0 | Apache-2.0 | Apache-2.0 | Apache-2.0 |
| Hisha 结论 | 设计参考/可选适配 | 长期 durable 适配 | **v1 内核** | Agent 内核 | 设计参考 | 设计参考 | 设计参考 | 设计参考 |

### 2.2 Agent 运行时

| 维度 | LangGraph | Dify Agent | n8n Agents | Flowise | Kestra AI |
| --- | --- | --- | --- | --- | --- |
| 状态/检查点 | 强（checkpointer） | 强（session store） | 中 | 中 | 中 |
| interrupt/resume | 强 | 强（human input） | 中 | 中 | 中 |
| 多 Agent | 强（子图/handoff） | 中 | 中 | 中 | 中 |
| 工具/MCP | 通过宿主 | 插件/MCP | MCP 节点 | MCP 节点 | MCP/工具 |
| 多租户 | 无（宿主） | 强 | 强 | 强 | 强 |
| 许可 | MIT | 修改 Apache | SUL | Apache+EE | Apache |
| Hisha | **采用为内核** | 设计参考 | Do Not Use | 设计参考 | 设计参考 |

### 2.3 集成/标准

| 项目 | 可复用层 | 风险 | Hisha 方式 |
| --- | --- | --- | --- |
| LiteLLM | 模型网关 | EE 边界、版本快、双预算 | HTTP 集成，MIT 部分 |
| OpenTelemetry | Trace/Metrics/Logs/OTLP | 基数/成本 | 直接采用 |
| MCP Servers | 参考/测试 | 非生产 | 测试夹具 |
| MCP Spec | 协议 | 2026-07-28 与 SDK 滞后 | 自建兼容层 + SDK |
| Agent Skills | 格式/示例 | 混合许可 | 标准 + Apache 示例 |
| awesome-copilot | 语料 | 质量/许可 | 测试语料 |

## 3. 关键发现

1. **没有单一项目同时满足 IR、可视化、Agent、Skill、MCP、Local-first、企业治理**。每个项目只覆盖 2-4 个能力域。
2. **工作流引擎分成三种世界观**：代码/事件溯源（Temporal）、声明式 YAML/插件（Kestra）、消息/节点（n8n/Node-RED）。Hisha 必须有自己的 IR，用适配器连接，而不是选一个项目当模型。
3. **Agent 与 Skill 正在融合，但 Workflow 没有被替代**：Dify、n8n、Flowise 都在 Workflow 画布中加入 Agent 节点与 Skill；Hisha 必须保持两者执行模型分离，只在节点/工具边界互相调用。
4. **MCP 是增长最快的接口，但协议在 2026 年发生了不兼容变更**：2026-07-28 移除会话/initialize、引入 `server/discover`、MRTR、`subscriptions/listen`；官方 TS SDK 1.32.2 仍以 2025-11-25 为稳定版，而 Rust RMCP 3.8.0 已支持 2026-07-28。任何“直接用 SDK 就支持最新 MCP”的说法都不成立；Hisha 用 Rust RMCP 并做 conformance。
5. **本地执行是普遍短板**：Flowise/n8n/Activepieces 的本地运行仍依赖数据库/Redis/队列；Temporal/Kestra 需要 JVM/服务端；Node-RED 最轻但缺少 IR/durable/审批。Hisha 的 Local Runtime 是差异化机会。
6. **许可证是硬约束**：n8n 不能商用复制；Dify 多租户需商业授权；多项目的 EE 目录不可用；Anthropic 文档 Skill 不可再分发。复用策略必须与法务/发布形态绑定。

## 4. 五类复用结论

| 分类 | 项目 | 具体动作 |
| --- | --- | --- |
| **Direct Reuse** | OpenTelemetry、Agent Skills 标准、awesome-copilot（测试语料）、MCP Spec（协议） | 直接依赖/遵循标准；保留许可与版权；固定版本 |
| **Integration** | Temporal、LangGraph（Python）、LiteLLM、Rust RMCP、MCP Servers（测试） | 通过适配器/HTTP/库集成；封装在 Hisha 契约后；版本锁定 |
| **Adaptation** | Graphon（可选 Python 引擎）、Activepieces Piece/Waitpoint 概念、Flowise MCP 双向模式 | 只借鉴或重写；不直接搬运行时代码 |
| **Design Reference** | Dify、n8n、Kestra、Flowable、Node-RED、Flowise、Airflow、Dagster | 研究 UX/语义/架构；不复制代码；文档标注来源 |
| **Do Not Use** | n8n 代码、Dify 代码（商业 SaaS）、Activepieces/Flowise/LiteLLM 的 EE 目录、Anthropic 专有 Skill、MCP 参考服务器（生产） | 明确禁止进入产品仓库；CI 许可证门禁阻断 |

> 分类允许“主分类 + 备注”；例如 Kestra 主分类是 Design Reference，同时保留一个可选的 Integration 适配器计划。任何跨分类变更需 ADR。

## 5. 不应组合的方案

- Dify + Kestra + Temporal 三引擎同时引入：三套模型、三套部署、三套权限，维护不可控。
- n8n 节点 + Activepieces Pieces + Flowise 节点混合：三种扩展 ABI、三套许可证、三套 UI schema。
- LangGraph 图 + Hisha IR 双写：两套流程真相，必然漂移。
- 用 MCP 参考服务器充当平台内置工具：安全/生产性不足。
- 用 LiteLLM enterprise 解决 SSO/RBAC：许可与身份边界冲突。

## 6. 结论

Hisha 的组合应是：

- **一个自有 IR + 编译器/校验器**（核心资产）。
- **一个自有轻量运行时**（本地/标准工作流）。
- **Temporal 适配器**（长期 durable，Phase 3 后 PoC）。
- **LangGraph 作为 Agent 内核**（封装在 AgentRuntime 后）。
- **LiteLLM 作为模型网关**（MIT 部分）。
- **OTel 作为可观测性**。
- **Rust RMCP 3.8**（2026-07-28 + 2025-11-25）作为 MCP 协议实现；Hisha 加租户/策略/审批/审计。
- **Agent Skills 标准作为 Skill 格式**。
- 其余项目只做设计参考/测试语料。

这不是“技术统一”的强行组合，而是按职责边界选择最少依赖。
