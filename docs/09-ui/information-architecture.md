# 信息架构

> 产品形态：现代开发者工作台，不是传统后台，也不是聊天窗口。默认日语；日/英/中完整 i18n；高信息密度、可键盘操作、窄屏可用。

## 1. 全局框架

```text
┌──────────────────────────────────────────────────────────────┐
│ TopBar: logo | workspace/project | search | command | locale │
│         | notifications | user                              │
├──────┬───────────────────────────────────────────────────────┤
│ Side │ Page content                                          │
│ Rail │                                                       │
│      │                                                       │
└──────┴───────────────────────────────────────────────────────┘
```

- 左侧 Rail：一级导航（图标 + 文字；可折叠）。
- TopBar：工作区/项目切换、全局搜索、命令面板、语言（ja/en/zh）、通知、用户。
- 内容区：页面头部（标题/面包屑/主操作）+ 内容；复杂页面用三栏/分栏。
- 全局命令面板：`Cmd/Ctrl+K`；跳转、创建、运行、搜索。
- 无隐藏的“更多”主操作；危险操作需确认/输入名称。
- 另有独立的 VS Code Extension 入口（Auralis Explorer），不复制完整 Designer；见 [VS Code/Copilot 模块](../14-vscode-copilot/README.md)。

## 2. 一级导航

| 导航 | 路由 | 说明 |
| --- | --- | --- |
| Overview | `/` | 平台概览/最近/告警/待办 |
| Workflow | `/workflows` | 列表、Designer、版本、执行 |
| Agents | `/agents` | Agent 列表/设计/测试 |
| Skills | `/skills` | Skill 列表/编辑/测试/导入导出 |
| MCP | `/mcp` | Server/Client/Tools/Resources/Prompts/Explorer |
| Plugins | `/plugins` | 已安装/市场 |
| Registry | `/registry` | 能力注册/关系图 |
| Connections | `/connections` | 连接器/凭证/外部服务 |
| Executions | `/executions` | 执行监控/日志/时间线 |
| Approvals | `/approvals` | 待办审批 |
| Marketplace | `/marketplace` | 模板/插件/Skill |
| Settings | `/settings` | 用户/权限/模型/部署/系统 |

> Workflow/Agent/Skill/MCP/Plugin 的“市场”入口统一到 Marketplace；页面内提供“浏览市场”链接，避免重复导航。

## 3. 路由/深链

```text
/w/:workspace/p/:project/workflows
/w/:workspace/p/:project/workflows/:workflowId
/w/:workspace/p/:project/workflows/:workflowId/designer?version=draft|1.2.0
/w/:workspace/p/:project/workflows/:workflowId/versions
/w/:workspace/p/:project/executions/:executionId
/w/:workspace/p/:project/agents/:agentId
/w/:workspace/p/:project/skills/:skillId
/w/:workspace/p/:project/mcp/servers/:serverId
/w/:workspace/p/:project/plugins/:pluginId
/w/:workspace/p/:project/registry/:capabilityId
/w/:workspace/p/:project/approvals/:approvalId
/w/:workspace/settings/...
```

- 深链可分享；权限不足显示请求访问/返回。
- 浏览器后退保持画布/检查器状态（可恢复）。
- 版本/执行/审批 ID 稳定；不把临时 UI 状态写进 URL（除版本/选中节点可选）。

## 4. 页面模式

| 模式 | 用于 | 结构 |
| --- | --- | --- |
| List/Detail | Workflow/Agent/Skill/MCP/Plugin/Execution | 过滤/表格/卡片 + 详情抽屉/页 |
| Three-column | Designer | Palette / Canvas / Inspector |
| Split | Skill Editor/MCP Explorer | 文件树/编辑 + 预览/测试 |
| Dashboard | Overview/Execution Monitor | 指标卡 + 图表 + 列表 |
| Workbench | Approval/Execution | 列表 + 详情 + 操作栏 |
| Settings | 配置 | 二级导航 + 表单 |

## 5. 全局状态

| 状态 | 表现 |
| --- | --- |
| 未登录 | 登录页；OIDC/本地；不泄漏资源 |
| 无租户/工作区 | 创建/加入引导 |
| 权限不足 | 403 页 + 请求访问；不显示敏感元数据 |
| 离线（Local） | 顶部“离线”徽标；云功能禁用；本地可用 |
| 加载 | 骨架屏；不阻塞导航 |
| 空 | 空状态 + 主操作 + 文档链接 |
| 错误 | 可操作错误 + traceId + 重试 |
| 冲突 | ETag/版本冲突；对比/覆盖/另存 |
| 只读 | 徽标；隐藏编辑操作；解释原因 |

## 6. 全局交互

- 命令面板：跳转/创建/运行/搜索；最近使用。
- 全局搜索：Workflow/Agent/Skill/MCP/Execution/Approval；按权限过滤；快捷键。
- 通知：审批/执行失败/发布/系统；可配置。
- 复制/分享：深链、YAML/JSON、执行 ID；权限检查。
- 危险操作：删除/发布/回滚/撤销凭证；确认 + 输入名称/二次认证（按策略）。
- 键盘：Tab 顺序、Esc 关闭、Cmd/Ctrl+S 保存、Cmd/Ctrl+Enter 运行、Cmd/Ctrl+K 命令面板。
- 自动保存草稿（可关闭）；显示保存状态/冲突。

## 7. 响应式

- **桌面 ≥1280**：三栏/分栏完整；高密度表格。
- **平板 768-1279**：Palette/Inspector 可折叠；底部面板可调。
- **窄屏 ≈607px**：单列；Palette/Inspector 变抽屉；底部调试器变 Tab/全屏；TopBar 压缩；表格变卡片/横向滚动；关键操作固定底部。
- 不用 hover-only 交互；触控目标 ≥44px。
- 窄屏验证是发布门禁（见 UI 设计系统）。

## 8. i18n

- 默认 `ja`；支持 `en`、`zh`；用户/工作区可覆盖。
- 所有 UI 文案走 i18n；无硬编码；错误/空状态/确认/文档链接同样。
- 日语语气：简洁、礼貌（です/ます），避免口语/夸张；业务术语保留英文（Workflow/Agent/Skill/MCP）。
- 日期/时间/数字/货币/时区本地化；执行时间显示相对 + 绝对。
- 文案长度差异：按钮/表格列需适配 en/zh；避免固定宽度截断。

## 9. 可访问性

- WCAG 2.2 AA 目标；对比度、焦点可见、键盘可达。
- 图标按钮有 aria-label；表单 label/error 关联。
- 动效可关闭（prefers-reduced-motion）。
- 图表/状态不仅靠颜色；提供文本/图标。
- 屏幕阅读器：表格/树/时间线语义；Monaco 编辑器需可切换纯文本/提供状态。

## 10. 导航图

见 [图集 §9](../02-architecture/diagrams.md#9-页面导航)。

## 11. 相关文档

- [UI 设计系统](ui-design.md)
- [Workflow Designer](workflow-designer.md)
- [Agent Center](agent-center.md)
- [Skill Center](skill-center.md)
- [MCP Center](mcp-center.md)
- [Execution Monitor](execution-monitor.md)
- [Local Runtime Manager](local-runtime-manager.md)
- [Plugin Marketplace](plugin-marketplace.md)
- [Approval Center 与其他页面](approvals-and-settings.md)
