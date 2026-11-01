# Hisha 设计文档

> Phase 0 · Research & Architecture<br>
> 证据冻结日期：2026-11-01<br>
> 文档语言：中文；产品术语保留英文原文。<br>
> UI 默认语言：日本語；支持日本語 / English / 中文。<br>
> 品牌与内部 ID：展示名 Auralis；永久内部 ID **hpa**；历史别名 Hisha。规则见 [BRANDING.md](../BRANDING.md)。

## 阅读路径

- **决策者**：产品愿景 → 产品需求 → 开源复用与许可证 → 15 个结论题 → 路线图与风险。
- **架构师**：系统架构 → 契约 → 执行平面 → Workflow IR → 执行引擎 → 数据模型 → 安全架构。
- **模块负责人**：Workflow / Agent / Skill / MCP / Plugin 各自规范，再读测试策略与验收标准。
- **设计与前端**：信息架构 → UI 设计系统 → 关键页面规格。
- **交付负责人**：PoC 方案 → 路线图 → 风险登记 → 验收标准。

## 目录

### 00 产品

- [品牌与内部 ID](../BRANDING.md)
- [产品愿景](00-product/product-vision.md)
- [产品需求](00-product/product-requirements.md)
- [用户场景](00-product/user-scenarios.md)
- [术语表](00-product/glossary.md)

### 01 研究

- [研究方法、版本与证据](01-research/README.md)
- [开源项目对比矩阵](01-research/open-source-analysis.md)
- [许可证分析](01-research/license-analysis.md)
- [架构对比](01-research/architecture-comparison.md)
- [复用策略](01-research/reuse-strategy.md)
- 项目报告：
  [Dify](01-research/projects/dify.md) ·
  [n8n](01-research/projects/n8n.md) ·
  [Kestra](01-research/projects/kestra.md) ·
  [Activepieces](01-research/projects/activepieces.md) ·
  [LangGraph](01-research/projects/langgraph.md) ·
  [Temporal](01-research/projects/temporal.md) ·
  [Flowable](01-research/projects/flowable.md) ·
  [Node-RED](01-research/projects/node-red.md) ·
  [Flowise](01-research/projects/flowise.md) ·
  [Airflow](01-research/projects/airflow.md) ·
  [Dagster](01-research/projects/dagster.md) ·
  [LiteLLM](01-research/projects/litellm.md) ·
  [OpenTelemetry](01-research/projects/opentelemetry.md) ·
  [MCP Servers](01-research/projects/mcp-servers.md) ·
  [Anthropic Skills](01-research/projects/anthropic-skills.md) ·
  [awesome-copilot](01-research/projects/awesome-copilot.md)
- 标准研究：
  [MCP 2026-07-28](01-research/standards/mcp-2026-07-28.md) ·
  [Agent Skills](01-research/standards/agent-skills.md) ·
  [外部 AI 客户端](01-research/standards/external-ai-clients.md)

### 02 架构

- [系统架构](02-architecture/system-architecture.md)
- [Control Plane](02-architecture/control-plane.md)
- [Execution Plane](02-architecture/execution-plane.md)
- [Local Runtime](02-architecture/local-runtime.md)
- [部署架构](02-architecture/deployment-architecture.md)
- [安全架构](02-architecture/security-architecture.md)
- [统一契约](02-architecture/contracts.md)
- [图集：9 张关键图](02-architecture/diagrams.md)

### 03 Workflow

- [Workflow IR](03-workflow/workflow-ir.md)
- [YAML / JSON Schema 与保真](03-workflow/yaml-json-schema.md)
- [节点规范](03-workflow/node-specification.md)
- [执行引擎](03-workflow/execution-engine.md)
- [表达式与数据映射](03-workflow/expression-and-mapping.md)
- [AI Workflow 生成](03-workflow/ai-workflow-generation.md)

### 04 Agent

- [Agent 架构](04-agent/agent-architecture.md)
- [Agent Runtime](04-agent/agent-runtime.md)
- [多 Agent](04-agent/multi-agent.md)
- [Phase 4 本地 Agent Platform 实现](04-agent/local-agent-platform.md)

### 05 Skill

- [Skill 规范](05-skill/skill-specification.md)
- [Skill Runtime](05-skill/skill-runtime.md)
- [本地执行](05-skill/local-execution.md)
- [Skill 安全](05-skill/skill-security.md)
- [分发与版本](05-skill/skill-distribution.md)

### 06 MCP

- [MCP 架构](06-mcp/mcp-architecture.md)
- [MCP Client](06-mcp/mcp-client.md)
- [MCP Server 与 Gateway](06-mcp/mcp-server.md)
- [MCP 安全](06-mcp/mcp-security.md)
- [外部 AI 集成](06-mcp/external-ai-integration.md)

### 07 Plugin

