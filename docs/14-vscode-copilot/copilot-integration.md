# GitHub Copilot 集成

> Copilot 继续负责 Chat/Agent；Auralis 只通过官方机制提供 Skills、MCP Tools、Instructions、Custom Agents。不得假设不同客户端（VS Code、Copilot CLI、Copilot app、Codex）路径/字段相同。

## 1. 集成机制总览

| 机制 | 用途 | Auralis 提供方式 | 官方路径/设置 |
| --- | --- | --- | --- |
| Agent Skills | 教 Copilot 何时/如何调用 Auralis | 安装标准 Skill；扩展 `chatSkills` 贡献只读技能 | 项目 `.github/skills/`、`.claude/skills/`、`.agents/skills/`；个人 `~/.copilot/skills/`、`~/.claude/skills/`、`~/.agents/skills/` |
| MCP Servers | 让 Copilot 调用 Auralis 工具 | 扩展注册 Local/Remote MCP provider；复用 Rust RMCP Server | `contributes.mcpServerDefinitionProviders` + `vscode.lm.registerMcpServerDefinitionProvider`；`.vscode/mcp.json` 顶层 `servers` |
| Custom Instructions | 项目级行为约束/工作方式 | 生成 Auralis 指令片段（用户确认） | `.github/copilot-instructions.md` 或 `AGENTS.md`；`.github/instructions/**/*.instructions.md` |
| Custom Agents | 预置角色/工具集 | 可选：从 Auralis Agent 生成 `.agent.md` | `.github/agents/<name>.agent.md`；个人 `~/.copilot/agents` |

## 2. Agent Skills

### 2.1 目录（VS Code/Copilot 官方）

- 项目：
  - `.github/skills/<name>/SKILL.md`
  - `.claude/skills/<name>/SKILL.md`
  - `.agents/skills/<name>/SKILL.md`
- 个人：
  - `~/.copilot/skills/<name>/SKILL.md`
  - `~/.claude/skills/<name>/SKILL.md`
  - `~/.agents/skills/<name>/SKILL.md`
- 自定义搜索位置：`chat.agentSkillsLocations`（支持但**不推荐**用于 Copilot；Copilot CLI/app 不继承 VS Code 设置）。
- 扩展贡献：`contributes.chatSkills: [{ "path": "./skills/<name>/SKILL.md" }]`；目录名必须与 frontmatter `name` 完全一致，否则不加载。

### 2.2 Auralis 选择

- **用户安装**：默认写项目 `.github/skills/<name>/`；可选个人 `~/.copilot/skills/<name>/`。
- **扩展内置**：Auralis 官方只读技能通过 `chatSkills` 随扩展分发；不写入用户仓库。
- 不同时写 `.claude/skills`/`.agents/skills` 以免重复；需要跨客户端时由用户显式选择目标。
- Skill 内容保持标准目录；Auralis 版本/来源元数据放在扩展 `globalState`/workspace `.hpa/skills.json`，不污染 SKILL.md。
- 不把 Auralis 私有字段写进 SKILL.md；Copilot 忽略未知字段但不保证。

### 2.3 Skill 内容要求

- `description` 明确“何时使用 Auralis”与“何时调用 MCP 工具”。
- 指令中给出工具发现顺序、审批/拒绝处理、错误恢复、结果展示。
- 不包含 secret/token/内部 URL；不要求 Copilot 绕过权限。
- 脚本只作为参考/确定性入口；执行由 Auralis Runtime 负责，不由 Copilot 直接执行未知脚本。

## 3. MCP Servers

### 3.1 推荐：Extension MCP Provider

```json
{
  "contributes": {
    "mcpServerDefinitionProviders": [
      { "id": "hpa-local", "label": "Auralis Local MCP" }
    ]
  }
}
```

```ts
vscode.lm.registerMcpServerDefinitionProvider("hpa-local", {
  provideMcpServerDefinitions: async () => [
    new vscode.McpHttpServerDefinition(
      "Auralis Local",
      vscode.Uri.parse("http://127.0.0.1:8787/mcp"),
      undefined,
      runtimeVersion
    )
  ],
  resolveMcpServerDefinition: async (server) => {
    const token = await auth.getLocalToken();
    server.headers = { Authorization: `Bearer ${token}` };
    return server;
  },
  onDidChangeMcpServerDefinitions: onRuntimeChanged
});
```

规则：

- `provide` 必须快速且**不弹认证**；认证在 `resolve`。
- 使用 `McpHttpServerDefinition`（Streamable HTTP）；本地 loopback + 短期 token。
- 不提供 `McpStdioServerDefinition` 启动任意脚本；除非有官方 Auralis Runtime bridge 且用户显式授权。
- Runtime 启动/停止/config 改变 → 触发 `onDidChangeMcpServerDefinitions`。
- 远程模式使用 HTTPS MCP Gateway；`resolve` 获取 PAT/OIDC token。
- 一个 provider id 管理本地/远程切换；不注册重复服务器。

### 3.2 兼容：`.vscode/mcp.json`

仅当 VS Code 不支持 MCP provider（或用户明确要求）时使用：

```json
{
  "servers": {
    "hpa-local": {
      "type": "http",
      "url": "http://127.0.0.1:8787/mcp",
      "headers": { "Authorization": "Bearer ${input:hpa-token}" }
    }
  }
}
```

