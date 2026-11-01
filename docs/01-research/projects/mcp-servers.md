# MCP Servers 2.2.0 研究报告

> 仓库：`modelcontextprotocol/servers` · tag `v2.2.0`（2026-10-28）· tarball SHA-256 `51239f3abffced0b...`<br>
> 研究方式：TS/Python 参考服务器源码（358 文件）+ LICENSE 过渡声明 + transports/SDK 用法；未构建运行。<br>
> 结论：**Direct Reuse as Test Fixtures；Do Not Use as Production**。

## 1. 项目定位

MCP 官方参考服务器集合：Everything、Fetch、Filesystem、Git、Memory、Sequential Thinking、Time。用于演示协议能力与 SDK 用法，官方明确说明“不是生产就绪方案”。另维护 MCP Registry 的入口说明，但 Registry 本身不在本仓库。

## 2. 核心功能

- 7 个参考服务器，覆盖 tools/resources/prompts、stdio/HTTP、认证示例。
- Everything 是协议能力测试服务器（含 prompts/resources/tools、legacy SSE、Streamable HTTP）。
- Filesystem 演示路径访问控制；Git 演示仓库操作；Memory 演示知识图谱持久化；Fetch 演示网页抓取。
- 每个服务器是独立 npm/Python 包。

## 3. 技术架构

- TypeScript 服务器：`src/everything`、`filesystem`、`memory`、`sequentialthinking`；Python：`src/fetch`、`git`、`time`。
- 使用官方 MCP SDK：`@modelcontextprotocol/server`、`@modelcontextprotocol/node`、`@modelcontextprotocol/server-legacy/sse`；Python `mcp`。
- 传输：stdio、legacy SSE（兼容）、Streamable HTTP（`everything/transports/streamableHttp.ts`）。
- 根 `package.json` workspaces `src/*`，根版本 0.6.2 与 release v2.2.0 不一致，说明 monorepo 版本与实际发布需分别核对。

## 4. 技术栈

TypeScript/Node.js、Python、官方 MCP SDK、npm/py 包；无数据库（Memory 用文件）、无前端。部署为本地进程或 HTTP 服务。

## 5. 数据模型

协议层：ServerInfo/Capabilities、Tool（name/description/inputSchema）、Resource（uri/mimeType）、Prompt（name/arguments）、Content blocks、JSON-RPC request/response。服务器业务数据各自定义（文件、git repo、memory graph）。

## 6. 执行机制

- 每个服务器实现 MCP handler；客户端调用 `tools/call`、`resources/read`、`prompts/get`。
- stdio：客户端启动进程，JSON-RPC 走 stdin/stdout；HTTP：单端点，SSE 流式响应。
- 无编排/无持久执行；工具 handler 直接执行本地操作。
- Everything 服务器用于验证协议边界与传输兼容。

## 7. 扩展机制

- 用 MCP SDK 编写新 server；注册 tools/resources/prompts；选择 transport。
- 没有平台级插件系统；MCP Registry 负责发现/分发（外部）。

## 8. 插件开发方式

MCP Server 即插件：实现协议 handler，打包为可执行文件/npm/py 包；通过客户端配置（stdio command 或 HTTP URL）接入。Hisha 的 MCP Client 应能导入这类服务器作为外部工具源。

## 9. API 设计

MCP JSON-RPC 协议；传输见上。没有 REST/OpenAPI。参考服务器的工具 schema 是学习 MCP tool 设计的好材料。

## 10. 部署架构

本地 stdio（客户端启动）、HTTP 服务、Docker（部分）；无集群/多租户。生产部署需自行加认证、限流、沙箱、审计。

## 11. 前端交互设计

无 UI；由客户端（Claude、VS Code/Copilot、Codex、Hisha）呈现工具调用与确认。

## 12. 许可证

根 `LICENSE` 明确处于 **MIT → Apache-2.0 过渡**：

- 新代码/规范贡献 Apache-2.0；文档（不含规范）CC-BY-4.0。
- 已获重许可同意的贡献 Apache-2.0；未同意的 MIT 贡献仍为 MIT。
- 不授予超出原许可的权利。

因此不能简单说“整个仓库都是 Apache-2.0”或“都是 MIT”；复用前必须按文件/包核查。参考服务器各自有 LICENSE 文件。

## 13. 可直接复用

- 作为 Hisha MCP Client/Server 的**测试夹具**与协议兼容性样本：Everything 覆盖 prompts/resources/tools 与两种 HTTP 传输；Filesystem/Git 覆盖本地权限边界。
- 不作为生产组件；不复制其安全模型。

## 14. 需要二次开发

若要把参考服务器用于生产：需要认证、租户隔离、限流、审计、沙箱、路径/网络策略、错误处理、版本化；这等于重写。Hisha 应把它们作为外部 MCP Server 接入，而不是内置。

## 15. 仅值得借鉴

- 工具 schema/描述的写法；资源 URI 设计；prompt 参数。
- Everything 的传输兼容测试。
- Filesystem 的允许路径模型（Hisha 需要更强的租户/工作区隔离）。
- 本地 stdio 与远程 HTTP 并存的客户端配置。

## 16. 不适合采用

- 作为生产服务器（官方明确非生产就绪）。
- 直接复制到 Hisha 内置工具（安全/许可/维护不可控）。
- 依赖 legacy SSE 作为新设计（2026-07-28 已移除 GET 流/会话）。

## 17. 与 Hisha 的兼容性

高（测试/协议），低（生产）。Hisha 的 MCP Gateway 应支持 stdio/Streamable HTTP，能接入这些服务器用于 conformance；本地 MCP Server 可参考其结构但必须加认证/权限/审计。

## 18. 引入后的维护成本

作为测试夹具：低（固定版本）。作为生产依赖：高（安全/协议演进/许可）。Hisha 只取前者。

## 证据路径

`LICENSE`、`README.md`、`package.json`、`src/everything/transports/{stdio.ts,sse.ts,streamableHttp.ts}`、`src/everything/__tests__/*`、`src/filesystem/*`、`src/git/*`、`src/fetch/*`、`src/memory/*`、`src/sequentialthinking/*`、`src/time/*`、各 `src/*/LICENSE`。
