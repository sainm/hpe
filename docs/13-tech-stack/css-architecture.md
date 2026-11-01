# CSS 架构与设计系统

> 目标：现代开发者工作台的样式体系，支持长会话、高信息密度、明暗/自定义主题、日英中排版、607px 窄屏与可访问性；样式与业务逻辑分离，无运行时 CSS-in-JS。
> 不是传统企业后台：避免大面积灰底卡片、低密度表单、重装饰；以画布/代码/时间线/轨迹为主，优先键盘与可读性。

## 1. 技术栈

| 技术 | 版本（2026-11-01） | License | 角色 |
| --- | --- | --- | --- |
| CSS3 | 浏览器原生 | — | 布局/变量/动画/容器查询 |
| Tailwind CSS | 4.3.3 | MIT | Utility-first；CSS-first `@theme` |
| @tailwindcss/postcss | 4.3.3 | MIT | PostCSS 管线（或 `@tailwindcss/vite`） |
| PostCSS | 8.5.32 | MIT | 项目级 CSS 变换/兼容 |
| CSS Variables | 原生 | — | Design Tokens/主题 |
| CSS Modules | Vite 8 内置 | — | 复杂组件局部样式 |
| Sass（可选） | 1.107.1 | MIT | 仅 mixin/function；非主题系统 |
| Lightning CSS（可选） | 1.33.0 | MPL-2.0 | 可选压缩/转换；不进核心 |
| Autoprefixer（可选） | 10.6.2 | MIT | 仅在目标浏览器需要时 |

- 主路径：`@tailwindcss/postcss` + PostCSS；Vite 自动处理 `.module.css`。`@tailwindcss/vite` 可作为性能替代，但不同时运行两套 Tailwind。
- 不引入 styled-components/Emotion 等运行时 CSS-in-JS；Radix 提供无样式 primitives，shadcn 生成可维护组件代码。
- 不把 Sass 作为第二套 token 系统；CSS Variables 是唯一主题源。

```js
// postcss.config.mjs
export default {
  plugins: {
    "@tailwindcss/postcss": {},
  },
};
```

### 1.1 架构规则（强制）

1. 样式与业务逻辑分离：组件不把颜色/间距/布局逻辑写进 TS。
2. 禁止不受控的全局 CSS：只有 tokens/base/themes/tailwind 四层。
3. 主题通过 CSS Variables；组件只用语义 token。
4. 布局/间距/排版优先 Tailwind utilities。
5. 复杂组件结构/伪元素/data 状态用 CSS Modules。
6. 避免内联 style；仅画布动态 transform/尺寸例外。
7. 禁止不必要的 CSS-in-JS 运行时依赖。
8. 全平台模块共享 `packages/ts/ui` 的 tokens/组件，不复制样式。
9. 可访问性与键盘导航是样式验收项（焦点/对比度/减少动效）。
10. 复用组件/variants，不在 feature 中重造按钮/表格/面板。

### 1.2 候选比较

| 方案 | 优点 | 问题 | 结论 |
| --- | --- | --- | --- |
| Tailwind 4 + CSS Modules + CSS vars | 高密度/快速/可定制/无运行时 | 需 token/规则治理 | **采用** |
| MUI/Chakra/Ant | 组件全/快 | 视觉同质化/运行时样式/定制成本 | 不采用 |
| CSS-in-JS（Emotion/styled） | 组件内聚 | 运行时开销/主题漂移/包体 | 不采用 |
| 纯 Sass + BEM | 可控 | 开发慢/主题/响应式重复 | 仅可选 mixin |
| 纯 CSS Modules | 局部性强 | utility 开发慢 | 与 Tailwind 混合 |

## 2. 分层

```text
packages/ts/ui/src/styles/
├── tailwind.css       # @import "tailwindcss"; @source ...
├── tokens.css         # 原始/语义 tokens（:root / [data-theme]）
├── base.css           # reset/排版/焦点/滚动条/基础元素
└── themes.css         # light/dark/custom 变量覆盖

packages/ts/ui/src/components/...
apps/web/src/features/workflow-canvas/Canvas.module.css
apps/web/src/features/execution-monitor/Timeline.module.css
```

| 层 | 允许 | 禁止 |
| --- | --- | --- |
| tokens | CSS variables、主题覆盖 | 组件选择器/业务类 |
| base | 元素 reset、排版、焦点、可访问性 | 组件样式 |
| components | Tailwind utilities + 少量 CSS Module | 页面布局/业务状态 |
| features | 页面/画布/编辑器布局、CSS Modules | 全局选择器/跨模块覆盖 |
| themes | `[data-theme]` 变量 | 组件结构 |

