# Extension UI

> 目标：VS Code 原生工作台体验（Tree View / Command Palette / Status Bar / Webview），不是 Auralis Web UI 的复制。复杂编辑跳转 Web UI。

## 1. 侧边栏

Activity Bar 容器 `hpa`，10 个 View：

| View | 内容 | 主要操作 |
| --- | --- | --- |
| Overview | Runtime 状态、登录/工作区、最近执行、待审批、快捷动作 | Start/Run/Sign in/Open Web |
| Workflows | 按项目/状态分组的 Workflow/版本 | Open in Web、Run、Versions、Copy ID |
| Agents | Agent/版本/最近会话 | Run、Status、Open in Web |
| Skills | 已安装/项目/个人/可搜索 | Search、Install、Update、Remove、Validate |
| MCP Servers | 本地/远程 Server、状态、工具数 | Register、Test、Refresh、Open Web |
| Plugins | 已安装/市场入口 | Open Marketplace/Web |
| Local Runtime | 状态/版本/端口/健康 | Start/Stop、Doctor、Logs、Settings |
| Executions | 最近/筛选/状态 | Open Detail、Cancel、Retry、Open Web |
| Artifacts | 按执行/类型分组的产物 | Preview、Open、Download |
| Settings | 端点/认证/Skill 目标/Copilot 开关 | Quick Pick/Webview |

- 使用 `TreeDataProvider`；节点 `contextValue` 驱动 `view/item/context` 菜单。
- 空状态：说明 + 主操作 + 文档。
- 加载：VS Code progress/placeholder；不阻塞其他 View。
- 权限/未信任：只读节点；执行/安装命令隐藏/禁用。
- 错误：节点描述/工具提示；点击打开 Output/Web UI。

## 2. Overview

Webview（或 Tree 摘要）：

- Runtime：状态（Running/Degraded/Offline）、版本、端点、平台。
- 身份：用户/tenant/workspace/project（已登录时）。
- 最近执行：状态/耗时/错误；点击详情。
- 待审批：数量/最近；打开 Auralis Approval Center。
- 快捷：Run Workflow、Install Skill、Register MCP、Open Web。
- 引导：未启动 Runtime/未登录/未安装 Copilot 兼容版本。

## 3. Workflows

- 树：Project → Status → Workflow → Version；显示版本/更新时间/风险。
- 执行：选择版本/输入（Quick Pick/Webview 表单）→ 启动；等待/审批状态。
- 复杂编辑：`Open in Auralis Web` 深链（版本/Designer）。
- 只读：扩展不编辑 IR/布局。
- 搜索/刷新；复制 ID/深链。

## 4. Agents

- 列表：Agent/版本/状态/最近 Session。
- 操作：Run（输入/预算确认）、Status、Open in Web、Copy ID。
- 无 Python AI Worker 时禁用并提示。
- 多 Agent/预算/审批由服务端；扩展只展示。

## 5. Skills

- 分组：Installed（项目/个人）、Available（搜索/市场）。
- 安装：选择目标（`.github/skills` 默认 / 个人 `~/.copilot/skills` / 可选 `.claude`/`.agents`）。
- 更新/移除/回滚；本地修改保护。
- 校验：调用 Auralis Skill Service；显示错误/修复。
- 打开 SKILL.md/目录；复制安装路径。
- 不执行脚本；脚本由 Runtime。

## 6. MCP Servers

- 本地/远程 Server；状态/工具数/最近检查。
- 注册/刷新/测试连接；打开 MCP Center Web UI。
- 工具列表：名称/风险/审批；只读查看 schema。
- 认证：本地 token/远程 PAT/OIDC；SecretStorage。
- MCP provider 自动注册本地 Server；不在 View 中重复添加。
- 失败：显示错误/重试/诊断。

## 7. Plugins

- 已安装/可用市场；打开 Web UI Marketplace 安装/升级。
- 扩展不直接安装插件（复杂 UI/权限在 Web）；只展示/跳转。
- 版本/许可/信任只读。
- Connectors/Credentials：通过 Auralis Web `Connections` 管理；扩展不存凭证/不复制连接器 UI。

## 8. Local Runtime

