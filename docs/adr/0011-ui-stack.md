# ADR-0011：React + Vite + XYFlow + Tailwind/shadcn

- 状态：已接受
- 日期：2026-11-01
- 决策者：架构负责人（Phase 0）

## 背景

需要现代开发者工作台、可视化画布、Monaco、高信息密度、三语、可访问性；候选 React/Next.js/Vue；画布 XYFlow/Vue Flow/自研。

## 决策

- React 19 + TypeScript + Vite（SPA）；不用 Next.js（本地/部署简单、无 SSR 需求）。
- `@xyflow/react` 12 作为画布；Monaco 作为 YAML/JSON/表达式编辑器。
- Tailwind CSS 4 + shadcn/ui（代码生成）+ 自有 Design Tokens；不直接复制 Dify/n8n/Flowise UI。
- Zustand（编辑器状态）+ TanStack Query（服务端状态）+ TanStack Router。
- i18next/react-i18next；默认 ja，en/zh。
- 窄屏 607px 适配；键盘/可访问性门禁。

## 备选

- Next.js：SSR/生态好，但本地/部署/复杂度不必要。
- Vue 3 + Vue Flow：Kestra 选择；团队/生态与 Hisha 其余 TS 不冲突，但 React 生态/组件更丰富。
- 自研画布：控制强但成本高；XYFlow 成熟。
- MUI/Chakra：可用，但视觉/定制/密度不如 Tailwind/shadcn。

## 后果

- 需要设计系统/组件治理；避免 shadcn 代码散落。
- XYFlow 大图性能需优化（虚拟化/分组/简化边）。
- 版本 peerDependencies 必须复核（React 19/XYFlow 12/Monaco）。
- 不能复制受限 UI 代码（n8n/Dify/Flowise）。

## 证据

[UI 设计系统](../09-ui/ui-design.md)、[Workflow Designer](../09-ui/workflow-designer.md)、npm 版本/peer 元数据（2026-11-01）。
