# n8n 2.45.7 研究报告

> 仓库：`n8n-io/n8n` · tag `n8n@2.45.7`（2026-10-30）· tarball SHA-256 `b4c2ce499557eab0...`<br>
> 研究方式：源码 tarball（31,920 文件）+ LICENSE/LICENSE_EE + 执行/节点/队列代码；未构建运行。<br>
> 结论：**Do Not Use Code；Design Reference**（SUL 商业限制，见 §12）。

## 1. 项目定位

成熟的通用工作流自动化平台（“fair-code”），以 400+ 集成和可视化节点编辑器为核心，近年加入 AI Agent、LangChain 节点、MCP Client/Server 与内置 Agent Skills。产品形态最接近 Hisha 的 Workflow Center，但执行模型、许可与语言栈不同。

## 2. 核心功能

- 可视化节点/连线编辑、表达式、凭据、子工作流、错误工作流、Webhook/Schedule/轮询触发。
- 节点与凭据：`packages/nodes-base/nodes` 309 个节点目录、`credentials` 412 个凭据定义。
- 执行历史、部分执行、固定数据（pin data）、等待节点、并发控制。
- Queue Mode（Bull + Redis）与独立 task runner 进程（Node/Python）。
- AI：`packages/@n8n/nodes-langchain`、`packages/cli/src/modules/agents`、`packages/@n8n/agents`、`packages/@n8n/instance-ai/skills/*`。
- MCP：`McpClient`、`McpTrigger`、`McpRegistryClientTool`、`McpOAuth2Api`，以及 `@n8n/mcp-apps`。
- Public REST API（OpenAPI）与 Webhook。

## 3. 技术架构

- TypeScript monorepo + pnpm/turbo；`packages/cli` 是 Express 服务器与运行时入口；`packages/core`、`packages/workflow`、`packages/nodes-base`。
- 前端：Vue 3 + `@n8n/design-system`（`packages/frontend/editor-ui`、`packages/frontend/@n8n/*`）。
- 执行：`packages/cli/src/workflow-runner.ts`、`workflows/workflow-execution.service.ts`、`active-executions.ts`；节点执行上下文在 `packages/core/src/execution-engine`。
- 队列：Bull 4.16 + ioredis；Queue Mode 将执行分发到 worker。
- 数据库：Postgres/SQLite；凭据加密；二进制数据存储。
- Task runner：独立进程，按语言启动（`task-runner-process-js.ts`/`py.ts`），用于 Code 节点等不可信代码；有 OOM/重启检测。

## 4. 技术栈

TypeScript、Node.js、Express、Vue 3、Bull/Redis、Postgres/SQLite、Jest/Vitest、Playwright（e2e）、OpenAPI。插件是 npm 包（`n8n-nodes-*`）。没有 JVM/Python 主栈依赖。

## 5. 数据模型（从代码可见）

Workflow（`nodes`、`connections`、`settings`、`pinData`、版本历史）；Execution（状态、模式、数据、错误、时间）；Credentials（加密实体）；Project/Tag；ActiveWorkflow；Agent/Thread/Message；MCP Server 配置。工作流以 JSON 存储，节点配置与连接是同一文档。

## 6. 执行机制

- 以节点为单位的前端编排 + 后端 `WorkflowExecute` 驱动；按连接顺序/分支执行，支持部分执行、等待与恢复。
- 表达式在 `packages/workflow/src/expression.ts`、`expressions/*` 中解析/求值；表达式安全在 `expression-sandboxing.ts`。
- 队列模式下执行被 Bull 分配到 worker；有 active execution 跟踪、超时、重试、错误工作流。
- 不是事件溯源/确定性重放引擎；长任务依赖 wait/webhook 与数据库状态，崩溃恢复能力弱于 Temporal。
- Task runner 是独立沙箱边界，但 Code 节点仍拥有较宽的运行时能力。

## 7. 扩展机制

- `INodeType`（`packages/workflow/src/interfaces.ts`）：`execute`、`poll`、`trigger`、`webhook`、`methods.loadOptions/listSearch/credentialTest`。
- 版本化节点：`VersionedNodeType` 按数字版本选择实现（`versioned-node-type.ts`）。
- 凭据类型 + `credentialTest`；节点描述生成 UI。
- 社区节点包：npm 包 + node-dev 工具；企业功能在 `.ee.` 文件/目录。

