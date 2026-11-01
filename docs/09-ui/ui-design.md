# UI 设计系统

> 风格：现代开发者工作台。中性底色、清晰层级、高信息密度；强调色只用于主操作/选中/状态。默认日语，日/英/中一致。
> CSS 实现：Tailwind CSS 4 + CSS Variables + CSS Modules + PostCSS；无运行时 CSS-in-JS。完整规则见 [CSS 架构](../13-tech-stack/css-architecture.md)。

## 1. 设计原则

1. **工作台而非后台**：信息密度高、键盘优先、少装饰、少弹窗。
2. **状态可见**：保存/运行/审批/离线/权限/冲突始终可见。
3. **危险操作显式**：删除/发布/回滚/撤销凭证需确认。
4. **一致**：同一动作在 Workflow/Agent/Skill/MCP 使用同一模式/术语。
5. **可访问**：键盘、对比度、屏幕阅读器、减少动效。
6. **国际化**：默认日语；所有文案/格式本地化。

## 2. Design Tokens

> 完整 token（z-index/断点/动效/组件尺寸/主题变量）见 [CSS 架构](../13-tech-stack/css-architecture.md)。

### 颜色（浅色）

| Token | 值 | 用途 |
| --- | --- | --- |
| `--bg` | `#F7F8FA` | 页面背景 |
| `--surface` | `#FFFFFF` | 卡片/面板 |
| `--surface-2` | `#F1F3F6` | 次级面板/hover |
| `--border` | `#E3E6EA` | 边框/分隔 |
| `--text` | `#111827` | 主文本 |
| `--text-2` | `#4B5563` | 次文本 |
| `--text-3` | `#6B7280` | 辅助/占位 |
| `--primary` | `#2563EB` | 主操作/选中 |
| `--primary-hover` | `#1D4ED8` | hover |
| `--accent` | `#0D9488` | 强调/链接辅助 |
| `--success` | `#16A34A` | 成功 |
| `--warning` | `#D97706` | 警告/等待 |
| `--danger` | `#DC2626` | 危险/失败 |
| `--info` | `#0891B2` | 信息 |
| `--focus` | `#93C5FD` | 焦点环 |

### 颜色（深色）

| Token | 值 |
| --- | --- |
| `--bg` | `#0B0F14` |
| `--surface` | `#11161D` |
| `--surface-2` | `#1A212B` |
| `--border` | `#28313D` |
| `--text` | `#E5E7EB` |
| `--text-2` | `#9CA3AF` |
| `--text-3` | `#6B7280` |
| `--primary` | `#3B82F6` |
| `--focus` | `#1E3A8A` |

> 语义色不直接用于大面积背景；状态同时用图标/文字，避免仅靠颜色。

### 字体

| 用途 | 字体 | 字号/行高 |
| --- | --- | --- |
| UI | `"Inter", "Noto Sans JP", "Hiragino Sans", "Yu Gothic", sans-serif` | 12/18, 13/20, 14/22, 16/24 |
| 标题 | 同上，600 | 20/28, 24/32 |
| 代码 | `"JetBrains Mono", "SFMono-Regular", monospace` | 12/18, 13/20 |
| 表格 | UI | 12/18（compact）/13/20 |
| 辅助 | UI | 11/16 |

### 间距/圆角/阴影

- 间距：2, 4, 6, 8, 12, 16, 24, 32, 40, 48。
- 圆角：4（输入）、6（按钮/卡片）、8（面板）、12（模态/大卡片）、999（徽标）。
- 边框：1px `--border`；选中 1px `--primary` + 2px focus ring。
- 阴影：`0 1px 2px rgba(16,24,40,.06)`（卡片）、`0 8px 24px rgba(16,24,40,.12)`（浮层）。
- 密度：表格 compact 32px 行高；comfortable 40px；编辑器 compact。

### 布局尺寸

| 区域 | 宽/高 |
| --- | --- |
| Rail | 240px（展开）/64px（折叠） |
| TopBar | 48px |
| Inspector | 360px（可调 320-480） |
| Palette | 280px |
| Bottom panel | 240px（可调 160-480） |
| 内容最大宽 | 1440px（列表/表单）；画布占满 |
| 触控目标 | ≥44×44px（窄屏） |

## 3. 组件