- VS Code 使用 `servers`；**不是** `mcpServers`。
- 不写入 token；使用 VS Code `inputs`/环境变量/SecretStorage。
- 写文件前确认工作区信任、用户同意、备份现有配置。
- Copilot CLI 使用 `mcp-config.json` 的 `mcpServers`；不要复制 VS Code 配置。

### 3.3 Copilot 侧行为

- Copilot 负责工具选择/调用；Auralis 服务端负责权限/审批/执行。
- Copilot 客户端提示/确认不能替代 Auralis 服务端审批。
- 工具结果按 MCP 规范返回；大结果用 Resource/Artifact 引用。
- 工具名以 Auralis MCP Server 暴露为准；客户端若限制名称字符，提供别名映射（见 MCP 集成）。
- **MCP 协议版本由 VS Code/Copilot 客户端决定**：客户端可能使用 2026-07-28 或旧版；Auralis Rust RMCP Server 必须同时支持协商/旧版；以实际握手日志验证，不假设客户端支持最新版。

## 4. Custom Instructions

### 4.1 官方路径

- Copilot 项目指令：`.github/copilot-instructions.md` 或 `AGENTS.md`。
- 定向指令：`.github/instructions/**/*.instructions.md`（frontmatter `applyTo`）。
- 设置：`chat.instructionsFilesLocations`（支持但不推荐跨客户端；Copilot CLI/app 不继承）。
- 父仓库发现：`chat.useCustomizationsInParentRepositories`（monorepo）。

### 4.2 Auralis 生成策略

命令 `hpa.copilot.installInstructions`：

1. 检测现有文件/内容；不覆盖。
2. 用户选择目标：`.github/copilot-instructions.md`（追加标记块）或 `.github/instructions/hpa.instructions.md`（新建）。
3. 生成内容：
   - Auralis 平台用途/可用工具。
   - 何时先搜索 Workflow/Skill、何时执行、何时请求审批。
   - 高风险操作必须等待审批；不绕过权限。
   - 结果/产物查看方式；错误/离线处理。
   - 不包含 token/secret/内部 URL。
4. 预览 diff；用户确认后写入；可一键移除标记块。
5. 不自动把 Custom Instructions 当作安全边界；服务端仍强制。

## 5. Custom Agents

### 5.1 官方路径

- 项目：`.github/agents/<name>.agent.md`。
- 个人：`~/.copilot/agents/<name>.agent.md`。
- 设置：`chat.agentFilesLocations`；`chat.modeFilesLocations` 已弃用。
- Frontmatter：`name`、`description`、`tools` 等（以当前官方 reference 为准）。

### 5.2 Auralis 生成策略

- 可选命令 `hpa.copilot.createAgent`：从 Auralis Agent 配置生成 `.github/agents/hpa-<name>.agent.md`。
- 只生成只读/低风险工具集；高风险管理由 Auralis Approval 处理。
- `tools` 字段中的 MCP 工具命名必须在当前 VS Code/Copilot 版本验证；无法确认时省略 `tools`，由 Copilot 默认工具选择。
- 不覆盖现有 agent 文件；用户确认；可移除。
- 生成 Agent 不是平台 Agent 的替代；权限/预算/审批仍由 Auralis 服务端执行。

## 6. 兼容性

| 客户端/版本 | Skills 路径 | MCP 配置/机制 | Instructions | Custom Agents | 备注 |
| --- | --- | --- | --- | --- | --- |
| VS Code + Copilot（当前） | `.github/skills` 等；扩展 `chatSkills` | MCP provider API；`.vscode/mcp.json` `servers` | `.github/copilot-instructions.md`/`AGENTS.md`；`.github/instructions/**` | `.github/agents/*.agent.md` | 以实际版本/功能矩阵为准 |
| Copilot CLI | `.github/skills` 等（按官方） | `mcp-config.json` `mcpServers` | 按 CLI 文档 | 按 CLI 文档 | 不继承 VS Code settings |
| Copilot app | 按官方 | 按官方 | 按官方 | 按官方 | 不假设与 VS Code 相同 |
| OpenAI Codex（实验） | `.github/skills`/`.agents/skills` | `config.toml [mcp_servers]` | `AGENTS.md` | 按 Codex 文档 | 非本模块主目标 |

- 扩展只承诺 VS Code + Copilot；其他客户端通过导出/文档支持，不共享配置文件。
- 版本不足时：提示升级；不要静默降级到不安全/不兼容方式。
- 不假设 `chat.agentSkillsLocations`/`chat.instructionsFilesLocations` 能跨 Copilot 客户端。

## 7. 不做的集成

- 不实现 Copilot Chat/Agent。
- 不写 Copilot 私有 API/未文档化配置。
- 不把 Auralis Skill 转成 VS Code 专有格式。
- 不通过 Custom Instructions/Agent 绕过审批。
- 不自动覆盖用户 Instructions/Agents/Skills。
- 不在扩展中执行 Skill 脚本/Workflow。

## 8. 相关

- [Skill 集成](skill-integration.md)
- [MCP 集成](mcp-integration.md)
- [官方证据](evidence.md)
- [Security](security.md)