- [插件架构](07-plugin/plugin-architecture.md)
- [Plugin SDK](07-plugin/plugin-sdk.md)
- [Marketplace](07-plugin/marketplace.md)
- [Phase 5 本地 Plugin & Connector 实现](07-plugin/local-implementation.md)

### 08 平台

- [数据模型](08-platform/data-model.md)
- [API 规范](08-platform/api-specification.md)
- [连接器架构](08-platform/connector-architecture.md)
- [权限模型](08-platform/permission-model.md)
- [统一能力注册中心](08-platform/capability-registry.md)
- [Phase 6 本地企业控制平面](08-platform/local-enterprise.md)
- [企业部署与运维](08-platform/enterprise-operations.md)
- [Phase 8 Hybrid Worker 协议](08-platform/hybrid-workers.md)
- [Hybrid Worker 私有部署](08-platform/hybrid-worker-deployment.md)
- [Phase 9 Browser SSO](08-platform/browser-sso.md)
- [Phase 10 OAuth 2.1 授权服务器](08-platform/oauth-authorization-server.md)

### 09 UI

- [信息架构](09-ui/information-architecture.md)
- [UI 设计系统](09-ui/ui-design.md)
- [Workflow Designer](09-ui/workflow-designer.md)
- [Agent Center](09-ui/agent-center.md)
- [Skill Center](09-ui/skill-center.md)
- [MCP Center](09-ui/mcp-center.md)
- [Execution Monitor](09-ui/execution-monitor.md)
- [Local Runtime Manager](09-ui/local-runtime-manager.md)
- [Plugin Marketplace](09-ui/plugin-marketplace.md)
- [Approval Center 与其他页面](09-ui/approvals-and-settings.md)

### 10 质量

- [测试策略](10-quality/test-strategy.md)
- [验收标准](10-quality/acceptance-criteria.md)
- [性能要求](10-quality/performance-requirements.md)

### 11 路线图

- [实施路线图](11-roadmap/implementation-roadmap.md)
- [风险登记](11-roadmap/risk-register.md)
- [三个 PoC](11-roadmap/poc-plan.md)

### 12 决策

- [15 个最终设计问题](12-decisions/final-questions.md)
- [待决问题](12-decisions/open-issues.md)

### 13 技术栈

- [技术栈总览](13-tech-stack/README.md)
- [语言策略](13-tech-stack/language-strategy.md)
- [Rust 技术栈](13-tech-stack/rust-stack.md)
- [TypeScript 技术栈](13-tech-stack/typescript-stack.md)
- [Python 技术栈](13-tech-stack/python-stack.md)
- [跨语言通信](13-tech-stack/cross-language.md)
- [Script/Plugin 执行](13-tech-stack/script-plugin-execution.md)
- [Local Runtime（Rust）](13-tech-stack/local-runtime-rust.md)
- [跨平台](13-tech-stack/cross-platform.md)
- [Monorepo、构建与发布](13-tech-stack/monorepo-build-release.md)
- [CSS 架构](13-tech-stack/css-architecture.md)
- [风险与维护成本](13-tech-stack/risks.md)
- [MVP 最小技术栈](13-tech-stack/mvp-stack.md)
- [技术栈证据](13-tech-stack/evidence.md)

### 14 VS Code & GitHub Copilot（增量模块）

- [模块总览](14-vscode-copilot/README.md)
- [Phase 7 本地实现与边界](14-vscode-copilot/local-implementation.md)
- [集成目标与边界](14-vscode-copilot/integration-objective.md)
- [Extension 架构](14-vscode-copilot/extension-architecture.md)
- [Copilot 集成](14-vscode-copilot/copilot-integration.md)
- [Skill 集成](14-vscode-copilot/skill-integration.md)
- [MCP 集成与工具](14-vscode-copilot/mcp-integration.md)
- [Local Runtime 桥接](14-vscode-copilot/local-runtime-bridge.md)
- [Extension UI](14-vscode-copilot/extension-ui.md)
- [用户场景](14-vscode-copilot/user-scenarios.md)
- [安全与权限](14-vscode-copilot/security.md)
- [测试与发布](14-vscode-copilot/testing-release.md)
- [官方证据](14-vscode-copilot/evidence.md)

### ADR

- [ADR 索引](adr/README.md)

## 文档状态约定

| 标记 | 含义 |
| --- | --- |
| 已定稿 | Phase 0 基线，变更需 ADR |
| 待验证 | 需要 PoC 或真实客户端验证 |
| 待决 | 需要用户或业务方裁决，见 open-issues |
| 未做 | 本阶段明确不交付 |

## 非目标

本阶段不交付可运行平台、不自研大模型、不构建云计费系统、不实现完整 BPMN 兼容、不承诺外部系统的 Exactly Once。PoC 方案已给出，但不在本轮执行。
