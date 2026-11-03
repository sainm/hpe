# Auralis for VS Code

Auralis 的 VS Code 薄客户端。它不执行 Workflow、Skill 脚本或插件代码；所有执行仍由本地 Rust Runtime 完成。

## 要求

- VS Code 1.143.0 或更高版本；已在 1.143.1 验证。
- 本地 Runtime：默认 `http://127.0.0.1:8787`，由 `hpa serve` 提供。
- 可选：GitHub Copilot Chat，用于通过官方 MCP provider 与项目 Skill 使用 Auralis。

## 安装

```bash
code --install-extension hpa-workbench.vsix
```

或使用 Extension 开发宿主：

```bash
code --extensionDevelopmentPath=apps/vscode-extension .
```

## 功能

- Auralis 活动栏：Overview、Workflows、Agents、Skills、MCP Servers、Plugins、Local Runtime、Executions、Artifacts、Settings。
- Workflow 运行、执行事件查看、取消、审批决定，以及执行与产物 Webview。
- 通过 Runtime 导出 API 安装/更新/移除 Skill 到 `.github/skills` 或个人 Copilot 目录；替换前备份，失败回滚，检测安装后的本地修改。
- 官方 `mcpServerDefinitionProviders` 注册本地 MCP Server；MCP PAT 只存 SecretStorage。
- 状态栏、离线提示、Runtime 版本信息与 Web UI 深链。
- 日文、英文、中文运行时文案；未信任工作区只读。

## 命令

`Auralis: Connect Local Runtime`、`Refresh`、`Runtime Status`、`Start Local Runtime`、`Run Workflow`、`Open Execution`、`Cancel Execution`、`Retry Execution`、`Decide Approval`、`Preview Artifact`、`Install Skill`、`Update Skill`、`Remove Skill`、`Register MCP Server`、`Open Web UI`、`Set API Token`、`Clear API Token`、`Open Settings`。

## 设置

- `hpa.mode`: `auto | local | remote`
- `hpa.local.endpoint`: 默认 `http://127.0.0.1:8787`
- `hpa.mcp.endpoint`: 留空时从本地端点推导为 `/mcp`
- `hpa.remote.endpoint` / `hpa.remote.mcpEndpoint`: 远程模式，必须 HTTPS
- `hpa.locale`: `auto | ja | en | zh`
- `hpa.skills.installTarget`: `project | personal`
- `hpa.skills.personalPath`: 个人 Skill 目录覆盖
- `hpa.runtime.binary` / `hpa.runtime.startArgs`: 显式启动命令；参数默认 `["serve"]`

## 安全边界

- 令牌只写 VS Code SecretStorage；不写 settings、workspace、日志或 Webview。
- 扩展不调用 shell 执行 Workflow/Skill 脚本；Skill 只通过 Runtime 导出的文件快照安装。
- Skill 目标名与路径做白名单/越界校验；符号链接在哈希与安装检查中被拒绝。
- Webview 使用 CSP nonce、HTML 转义和严格消息白名单。
- 未信任工作区禁止运行、审批、取消与 Skill 写入；只允许查看。
- Runtime 只允许回环 HTTP；远程端点必须 HTTPS。

## 已知限制

- MCP Server 当前只服务 `local` 租户；`/mcp` 不经过 Enterprise TenantGate。
- 无远程 Worker 派发、浏览器 SSO、Skill Marketplace 发布或自动升级。
- Copilot Chat 的登录会话未自动准备；本版本验证了 MCP provider 启动/工具发现与 `.github/skills` 安装，未验证模型驱动的 `tools/call`。
- 扩展通过 Runtime 的 export API 写 Skill；扩展宿主与 Runtime 必须能访问同一数据目录/文件系统。

完整实现说明见 `docs/14-vscode-copilot/local-implementation.md`。