- 全局 CSS 仅 `tailwind.css`/`tokens.css`/`base.css`/`themes.css`；其他样式必须 CSS Module/Tailwind。
- 禁止 `body .xxx`、`#app .xxx` 等跨模块选择器；禁止 `!important`（除第三方覆盖且注释说明）。

## 3. Design Tokens

### 3.1 原始 tokens

```css
:root {
  --hpa-gray-0: #ffffff;
  --hpa-gray-50: #f7f8fa;
  --hpa-gray-100: #f1f3f6;
  --hpa-gray-200: #e3e6ea;
  --hpa-gray-500: #6b7280;
  --hpa-gray-700: #374151;
  --hpa-gray-900: #111827;

  --hpa-blue-500: #2563eb;
  --hpa-blue-600: #1d4ed8;
  --hpa-teal-600: #0d9488;
  --hpa-green-600: #16a34a;
  --hpa-amber-600: #d97706;
  --hpa-red-600: #dc2626;
  --hpa-cyan-600: #0891b2;

  --hpa-space-1: 2px;
  --hpa-space-2: 4px;
  --hpa-space-3: 6px;
  --hpa-space-4: 8px;
  --hpa-space-5: 12px;
  --hpa-space-6: 16px;
  --hpa-space-7: 24px;
  --hpa-space-8: 32px;
  --hpa-space-9: 40px;
  --hpa-space-10: 48px;

  --hpa-radius-sm: 4px;
  --hpa-radius-md: 6px;
  --hpa-radius-lg: 8px;
  --hpa-radius-xl: 12px;
  --hpa-radius-full: 999px;

  --hpa-font-sans: "Inter", "Noto Sans JP", "Hiragino Sans", "Yu Gothic", sans-serif;
  --hpa-font-mono: "JetBrains Mono", "SFMono-Regular", "Cascadia Code", monospace;

  --hpa-text-2xs: 11px;
  --hpa-text-xs: 12px;
  --hpa-text-sm: 13px;
  --hpa-text-md: 14px;
  --hpa-text-lg: 16px;
  --hpa-text-xl: 20px;
  --hpa-text-2xl: 24px;

  --hpa-z-base: 0;
  --hpa-z-sticky: 20;
  --hpa-z-drawer: 40;
  --hpa-z-modal: 50;
  --hpa-z-toast: 60;
  --hpa-z-tooltip: 70;

  --hpa-duration-fast: 120ms;
  --hpa-duration-base: 180ms;
  --hpa-duration-slow: 240ms;
  --hpa-ease-standard: cubic-bezier(0.2, 0, 0, 1);
}
```

### 3.2 语义 tokens

```css
:root {
  --bg: var(--hpa-gray-50);
  --surface: var(--hpa-gray-0);
  --surface-2: var(--hpa-gray-100);
  --border: var(--hpa-gray-200);
  --text: var(--hpa-gray-900);
  --text-2: var(--hpa-gray-700);
  --text-3: var(--hpa-gray-500);
  --primary: var(--hpa-blue-500);
  --primary-hover: var(--hpa-blue-600);
  --accent: var(--hpa-teal-600);
  --success: var(--hpa-green-600);
  --warning: var(--hpa-amber-600);
  --danger: var(--hpa-red-600);
  --info: var(--hpa-cyan-600);
  --focus: #93c5fd;
  --selection: color-mix(in srgb, var(--primary) 18%, transparent);
  --shadow-sm: 0 1px 2px rgb(16 24 40 / 0.06);
  --shadow-lg: 0 8px 24px rgb(16 24 40 / 0.12);
  --control-height-sm: 28px;
  --control-height-md: 32px;
  --control-height-lg: 36px;
  --row-height-compact: 32px;
  --row-height-comfortable: 40px;
  --rail-width: 240px;
  --rail-width-collapsed: 64px;
  --topbar-height: 48px;
  --inspector-width: 360px;
  --breakpoint-narrow: 607px;
}
```

### 3.3 Tailwind 映射

```css
@import "tailwindcss";
@import "./tokens.css";
@import "./themes.css";
@import "./base.css";

@theme inline {
  --color-bg: var(--bg);
  --color-surface: var(--surface);
  --color-surface-2: var(--surface-2);
  --color-border: var(--border);
  --color-text: var(--text);
  --color-text-2: var(--text-2);
  --color-primary: var(--primary);
  --color-danger: var(--danger);
  --font-sans: var(--hpa-font-sans);
  --font-mono: var(--hpa-font-mono);
  --radius-md: var(--hpa-radius-md);
  --breakpoint-narrow: 607px;
}
```

