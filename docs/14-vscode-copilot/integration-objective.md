# 集成目标与边界

## 1. 目标

1. 将 VS Code + GitHub Copilot 作为 Auralis 的官方本地 AI 工作入口。
2. 用户无需离开编辑器即可发现/安装 Skills、发现/调用 MCP Tools、运行 Workflow/Agent、查看执行与产物、管理 Local Runtime。
3. 复用 Auralis 的能力注册、权限、审批、审计与执行；不复制这些逻辑。
4. 保持 Copilot 作为 Chat/Agent；Auralis 提供能力与治理。
5. 复杂流程编辑仍回到 Auralis Web UI，避免在扩展里重建 Designer。

## 2. 非目标

- 不开发新的 AI Chat/Agent 产品。
- 不替代 GitHub Copilot 的对话、模型选择、编辑器集成。
- 不在扩展内执行 Workflow/脚本/插件。
- 不重写 MCP 协议、RMCP、Local Runtime、IR、权限模型。
- 不实现完整 Workflow Designer/Agent Designer/Skill Editor。
- 不把 Auralis Skill 转成 VS Code 私有格式。
- 不假设 Copilot CLI/VS Code/Codex 的 Skill/MCP 配置相同。
- 不自动修改用户仓库/覆盖现有 Instructions/Agents/Skills。

## 3. 用户与价值

| 用户 | 价值 |
| --- | --- |
| 使用 Copilot 的开发者 | 在 Chat 中通过 Skill 指导调用 Auralis Workflow/MCP，不切换工具 |
| 平台工程师 | 在编辑器内管理 Local Runtime/MCP/Skill，快速调试 |
| 团队负责人 | 统一 Skill/MCP/审批/审计入口，不牺牲治理 |
| 外部 AI 用户 | 用官方 MCP/Agent Skills 机制复用企业能力 |

## 4. 集成边界

| 层 | 负责 | 不负责 |
| --- | --- | --- |
| GitHub Copilot | Chat、Agent、模型、工具选择、客户端审批提示 | 不负责 Auralis 权限/审批/执行 |
| VS Code | Extension Host、Tree View、Webview、SecretStorage、MCP provider API、Skill 加载 | 不执行 Auralis 工作流 |
| Auralis Extension | UI/命令/配置/认证桥接、Skill 安装、MCP provider、执行监控 | 不执行脚本/工作流；不实现 Chat |
| Auralis MCP Server | 工具 schema、认证/租户、权限/审批、调用执行 | 不暴露内部 REST 全集 |
| Auralis Local Runtime | 执行、沙箱、进程/资源、日志、产物、审计 | 不处理 Copilot UI |
| Auralis Control Plane | 账号/租户/团队/远程审批/审计 | 不依赖 VS Code 运行 |

## 5. 成功标准

- 从打开项目到 Copilot 调用 Auralis MCP Tool < 5 分钟（已安装 Runtime）。
- Skill 安装/更新/移除可逆，路径符合 VS Code/Copilot 原生规范。
- 扩展不在本地执行脚本；所有执行可在 Auralis Execution Monitor 追溯。
- 高风险工具调用在 Auralis 服务端审批，客户端无法绕过。
- 断网/本地 Runtime 停止/未登录时有明确状态与恢复路径。
- 日/英/中 UI；VS Code 高对比/键盘可用。

## 6. 版本/兼容前提

- VS Code 版本需支持 `chatSkills`、MCP provider API、Custom Agents（以官方文档/功能矩阵为准）。
- Copilot 版本/IDE 支持范围不同；扩展只使用官方机制，并在不满足时降级/提示。
- Auralis Local Runtime 需支持 Local API/SSE、MCP HTTP、Skill/Execution/Artifact 端点。
- 旧版 VS Code 不支持 MCP provider 时，才考虑写 `.vscode/mcp.json`（`servers`），且需用户确认。

## 7. 相关

- [Extension 架构](extension-architecture.md)
- [Copilot 集成](copilot-integration.md)
- [官方证据](evidence.md)
