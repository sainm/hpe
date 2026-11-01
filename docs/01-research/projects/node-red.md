# Node-RED 5.0.9 研究报告

> 仓库：`node-red/node-red` · tag `5.0.9`（2026-10-21）· tarball SHA-256 `b3a133c343f60b6c...`<br>
> 研究方式：Node.js monorepo 源码（1,550 文件）+ Apache-2.0 + runtime/registry/flows/storage/editor 代码；未构建运行。<br>
> 结论：**Design Reference（轻量本地运行时/节点生命周期）**；不嵌入为 Hisha 引擎。

## 1. 项目定位

面向事件驱动/物联网/家庭自动化的低代码 Node.js 工具：浏览器编辑流，运行时在 Node.js 中传递消息。强调本地部署、轻量、节点生态；不是企业工作流/durable execution 平台。

## 2. 核心功能

- Flow 编辑器：拖拽节点、连线、子流程、组、调试侧栏、上下文浏览器。
- 运行时：消息传递（`msg`）、节点生命周期、上下文（node/flow/global）、凭据加密、项目/库。
- 触发：Inject、HTTP、MQTT、WebSocket、TCP、定时等；核心节点 41 个（`@node-red/nodes/core`）。
- 子流程、Catch/Status/Link 节点、环境变量。
- 本地文件系统存储 flow/credentials；Projects 功能。
- 管理 API/编辑器 API/Comms WebSocket。

## 3. 技术架构

- Node.js monorepo：`packages/node_modules/@node-red/runtime`、`registry`、`editor-client`、`editor-api`、`nodes`。
- 运行时：`runtime/lib/flows/Flow.js`、`Subflow.js`、`Group.js`；`runtime/lib/nodes/index.js` 的 `registerType`；`runtime/lib/nodes/Node.js`。
- 注册表：`@node-red/registry` 加载 npm 节点包；节点由 JS 运行时 + HTML 编辑器定义组成。
- 存储：`runtime/lib/storage/localfilesystem/*`；默认 `~/.node-red`。
- 编辑器：vanilla JS + Ace 编辑器（`editor-client/src/js`、`ace`），不是 React/Vue。

## 4. 技术栈

Node.js、JavaScript、Express（editor-api）、WebSocket、Ace、npm 节点包、本地文件系统。无数据库（默认），无容器依赖。

## 5. 数据模型（从代码可见）

Flow JSON：`nodes`（id/type/name/props/wires）、`configs`、`subflows`、`groups`；凭据单独加密文件；上下文运行时内存/文件；settings.js。Flow 以 JSON 文档存储，节点类型注册表决定行为。

## 6. 执行机制

- 事件驱动：节点收到 `msg`，处理后调用 `send()` 沿 wires 传递；没有 DAG 调度器/超步/检查点。
- 错误通过 Catch 节点/`node.error`；状态通过 Status 节点；子流程是节点复用。
- 长任务/等待依赖节点自身（delay/trigger）；崩溃后不保证恢复。
- 适合轻量本地自动化；不适合需要版本、审批、补偿、多租户的业务流程。

## 7. 扩展机制

- npm 节点包：`registerType(nodeSet, type, constructor, opts)`；节点 constructor 处理 `input`/`close`，可选 `credentials`、`editor` HTML。
- 子流程、组、库（function/template）。
- 插件（`runtime/lib/plugins.js`）可扩展运行时/编辑器。

## 8. 插件开发方式

创建 npm 包，提供 `.js`（运行时）与 `.html`（编辑器）文件，在 `package.json` 的 `node-red.nodes` 中注册；安装到 `~/.node-red/node_modules`。没有签名/权限清单，供应链风险由用户承担。

## 9. API 设计

Admin HTTP API（`runtime/lib/api/*`：flows、nodes、settings、context、library、projects）；Editor API（`editor-api/lib/admin/*`）；Comms WebSocket 推送运行时事件。API 以 Flow/节点为中心。

## 10. 部署架构

npm 全局安装 / Docker / 本地 Node.js；默认单进程 + 本地文件；可通过 settings 配置存储/认证。适合边缘/本机，不是多租户集群。

## 11. 前端交互设计

经典三栏编辑器：节点面板、画布、侧栏（信息/调试/配置）；Ace 编辑函数；节点 HTML 定义配置 UI。交互轻量、可离线；但视觉与信息密度不符合 Hisha 的现代开发者工作台目标。

## 12. 许可证

根 `LICENSE` 为 Apache-2.0（OpenJS Foundation）。可商用/修改，需保留版权/NOTICE；节点包各自许可。无 copyleft。

## 13. 可直接复用

- 概念：`registerType` 节点生命周期、子流程、上下文作用域、本地文件存储、离线优先。
- 代码：不直接复用（vanilla JS 编辑器、无 IR/无版本/无 durable）。
- 可作为 Hisha Local Runtime 的“最小可用体验”参照：一个进程、本地目录、无云依赖。

## 14. 需要二次开发

若以 Node-RED 为基础：需要重写 IR/版本/权限/审批/多租户/执行恢复，并替换编辑器；等于重做平台，不建议。

## 15. 仅值得借鉴

- 节点注册与生命周期的最小契约。
- 上下文作用域（node/flow/global）与消息传递模型。
- 本地文件存储 + 项目/库的组织方式。
- 子流程的复用方式。
- 离线/单机体验的简洁性。

## 16. 不适合采用

- 消息传递模型作为 Hisha Workflow IR（缺少显式控制流/版本/数据契约）。
- 无检查点/无审批/无多租户的执行模型。
- vanilla JS + Ace 作为 Hisha 前端。
- 无签名/权限的 npm 节点供应链模型。

## 17. 与 Hisha 的兼容性

低（代码/模型），中（本地运行时理念）。Hisha 的 Local Runtime 应保留“单进程、本地目录、离线可用”的体验，但执行模型必须是 IR + 检查点 + 权限，而不是自由消息传递。

## 18. 引入后的维护成本

作为依赖：低（Node.js 轻量），但无法满足平台语义；作为参考：低。Hisha 不引入。

## 证据路径

`LICENSE`、`packages/node_modules/@node-red/runtime/lib/nodes/index.js`、`Node.js`、`flows/Flow.js`、`flows/Subflow.js`、`flows/Group.js`、`lib/storage/localfilesystem/*`、`lib/api/*`、`lib/plugins.js`、`packages/node_modules/@node-red/registry/*`、`packages/node_modules/@node-red/nodes/core/*`、`packages/node_modules/@node-red/editor-client/src/*`、`packages/node_modules/@node-red/editor-api/lib/admin/*`、`package.json`。