## 8. 插件开发方式

开发 `n8n-nodes-<name>` npm 包，导出 `INodeType` 实现与 credentials；通过 `packages/node-dev` 构建；安装到实例的 community nodes 目录。没有独立 manifest/签名体系（社区节点风险由用户承担）。

## 9. API 设计

Public API `/api/v1`，`packages/cli/src/public-api/v1/openapi.yml`；覆盖 workflows、executions、credentials、projects、tags 等。Webhook 触发器直接暴露用户路径。内部 API 与前端共享 Express 控制器。API 以工作流/执行为中心，不是统一能力注册。

## 10. 部署架构

npm 全局安装、Docker、Queue Mode（主/worker/Redis/Postgres）。Helm 在外部仓库；本快照只有 `.github/workflows/test-e2e-helm.yml`。支持 SQLite（单机）与 Postgres（生产）。

## 11. 前端交互设计

Vue 3 编辑器：左侧节点面板、中间画布、右侧参数面板、底部执行数据；表达式编辑器与固定数据；执行历史列表/详情。交互成熟，但画布与节点 UI 是自研组件，不是 React Flow。AI Agent 与 Workflow 编辑器开始融合，存在概念混杂风险。

## 12. 许可证

根 `LICENSE.md`：

- “Content of branches other than the main branch (i.e. "master") are not licensed.”——非 master 分支内容未授权。
- `.ee.` 文件名或 `.ee` 目录下的代码不属于 SUL，必须持有 n8n Enterprise License。
- 其余内容适用 **Sustainable Use License 1.0**：仅可为自己的内部业务目的、非商业或个人用途使用/修改；只有免费且非商业目的才可分发给他人；不得移除许可/版权声明。

因此 n8n 代码**不能**用于 Hisha 的商业 SaaS 或对外分发产品。即使只复用单个节点实现，也受 SUL 约束。法律结论需法务复核。

## 13. 可直接复用

无。不能在商业产品中复制代码、节点实现或前端组件。可参考其公开 API 形状与文档描述。

## 14. 需要二次开发

若基于 n8n 二次开发，必须解决商业授权、品牌、Vue 技术栈、执行模型与多租户，成本高且法律风险大。不建议。

## 15. 仅值得借鉴

- 节点版本化（`VersionedNodeType`）与凭据测试方法。
- 表达式沙箱与“固定数据”调试体验。
- Queue Mode + 独立 task runner 的进程边界。
- 节点/凭据的 npm 分发与社区生态。
- Agent Skills 作为实例内 AI 助手的知识包（`instance-ai/skills/*`）。
- MCP Client/Trigger/OAuth 凭据的产品化方式。

## 16. 不适合采用

- SUL 与分支限制。
- Vue 3 + 自研画布作为 Hisha 前端（Hisha 选 React/XYFlow）。
- 非确定性、状态存 DB 的执行模型作为 Hisha 的 durable engine。
- 把 Agent、Workflow、节点执行混在同一 UI 与运行时。

## 17. 与 Hisha 的兼容性

概念高度重叠（Workflow Center、节点、表达式、执行历史、MCP、Skills），但代码与许可不兼容。Hisha 应独立实现 IR/运行时，只在交互模式上参考 n8n；绝不能为“省时间”复制节点代码。

## 18. 引入后的维护成本

作为依赖/代码库：不可接受（法律 + 升级 + Vue/TS 单体）。作为设计参考：低。作为集成目标（导入 n8n 工作流）：可做一个只读转换器，但不进入核心。

## 证据路径

`LICENSE.md`、`LICENSE_EE.md`、`package.json`、`packages/workflow/src/interfaces.ts`、`packages/workflow/src/versioned-node-type.ts`、`packages/workflow/src/expression.ts`、`packages/core/src/execution-engine/*`、`packages/cli/src/workflow-runner.ts`、`packages/cli/src/task-runners/*`、`packages/cli/src/public-api/v1/openapi.yml`、`packages/nodes-base/nodes/*`、`packages/@n8n/instance-ai/skills/*`、`packages/@n8n/agents/*`、`packages/@n8n/nodes-langchain/nodes/mcp/*`。
