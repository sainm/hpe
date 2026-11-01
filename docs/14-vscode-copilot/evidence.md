# 官方证据与限制

> 核查日期：2026-11-01。来源：VS Code 官方文档、VS Code Extension API 文档、GitHub Copilot 官方文档、`@types/vscode` 1.140.0 类型定义与 1.143.1 本机 API。扩展已构建、打包 VSIX 并在 1.143.1 运行；未发布 Marketplace，未做模型驱动的 Copilot MCP `tools/call` E2E。

## 1. Agent Skills

**VS Code 官方**：<https://code.visualstudio.com/docs/agent-customization/agent-skills>

提取到的事实：

- 项目技能：`.github/skills/`、`.claude/skills/`、`.agents/skills/`。
- 个人技能：`~/.copilot/skills/`、`~/.claude/skills/`、`~/.agents/skills/`。
- 自定义搜索：`chat.agentSkillsLocations`（支持但不推荐用于 Copilot；Copilot CLI/app 不继承）。
- 扩展贡献：`contributes.chatSkills`，`path` 指向 `SKILL.md`；**目录名必须与 frontmatter `name` 一致**，否则不加载。
- Agent Skills 标准为开放标准；VS Code Copilot/CLI/app/实验 Codex 支持范围不同。
- Monorepo：`chat.useCustomizationsInParentRepositories` 可发现父仓库。

**GitHub Copilot 官方**：<https://docs.github.com/en/copilot/concepts/customizations/agent-skills>

- 项目 `.github/skills`、`.claude/skills`、`.agents/skills`；个人 `~/.copilot/skills`/`~/.agents/skills`。
- IDE 支持取决于版本；必须查 Copilot feature matrix/各 surface setup guide。
- 不得假设 personal Skill 在所有 surface/云会话可用。

## 2. MCP

**VS Code MCP 用户文档**：<https://code.visualstudio.com/docs/agent-customization/mcp-servers>

- 工作区 `.vscode/mcp.json` 顶层 `servers`；`mcpServers` 是可移植文件的键，不可混用。
- 支持 Extension 注册、CLI `--add-mcp`、全局配置、自动发现。

**VS Code MCP 开发者文档**：<https://code.visualstudio.com/api/extension-guides/ai/mcp>

- `contributes.mcpServerDefinitionProviders`（id/label）。
- `vscode.lm.registerMcpServerDefinitionProvider(id, provider)`。
- `McpServerDefinitionProvider`：
  - `provideMcpServerDefinitions`（eager；不应要求用户交互/认证）。
  - `resolveMcpServerDefinition`（可认证/修改非只读属性）。
  - `onDidChangeMcpServerDefinitions`。
- 类型：`McpStdioServerDefinition`（子进程 stdin/stdout）、`McpHttpServerDefinition`（Streamable HTTP；headers）。
- `@types/vscode 1.140.0` 的 `index.d.ts` 已确认上述类型/API（MIT）；本机 VS Code 1.143.1 实际注册 provider 并启动服务器。

## 3. Custom Instructions

**VS Code 官方**：<https://code.visualstudio.com/docs/agent-customization/custom-instructions>

- Copilot 项目指令：`.github/copilot-instructions.md` 或 `AGENTS.md`。
- 定向指令：`.github/instructions/**/*.instructions.md`。
- 设置：`chat.instructionsFilesLocations`（支持但不推荐跨客户端；Copilot CLI/app 不继承）。
- 不假设自定义搜索位置能被所有 Copilot 客户端继承。

## 4. Custom Agents

**VS Code 官方**：<https://code.visualstudio.com/docs/agent-customization/custom-agents>

- 项目：`.github/agents/<name>.agent.md`。
- 个人：`~/.copilot/agents`。
- 设置：`chat.agentFilesLocations`；`chat.modeFilesLocations` 已弃用。
- Frontmatter：`name`、`description`、`tools` 等；具体字段以官方 reference 为准。
- 生成 Agent 的工具命名/可用性需按当前 VS Code/Copilot 验证；不假设 MCP 工具名可直接写入。

## 5. Extension 工具链

| 包 | 版本 | License | 证据 |
| --- | --- | --- | --- |
| `@types/vscode` | 1.140.0 | MIT | npm tarball `index.d.ts` |
| `@vscode/vsce` | 4.1.0 | MIT | npm registry |
| `@vscode/test-cli` | 0.0.15 | MIT | npm registry |
| `@vscode/test-electron` | 3.1.0 | MIT | npm registry |
| esbuild | 0.28.2 | MIT | npm registry |
| `@vscode-elements/elements` | 2.5.1 | MIT | npm registry |
| `@vscode/webview-ui-toolkit` | 1.4.0 | MIT | 旧方案；不采用 |

## 6. 未验证/限制

- 已构建/打包 `.vsix`，在 VS Code 1.143.1 完成扩展激活、MCP provider 启动/工具发现与 Skill 安装；Copilot 模型驱动的 `tools/call` 未验证。
- Copilot IDE 版本/功能矩阵未逐版本穷举；实现固定在 1.143.1 验收，未做 1.140–1.142 运行测试。
- `chatSkills` 未纳入本实现；Skill 通过 `.github/skills` 安装路径提供。Custom Agents 未实现。
- Custom Agent 的 `tools` 字段如何引用 MCP 工具未在当前文档中确认；生成器默认省略/需验证。
- `McpHttpServerDefinition` 是否支持所有 OAuth 流由客户端实现决定；`resolve` 只负责提供 header。
- VS Code/Copilot 实际使用的 MCP 协议版本由客户端决定；Auralis RMCP Server 需同时支持 2026-07-28/旧版，并以真实握手验证。
- 多根/Remote SSH/Container 的 Runtime 位置策略需实际测试。
- 未做 Marketplace 发布/签名；VSIX 供应链流程待实现。
- 命名：本模块使用 Auralis；既有 Phase 0 文档用 Hisha 代号，内部契约不变。

## 7. 复现/核对

```bash
# VS Code docs
curl -sSL https://code.visualstudio.com/docs/agent-customization/agent-skills | grep -o '\.github/skills' | head

# MCP API types
npm pack @types/vscode@1.140.0
tar -xzf types-vscode-1.140.0.tgz
grep -n "registerMcpServerDefinitionProvider\|McpHttpServerDefinition" index.d.ts

# Extension tool versions
npm view @vscode/vsce version license
```
