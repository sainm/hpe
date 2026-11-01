# ADR-0019：VS Code + GitHub Copilot 作为增量集成入口

- 状态：已接受（增量模块）
- 日期：2026-11-01
- 决策者：架构/产品（集成模块）

## 背景

需要把 VS Code + GitHub Copilot 作为 Auralis 的官方本地 AI 工作入口，复用 Skills/MCP/Workflows/Agents/Local Runtime/Plugins/Connectors；不重新开发 AI Chat，不替换 Copilot，不修改既有架构/技术选型/模块职责。VS Code/Copilot 的 Skill 路径、MCP 配置、Instructions/Agents 机制随版本变化，不能假设跨客户端一致。

## 决策

- 新增独立 **TypeScript VS Code Extension**（`hpa.workbench`，展示名 Auralis），作为薄客户端：Tree/命令/状态栏/Webview/配置/认证/Skill 安装/MCP provider；不执行 Workflow/脚本。
- Copilot 继续负责 Chat/Agent；通过官方机制接入：
  - Agent Skills：项目 `.github/skills` 等原生路径 + 扩展 `chatSkills` 只读贡献。
  - MCP：`contributes.mcpServerDefinitionProviders` + `vscode.lm.registerMcpServerDefinitionProvider`；本地用 `McpHttpServerDefinition` 指向 Rust Local MCP Server；`provide` 不弹认证、`resolve` 取短期 token。
  - Custom Instructions/Agnets：可选生成到官方路径，用户确认、不覆盖。
- 复用既有 Auralis Local API/SSE、MCP Gateway/Local Server、Rust Runtime、权限/审批/审计；不新增执行引擎/MCP 核心/Chat。
- 复杂 Workflow 编辑跳转 Auralis Web UI；扩展不实现 Designer。
- 客户端提示不是审批；高风险工具在 Auralis 服务端强制审批。
- 版本不足时禁用对应功能/提示升级；旧版 MCP 兼容才写 `.vscode/mcp.json` 的 `servers`。
- 命名：新模块用 Auralis；既有 Hisha 文档/契约不改名。

## 备选

1. 在扩展内实现完整 Chat/Agent：重复 Copilot，成本高，不选。
2. 只写 `.vscode/mcp.json`/Skill 文件，不做 provider/UI：简单但认证/状态/更新弱，不选。
3. 扩展直接执行脚本/Workflow：重复 Runtime/安全边界，不选。
4. 新写 MCP 协议/Server：重复既有 RMCP，不选。
5. 在扩展内重建 Designer：违背“复杂编辑回 Web UI”，不选。

## 后果

- 需要跟踪 VS Code/Copilot 版本/路径/API；建立兼容矩阵。
- 认证/SecretStorage/Runtime 发现/远程开发/多根工作区需测试。
- Skill 安装需严格路径/权限/覆盖保护；不能污染用户文件。
- 扩展只增加集成层，不改变既有 Rust/TS/Python 架构与契约。
- 需 VSIX 测试/签名/发布流程；Marketplace 发布需审核。

## 证据

[模块 README](../14-vscode-copilot/README.md)、[官方证据](../14-vscode-copilot/evidence.md)、[Copilot 集成](../14-vscode-copilot/copilot-integration.md)、[MCP 集成](../14-vscode-copilot/mcp-integration.md)。