- Tailwind v4 使用 CSS-first `@theme`/CSS variables；不再需要 JS config 作为主题源。
- 组件只用语义 token（`bg-surface`、`text-text-2`），不用原始灰度/十六进制。
- 业务代码禁止硬编码颜色/间距/字号；动态值通过 CSS 变量。

## 4. 主题

```css
:root { color-scheme: light; }
[data-theme="dark"] {
  color-scheme: dark;
  --bg: #0b0f14;
  --surface: #11161d;
  --surface-2: #1a212b;
  --border: #28313d;
  --text: #e5e7eb;
  --text-2: #9ca3af;
  --text-3: #6b7280;
  --primary: #3b82f6;
  --focus: #1e3a8a;
  --selection: color-mix(in srgb, var(--primary) 28%, transparent);
}
[data-theme="custom"] {
  /* 由租户/用户 Theme JSON 生成，只允许 token 白名单，不注入任意 CSS */
}
```

- 主题优先级：用户 > 工作区 > 系统（`prefers-color-scheme`）。
- 自定义主题通过 Token JSON → CSS 变量；服务端校验对比度/合法值；禁止注入 `url()`/`expression()`/任意选择器。
- 切换主题不重建组件；仅改变 `data-theme`/变量；Monaco/图表/画布同步。
- 持久化用户偏好；SSR 不需要（SPA）。
- 状态色不只靠颜色：图标/文本/`aria`。

## 5. Tailwind 与 CSS Modules

### Tailwind

用于：布局（flex/grid/gap）、间距、排版、尺寸、边框、状态 utilities、响应式。
规则：

- 组件内优先语义 token/utilities；跨组件组合用 CVA variants。
- 禁止用 arbitrary values 绕过 token（`bg-[#123456]`、`p-[13px]`），除非一次性布局且有注释。
- 禁止 `@apply` 泛滥；仅在 CSS Module 中需要复用少量 utilities 时使用。
- 动态 class 必须来自静态映射，禁止拼接未声明字符串（Tailwind 无法生成）。

### CSS Modules

用于：复杂选择器/伪元素/子元素结构/data 属性状态/画布节点/时间线/日志/差异视图等 Tailwind 不擅长的样式。

```css
/* Canvas.module.css */
.node {
  position: absolute;
  display: flex;
  align-items: center;
  gap: var(--hpa-space-4);
  border: 1px solid var(--border);
  border-radius: var(--hpa-radius-md);
  background: var(--surface);
  box-shadow: var(--shadow-sm);
}
.node[data-selected="true"] {
  border-color: var(--primary);
  box-shadow: 0 0 0 2px color-mix(in srgb, var(--primary) 25%, transparent);
}
.node[data-status="failed"] { border-color: var(--danger); }
```

- 类名 camelCase/语义化；不使用全局 hash 依赖。
- 不复制 Tailwind 的 spacing/color；用变量。
- 只在允许层使用；feature 的 CSS Module 不导入其他 feature。

## 6. 组件样式规范

- 变体：CVA（`variant`, `size`, `tone`），类名组合 `clsx` + `tailwind-merge`。
- 状态：`data-state`/`aria-*` 驱动样式（hover/focus/disabled/loading/selected/error），不靠 JS 改 style。
- 焦点：`:focus-visible` 2px `--focus` ring；不只靠 outline 颜色。
- 禁用：降低对比度但保持可读；`cursor-not-allowed`；`aria-disabled`。
- 加载：骨架/进度；不改变布局尺寸。
- 危险：`--danger` + 文本/图标；确认对话框。
- 密度：compact/comfortable 通过 CSS 变量；表格/列表/编辑器。
- 图标：Lucide，16/20px；按钮图标 + 文本；图标按钮 aria-label。
- 分隔/卡片：1px border + 轻阴影；避免层层卡片。
- 动效：120-240ms；`prefers-reduced-motion` 关闭；不阻塞交互。

## 7. UI 模块重点

