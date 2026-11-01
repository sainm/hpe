# ADR-0017：CSS 架构

- 状态：已接受
- 日期：2026-11-01
- 决策者：架构/设计（Phase 0 技术栈）

## 背景

平台是长会话、高信息密度的开发者工作台，需要 Canvas/Monaco/Debugger/Agent/Skill/MCP/市场/执行监控等复杂 UI；必须支持明暗/自定义主题、日英中排版、607px 窄屏、可访问性，并避免全局 CSS/运行时 CSS-in-JS 的维护问题。

## 决策

- Tailwind CSS 4.3.3（CSS-first `@theme` + CSS Variables）+ PostCSS 8.5.32 + CSS Modules（Vite 8 内置）。
- 全局 CSS 仅 tokens/base/themes/tailwind 四层；组件/功能样式用 Tailwind utilities + CSS Module。
- Design Tokens：颜色/排版/间距/圆角/阴影/z-index/断点/动效/尺寸；语义 token 优先。
- 主题：light/dark/custom；`[data-theme]` + CSS 变量；自定义主题只允许 token 白名单，禁止任意 CSS 注入。
- 无运行时 CSS-in-JS；Radix primitives + CVA variants + clsx/tailwind-merge。
- 日英中字体栈/行高；不自托管巨型 CJK 字体；离线优先系统字体。
- 607px 单列/抽屉/Tab；触控 ≥44px；可访问性/对比度/焦点/减少动效。
- 性能预算/视觉回归/stylelint/axe；禁止全局选择器/`!important`/任意 hex/px 绕过 token。

## 备选

- MUI/Chakra/Ant：快但视觉/密度/运行时样式/定制受限。
- CSS-in-JS（Emotion/styled-components）：运行时开销/主题漂移；不选。
- 纯 CSS Modules：可控但 utility 开发慢；混合 Tailwind 更高效。
- Sass 作为主题源：与 CSS 变量重复；仅可选 mixin。

## 后果

- 需要 token/组件治理/stylelint/视觉回归；设计系统成为产品资产。
- Tailwind v4 新引擎/插件差异需锁定/回归。
- 自定义主题需对比度校验/安全白名单。
- Monaco/XYFlow 主题需与 CSS variables 同步。

## 证据

[CSS 架构](../13-tech-stack/css-architecture.md)、[UI 设计系统](../09-ui/ui-design.md)、[证据](../13-tech-stack/evidence.md)。