- 状态：Running/Degraded/Offline；版本/协议/端口/数据目录。
- 操作：Start（显式）、Stop（仅扩展启动）、Doctor、Logs、Settings。
- 健康检查结果；版本升级提示（不自动）。
- 未安装：打开安装文档/CLI 提示。

## 9. Executions

- 最近/按 Workflow/状态；实时状态。
- 详情 Webview：状态/时间线/节点/日志/错误/审批/产物。
- 操作：Cancel、Retry（策略）、Open in Web、Copy ID。
- 审批等待：打开 Approval Center；拒绝/超时显示原因。
- 大日志：过滤/分页/下载；不阻塞。

## 10. Artifacts

- 按执行/类型分组；名称/类型/大小/时间。
- 预览：文本/图片/PDF（VS Code 支持）；大文件下载/外部打开。
- 安全：只允许 Auralis 授权产物；不暴露 Runtime 文件系统路径。
- 打开/复制链接/下载；审计由 Runtime。

## 11. Settings

Quick Pick/Webview：

- Runtime：mode/local endpoint/mcp endpoint/remote endpoint。
- Auth：Sign in/Sign out；token 状态（不显示值）。
- Skills：安装目标/自动更新（默认关）。
- Copilot：Custom Instructions/Custom Agents 生成开关。
- Telemetry：默认关。
- Diagnostics：Doctor/Open Logs/Copy support info。

## 12. Status Bar

- Runtime 状态（$(pulse)/$(warning)/$(circle-slash)） + 文本。
- 执行中/待审批数量；点击打开 Overview/Approvals。
- 未登录/未信任提示。
- 不显示 token/敏感信息。

## 13. Command Palette

命令统一 `hpa.*`：

- `hpa.runtime.start/stop/status/doctor/logs`
- `hpa.workflow.open/run/refresh`
- `hpa.agent.run/status`
- `hpa.skill.install/update/remove/validate/refresh`
- `hpa.mcp.register/refresh/test`
- `hpa.execution.open/cancel/retry`
- `hpa.artifact.open`
- `hpa.copilot.installInstructions/createAgent`
- `hpa.settings.open`
- `hpa.web.open`

`when` 条件：`hpa.runtime.running`、`workspaceTrusted`、`hpa.authenticated`、资源类型。

## 14. Webview 规范

- HTML/CSS + `@vscode-elements/elements`（或原生控件）；使用 VS Code CSS variables（`--vscode-*`），明暗/高对比自动。
- CSP：`default-src 'none'; img-src ${webview.cspSource} https: data:; style-src ${webview.cspSource} 'unsafe-inline'; script-src 'nonce-...'`。
- 不加载远程脚本/CDN；本地 media 通过 `asWebviewUri`。
- 消息：`postMessage` + Zod 校验；命令白名单。
- 状态保持谨慎；执行数据可重新获取。
- 键盘/焦点/ARIA；不使用 hover-only。
- 大日志/产物分页/虚拟滚动。

## 15. 状态/空/错误

| 状态 | 表现 |
| --- | --- |
| 未安装 Runtime | Offline + Start/文档 |
| 未登录 | Sign in 提示 |
| 未信任工作区 | 只读；执行/写入禁用 |
| 版本不兼容 | 禁用执行；升级提示 |
| 空列表 | 解释 + 主操作 |
| 加载 | Progress/占位 |
| 错误 | 节点/通知 + Output/重试 |
| 审批等待 | Waiting + 打开 Approval Center |
| 离线 | 本地可用/远程禁用 |

## 16. i18n/可访问性

- Extension 原生 `package.nls.ja.json`/`zh-cn.json` + `vscode.l10n`；默认跟随 VS Code，fallback ja。
- Webview 传入 locale/messages；数字/日期 `Intl`。
- 高对比/键盘/屏幕阅读器；图标按钮 aria-label；状态不只靠颜色。
- 命令/设置/错误/通知全部本地化，无硬编码。

## 17. 相关

- [Extension 架构](extension-architecture.md)
- [Copilot 集成](copilot-integration.md)
- [Skill 集成](skill-integration.md)
- [MCP 集成](mcp-integration.md)
- [Local Runtime 桥接](local-runtime-bridge.md)