| 模块 | 样式重点 |
| --- | --- |
| Workflow Canvas | 节点状态/端口/边/选中/错误；CSS Modules；动态坐标用 inline style（仅 transform）；网格/小地图低对比 |
| Code Editor | Monaco 主题与 `data-theme` 同步；YAML/JSON 诊断色；编辑器容器 CSS Module；不覆盖 Monaco 内部全局类 |
| Execution Debugger | 日志/时间线/差异；等宽字体；虚拟滚动；状态色；高密度 |
| Agent Designer | 表单/预算/工具选择；可折叠分组；测试轨迹 |
| Skill Editor | 文件树/编辑/预览/测试；分栏；窄屏 Tabs |
| MCP Explorer | 请求/响应分栏；JSON 高亮；Schema 表单；连接状态 |
| Plugin Marketplace | 卡片/详情/权限/评分；信任徽标；截图比例 |
| Execution Monitor | 表格/时间线/日志/产物；高密度；实时状态；断线提示 |

## 8. 排版（日/英/中）

- 日文正文：`line-height: 1.7`；标题 1.4-1.5；避免负字距；可启用 `font-feature-settings: "palt"`（按字体测试）。
- 英文：Inter/Noto Sans；标题 600；正文 14px/22px。
- 中文：Noto Sans SC/PingFang SC/Microsoft YaHei；行高 1.65。
- 等宽：JetBrains Mono；日志/代码/YAML。
- 字体不通过 CDN 加载；默认系统字体；可选自托管 Noto 子集（离线/体积权衡）。
- 数字/日期/货币用 `Intl`；表格数字 `font-variant-numeric: tabular-nums`。
- 文案长度差异：按钮/表格列用 `min-width`/truncate + tooltip；不写死宽度。

## 9. 响应式

- 断点：`narrow: 607px`、`md: 768px`、`lg: 1024px`、`xl: 1280px`、`2xl: 1440px`。
- ≥1280：Rail 240 + 内容 + Inspector 360；高密度表格。
- 768-1279：Rail 折叠；Inspector 可抽屉。
- ≤607：单列；Palette/Inspector 全屏抽屉；底部面板 Tabs；主操作固定底部；触控 ≥44px。
- 不使用 hover-only 交互；键盘/触控可达。
- 容器查询用于卡片/面板自适应（可选）。

## 10. 可访问性

- 对比度：正文 ≥4.5:1，大文本/图标 ≥3:1；焦点环可见。
- 语义 HTML：button/a/table/label；不把 div 当按钮。
- 键盘：Tab 顺序、Esc 关闭、焦点陷阱仅模态、焦点返回。
- 状态：文本/图标/aria-live；不只靠颜色。
- 动效：`prefers-reduced-motion` 关闭/缩短。
- 触控：≥44×44px（窄屏）；间距足够。
- 屏幕阅读器：表格/树/时间线语义；Monaco 提供只读/文本模式。

## 11. 性能

- Tailwind 只生成使用的 utilities；`@source` 指向 apps/packages；不扫描 node_modules 全量。
- CSS Modules 按组件/路由；Vite 代码分割。
- 无运行时 CSS-in-JS；主题切换只改变 CSS 变量。
- 限制全局 CSS 体积；避免大型第三方 CSS；按需引入 Radix 组件样式。
- 图片/图标：SVG/字体子集；避免大图；懒加载。
- 预算：初始 CSS ≤80KB gzip（目标）；Designer 路由 CSS ≤120KB gzip；按路由拆分。
- 窄屏/低端设备减少阴影/动画。

## 12. 测试/门禁

- Stylelint（standard/order）+ Prettier；禁止硬编码颜色/间距（自定义规则或 review）。
- Playwright 视觉回归：light/dark/custom、ja/en/zh、1280/768/607。
- axe/Playwright 可访问性；键盘路径。
- CSS bundle 预算/重复类检查。
- 禁止全局选择器/`!important`/inline style（canvas 动态 transform 例外）。
- 组件 Storybook（可选）覆盖状态。

## 13. 反模式

- 用 CSS-in-JS 运行时（styled-components/Emotion）替代 CSS 变量/Tailwind。
- 全局 `.btn`/`.card` 在 feature 中定义/覆盖。
- 直接写十六进制/px 间距；绕过 token。
- 主题切换重渲染整页/重载 CSS 文件。
- 用 `!important` 解决层叠；用 scope/层解决。
- 把业务条件写进 CSS（用 data 属性/状态类）。
- 为每个页面复制组件样式；用 UI 包组件变体。

## 14. 相关文档

- [UI 设计系统](../09-ui/ui-design.md)
- [TypeScript 技术栈](typescript-stack.md)
- [Workflow Designer](../09-ui/workflow-designer.md)
- [信息架构](../09-ui/information-architecture.md)
- [证据](evidence.md)
