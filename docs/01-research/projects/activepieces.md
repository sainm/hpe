# Activepieces 0.92.3 研究报告

> 仓库：`activepieces/activepieces` · tag `0.92.3-hotfix.2`（2026-10-28）· tarball SHA-256 `0af2c323921a1713...`<br>
> 研究方式：源码 tarball（26,337 文件）+ MIT/EE 许可证 + Piece/Sandbox/Worker/Waitpoint/MCP 代码；未构建运行。<br>
> 结论：**Design Reference；部分概念可 Adaptation**（Piece SDK、Waitpoint、MCP 工具面），不整体引入。

## 1. 项目定位

开源的 Zapier 替代品，核心是 700+ “Piece”（集成包）、可视化 Flow、连接/凭证、运行历史，以及 AI Agent、Tables、MCP。产品以 SaaS 自动化和集成生态为中心，不是通用 AI Agent 平台。

## 2. 核心功能

- Flow 可视化编排（触发 + Action + 分支 + 循环）；Flow 版本与发布；运行/步骤运行/日志。
- Pieces：`packages/pieces/community` + `core`，约 760 个 package.json；每个 Piece 定义 actions/triggers/auth/props。
- 连接（Connection）与凭证；项目/平台多租户；模板。
- Waitpoints：人工/外部信号暂停与恢复（`waitpoints/*`、`barrier-*`、`resume-service`）。
- AI Agents（`api/src/app/agents`）与 MCP Server：大量 `ap-*` 工具（构建/测试/发布 Flow、读步骤代码、查询运行）。
- Tables、文件、事件目的地、分析。

## 3. 技术架构

- TypeScript + Bun monorepo；API：Fastify 5 + TypeORM 0.3 + Postgres（或 PGlite）+ Redis + BullMQ + Socket.IO。
- Worker：`packages/server/worker`，按 job 执行；Sandbox：`packages/server/sandbox`，每个 worker 一个“box”，用 `isolated-vm` 6.2.0 + esbuild 打包 Piece 后在沙箱进程运行引擎。
- Engine：`packages/server/engine/src/main.ts`，启动时安装 SSRF guard，通过 worker socket 回报进度/错误。
- Piece Framework：`packages/pieces/framework/src/lib/piece.ts`、`piece-metadata.ts`。
- Web：React 19 + `@xyflow/react` 12 + Radix UI + Tailwind + i18next + TanStack Query。
- 部署：Docker Compose（app/worker/Postgres/Redis）、Pulumi、CLI。

## 4. 技术栈

TypeScript/Bun、Fastify、TypeORM、Postgres/PGlite、Redis、BullMQ、isolated-vm、esbuild、React 19、XYFlow 12、Radix、Tailwind、i18next、Socket.IO。核心 MIT；EE 目录商业许可。

## 5. 数据模型（从代码可见）

Flow、FlowVersion、FlowRun、StepRun、Connection、Piece metadata、Project/Platform、User、Waitpoint/WaitpointSignal、Agent、MCP tool config、Table/Record、Folder、Template。Flow 以 JSON steps + trigger 存储，版本化。

## 6. 执行机制

- API 创建 FlowRun，投递到 BullMQ；Worker 取得 job，调用 Sandbox 执行。
- Sandbox 预热/缓存 Piece bundle；每个 box 同时只处理一个操作；超时/内存/日志大小错误有专门错误码；执行结束释放或失效。
- Engine 在沙箱内运行 Flow 步骤，SSRF guard 限制出站请求；结果通过 socket 回传。
- Waitpoint：持久化暂停点，支持外部信号、超时、barrier；恢复服务继续运行。
- 没有确定性事件重放；恢复依赖持久化步骤状态。
- MCP Server 将 Flow/步骤/运行操作暴露为工具，供外部 AI 使用。

## 7. 扩展机制

- Piece：`createPiece` + actions/triggers + auth + props；每个 Piece 是独立 npm/workspace 包。
- Piece 版本、升级与 Flow 版本迁移（`flow-version/piece-upgrade-*`）。
- 自定义 Piece、社区 Piece、模板。
- MCP 工具是 API 层能力，不是 Piece 本身。

## 8. 插件开发方式