| 组件 | 用途 | 状态 |
| --- | --- | --- |
| AppShell | Rail/TopBar/内容 | collapsed/offline/loading |
| DataTable | 列表 | empty/loading/error/selected/sorted/filtered |
| EntityCard | Workflow/Agent/Skill/MCP | status/labels/actions |
| Inspector | 配置面板 | dirty/read-only/error/conflict |
| SchemaForm | 节点/连接器配置 | field error/required/disabled |
| CodeEditor | YAML/JSON/Monaco | read-only/dirty/error/diff |
| DiffViewer | 版本/生成 diff | added/removed/changed |
| StatusBadge | 执行/审批/信任 | queued/running/waiting/succeeded/failed/cancelled |
| Timeline | 执行/审计 | live/paused/filtered |
| LogViewer | 日志 | follow/pause/search/filter/truncated |
| CommandPalette | 全局命令 | empty/loading/results |
| Drawer | 窄屏 Inspector/详情 | open/close/focus trap |
| Toast | 操作结果 | success/error/info/action |
| ConfirmDialog | 危险操作 | require-name/step-up |
| PermissionPicker | 权限/审批 | search/group/selected |
| EntityPicker | 选择 Workflow/Agent/Skill/MCP | search/version/empty |
| EmptyState | 空 | icon/title/description/action/docs |
| ErrorState | 错误 | code/message/trace/remediation |
| Skeleton | 加载 | list/detail/table |
| Tabs | 底部/详情 | active/overflow |
| SplitPane | 编辑器/详情 | resizable/collapsed |
| GraphView | 能力关系/执行图 | zoom/pan/selected |

## 4. 页面状态

每个页面必须定义：

- **loading**：骨架屏；不闪烁。
- **empty**：解释 + 主操作 + 文档。
- **error**：可操作错误 + traceId + 重试；不显示堆栈。
- **permission**：403 + 请求访问 + 返回；不泄漏资源。
- **offline**：本地可用/云禁用；明确标识。
- **dirty**：保存/放弃/对比；离开确认。
- **conflict**：对比/覆盖/另存；不静默覆盖。
- **read-only**：原因/版本/权限；隐藏编辑。
- **running/waiting**：进度/审批/取消；不阻塞其他工作。

## 5. 交互规范

- 主操作每页一个（Primary）；次操作 Secondary；危险 Danger。
- 保存：自动保存草稿 + 手动 `Cmd/Ctrl+S`；显示“已保存/保存中/冲突”。
- 运行：`Cmd/Ctrl+Enter`；先校验；显示输入表单；运行中可取消。
- 删除：确认 + 依赖影响；高风险输入名称。
- 发布：校验结果/版本/变更摘要；不可跳过错误。
- 审批：显示影响/风险/参数摘要；批准/拒绝/转交；不可批量批准高风险（可配置）。
- 搜索：即时/防抖；空结果建议；键盘上下选择。
- 复制：深链/YAML/JSON/ID；权限检查。
- 通知：不打断；审批/失败可操作。

## 6. 响应式（607px 门禁）

- ≥1280：三栏/分栏；Inspector 固定。
- 768-1279：Palette/Inspector 可折叠；底部面板可调。
- ≤607：
  - Rail 折叠为图标/底部导航（按页面）。
  - Palette/Inspector 变全屏抽屉；一次只显示一个。
  - 底部调试器变 Tab（输入/输出/日志/时间线），可全屏。
  - 表格变卡片/横向滚动；主操作固定底部。
  - TopBar 只保留返回/标题/主操作/菜单。
  - 触控目标 ≥44px；不使用 hover-only。
- 窄屏必须实测：Workflow Designer、Skill Editor、MCP Explorer、Execution Detail、Approval Detail。

## 7. 可访问性

- 对比度 ≥4.5:1（正文）/3:1（大文本/图标）。
- 焦点可见；焦点陷阱仅模态；Esc 关闭。
- 表单 label/描述/错误关联；必填标识。
- 表格/树/时间线语义；键盘操作。
- 图标按钮 aria-label；状态文本。
- 动效 ≤240ms；`prefers-reduced-motion` 关闭。
- Monaco：提供只读/纯文本切换；错误列表可键盘访问。

## 8. i18n

- 默认 `ja`；`en`/`zh`；用户/工作区覆盖。
- 所有文案（含错误/空状态/确认/邮件/通知）走 i18n。
- 日语简短礼貌；术语保留英文；避免长句导致按钮溢出。
- 日期/时间/数字/货币/时区本地化；相对时间 + 绝对时间 tooltip。
- 文案 key 语义化；禁止拼接句子（用 ICU/占位符）。
- 截图/测试覆盖三语；窄屏检查 en/zh 长度。

## 9. 视觉细节

- 选中节点：2px primary 边框 + 轻背景；不改变布局。
- 状态点：小圆点 + 文本；执行中脉冲（可关闭）。
- 连线：默认 1.5px 中性；选中 2px primary；错误红色虚线。
- 画布网格：低对比；可关闭。
- 图标：Lucide；统一 16/20px。
- 空状态插画：克制；不使用夸张 3D。
- 表格：斑马纹可选；固定表头；列宽可调；批量操作栏。

## 10. 相关文档

- [信息架构](information-architecture.md)
- [Workflow Designer](workflow-designer.md)
- [Execution Monitor](execution-monitor.md)
- [Skill Center](skill-center.md)
- [MCP Center](mcp-center.md)
