# Flowise 3.1.4 研究报告

> 仓库：`FlowiseAI/Flowise` · tag `flowise@3.1.4`（2026-07-29）· tarball SHA-256 `77ab8115dd39f031...`<br>
> 研究方式：TypeScript monorepo 源码（2,445 文件）+ LICENSE + server/components/agentflow 代码；未构建运行。<br>
> 结论：**Design Reference（AI 画布/节点库/MCP 入口）**；agentflow 不直接复用。

## 1. 项目定位

可视化构建 LLM 应用/Agent 的平台：Chatflow、Agentflow、工具/检索/向量库节点、 marketplace、API/SDK、MCP。它是“AI 节点画布”，不是确定性通用 Workflow 平台。

## 2. 核心功能

- Chatflow/Agentflow 可视化编排；LLM、Prompt、Memory、Retriever、Vector Store、Tool、MCP 等节点。
- Marketplace / 模板；API、嵌入组件、SDK；评估、观测、队列。
- MCP：MCP Server（平台暴露工具）与 Custom MCP 客户端节点。
- 凭据、变量、会话、文档存储、数据集。
- 企业功能：SSO、RBAC、审计、Workspace（`server/src/enterprise/*`）。

## 3. 技术架构

- TypeScript monorepo：`packages/server`（Express、TypeORM、BullMQ、Redis）、`packages/components`（26 个节点类别）、`packages/ui`、`packages/agentflow`、`packages/observe`。
- `server/src/NodesPool.ts` 动态加载节点；`buildChatflow.ts` 执行；`queue/PredictionQueue.ts` 处理异步预测。
- `packages/agentflow` 是独立的 React 画布包（版本 `0.0.0-dev.14`，peer React 18/reactflow 11）。
- MCP：`controllers/mcp-server`、`mcp-endpoint`、`custom-mcp-servers`，以及 `components/nodes/tools/MCP/*`。

## 4. 技术栈

Node.js/TypeScript、Express、TypeORM、Postgres、Redis、BullMQ、React 18、MUI、reactflow 11、Redux、TipTap、CodeMirror。核心 Apache-2.0；enterprise 目录商业许可。

## 5. 数据模型（从代码可见）

Chatflow/Agentflow（flowData JSON）、Credential、Tool、DocumentStore、ChatMessage、ChatFlow 运行、Dataset、Evaluation、MCP Server、Custom MCP、Workspace/User（企业）。Flow 以 JSON 节点/边存储。

## 6. 执行机制

- `buildChatflow` 递归/顺序执行节点，节点 `init`/`run` 返回输出；支持条件、循环/Agent 节点。
- 预测队列（BullMQ）异步处理，Redis 缓存/队列；流式返回。
- 执行以 AI 链为中心，不保证确定性/事件溯源；没有工作流版本快照/审批的完整语义。
- MCP 端点把 Chatflow/工具暴露给外部客户端。

## 7. 扩展机制

- 节点：`packages/components/nodes/**`，通过 NodesPool 加载；节点定义含输入/输出/凭据。
- Marketplace 模板；Custom Tool；Custom MCP；API/SDK。
- 企业 SSO/RBAC/审计是闭源目录。

## 8. 插件开发方式

编写 TS 节点类，注册到 components；也可通过 Marketplace 模板/自定义工具扩展。没有独立的插件 manifest/签名体系；企业功能不可用。

## 9. API 设计

Express REST：chatflows、nodes、credentials、tools、variables、document-store、prediction、MCP server/endpoint、custom MCP；SDK 包；OpenAPI（api-documentation）部分覆盖。API 以 Chatflow/预测为中心。

## 10. 部署架构

Docker、Docker Compose（含 queue/worker）、Postgres、Redis；可单机或队列模式。没有完整的 Helm 一等支持。

## 11. 前端交互设计

React 18 + MUI 的经典节点画布：左侧节点库、中间 reactflow 画布、右侧节点配置；Marketplace、API 文档、聊天预览。信息密度较低，视觉偏传统后台；agentflow 包试图抽出画布但仍是 dev 版本。

## 12. 许可证

根 `LICENSE.md`：

- `packages/server/src/enterprise` 与带明确版权声明的文件适用 Commercial License。
- 其余内容 Apache-2.0。
- 第三方组件沿用原许可。

因此核心可商用/修改，但企业目录不可用；`agentflow` 包为 Apache-2.0（package.json）但仍是 dev 版本且 peer React 18/reactflow 11。法律结论需法务复核。

## 13. 可直接复用

- 设计：AI 节点分类、Marketplace、Chatflow/Agentflow 双模式、MCP 服务器/客户端双向入口。
- 代码：不建议直接复用 agentflow（dev 版本、React 18/MUI/reactflow 11 与 Hisha React 19/XYFlow 12 不匹配、耦合 Flowise 组件）。
- 可参考其 MCP 端点/自定义 MCP 的产品化方式。

## 14. 需要二次开发

若复用 agentflow：需要升级 React/XYFlow、剥离 MUI/Flowise 依赖、重写节点 schema、补版本/权限/审批；成本接近自研画布。

## 15. 仅值得借鉴

- AI 节点的输入/输出/凭据 schema 组织。
- Chatflow 与 Agentflow 的产品分层。
- MCP Server + Custom MCP 客户端双向能力。
- Marketplace 模板/节点分发。
- 队列模式与聊天流式体验。

## 16. 不适合采用

- 作为 Hisha 的 Workflow 引擎（AI-centric、无强 IR/版本/审批）。
- 直接依赖 agentflow（dev 版本、技术栈不匹配）。
- enterprise 目录。
- MUI + React 18 的 UI 体系。

## 17. 与 Hisha 的兼容性

概念中高（AI 画布、节点、MCP），代码低。Hisha 的 Workflow Designer 应为确定性 IR 服务，AI 节点只是其中一类；不能把 Flowise 的 Chatflow 模型当 Canonical IR。

## 18. 引入后的维护成本

作为依赖：高（Express/TypeORM/MUI/React 18/dev 画布/EE 边界）。作为参考：低。Hisha 不引入。

## 证据路径

`LICENSE.md`、`package.json`、`packages/server/package.json`、`packages/server/src/NodesPool.ts`、`packages/server/src/utils/buildChatflow.ts`、`packages/server/src/queue/PredictionQueue.ts`、`packages/server/src/controllers/mcp-server/*`、`packages/server/src/controllers/mcp-endpoint/*`、`packages/server/src/controllers/custom-mcp-servers/*`、`packages/components/nodes/*`、`packages/agentflow/package.json`、`packages/ui/package.json`、`docker/*`。