TypeScript Piece 包，使用 `@activepieces/pieces-framework` 定义 metadata/actions/triggers/auth；本地 workspace 或 npm 分发；平台安装后由 sandbox 打包运行。开发体验接近 Hisha 想要的 Connector/Node SDK，但其运行时与平台耦合。

## 9. API 设计

Fastify REST，按模块划分（flows、flow-runs、pieces、connections、agents、mcp、waitpoints、tables）；使用 Zod type provider；OpenAPI 由 Fastify Swagger 生成；Socket.IO 推送运行状态；Webhook 触发。MCP 工具有独立 controller/service。

## 10. 部署架构

Docker Compose 的 app + worker + Postgres + Redis；支持 PGlite 单机；Pulumi 部署；可水平扩展 worker。没有看到完整的 Helm/K8s 一等支持。

## 11. 前端交互设计

React 19 + XYFlow 的 Flow 编辑器；左侧步骤/触发器选择，中间画布，右侧步骤配置/测试；运行面板；Pieces 目录；连接管理；模板；AI/MCP 入口。信息密度和现代感较好，适合参考。

## 12. 许可证

根 `LICENSE`：

- `packages/ee/` 与 `packages/server/api/src/app/ee` 下的内容适用 `packages/ee/LICENSE` 的 Activepieces Enterprise License（生产使用需订阅/协议）。
- 其余内容为 MIT Expat。
- 第三方组件沿用各自许可证。

因此核心代码可商用/修改（保留版权），但 EE 代码不可用；Piece 包也可能有独立许可证，必须逐个核查。法律结论需法务复核。

## 13. 可直接复用

- Piece SDK 的分层（metadata/auth/actions/triggers）可作为设计参考，不建议直接依赖其 framework。
- Waitpoint 的“持久化暂停点 + 信号 + 超时 + barrier”模型适合 Hisha 的人工审批/外部事件。
- Sandbox 的“每 worker 一个 box + bundle 缓存 + 资源错误分类”值得借鉴。
- MCP 工具面的“Flow 构建/测试/发布/查询”是 Hisha MCP Gateway 的好参照。

## 14. 需要二次开发

若要复用 Piece 生态，需要把 760 个 Piece 的运行时从 Activepieces sandbox 解耦、建立许可证清单、适配 Hisha Connector/Node 双入口；维护成本很高，不建议在 Phase 1 做。

## 15. 仅值得借鉴

- Waitpoint/barrier 的持久化语义。
- isolated-vm + esbuild bundle 的轻量沙箱。
- SSRF guard 在引擎启动时统一安装。
- Piece 版本升级与 Flow 迁移。
- MCP 工具的产品化粒度（不是只暴露内部 REST）。

## 16. 不适合采用

- 整体作为 Hisha 内核（Fastify/TypeORM/BullMQ 与 Hisha NestJS/Postgres 设计重叠但不同）。
- 直接搬运 760 个 Piece（许可证、质量、维护、双入口权限都不可控）。
- 把 Waitpoint 当作完整的 durable execution（它解决暂停，不解决确定性重放）。
- EE 目录代码。

## 17. 与 Hisha 的兼容性

概念兼容度高：Piece ↔ Connector/Node，Waitpoint ↔ Approval/External Event，MCP 工具 ↔ Hisha MCP Gateway。代码兼容度低：运行时、SDK、数据模型、许可证边界均需重做。适合作为“连接器生态与暂停恢复”的参考实现。

## 18. 引入后的维护成本

作为依赖：高（Bun/isolated-vm/大量 Piece/EE 边界）。作为参考：低。作为 Piece 导入源：中高（逐包许可证与质量审核）。Hisha 应在 Phase 2 后自建 Connector SDK，仅按需导入经验证的连接器。

## 证据路径

`LICENSE`、`packages/ee/LICENSE`、`packages/server/api/package.json`、`packages/server/sandbox/src/lib/sandbox.ts`、`packages/server/sandbox/src/lib/sandbox-manager.ts`、`packages/server/worker/src/lib/execute/*`、`packages/server/engine/src/main.ts`、`packages/pieces/framework/src/lib/piece.ts`、`packages/server/api/src/app/mcp/tools/*`、`packages/server/api/src/app/waitpoints/*`、`packages/server/api/src/app/agents/*`、`packages/web/package.json`、`docker-compose.yml`。
