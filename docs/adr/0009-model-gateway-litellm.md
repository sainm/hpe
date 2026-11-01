# ADR-0009：模型网关集成 LiteLLM

- 状态：已接受
- 日期：2026-11-01
- 决策者：架构负责人（Phase 0）

## 背景

Hisha 不打算自研模型 provider 适配/预算/路由；LiteLLM 提供统一 OpenAI 兼容接口、Router、预算/限流、成本、MCP Gateway，MIT 核心。需求要求本地/云/无模型可选。

## 决策

- LiteLLM（MIT 部分）作为 Model Gateway，通过 HTTP 集成；不在 Hisha 内嵌其 Python 代码。
- Hisha 掌握租户/密钥/预算映射：Team/Key ↔ tenant/workspace；Vault ↔ model config；spend logs → usage events。
- 排除 `enterprise/` 功能；SSO/RBAC 由 Hisha 自建。
- 模型配置/健康检查/成本可见；默认不绑定单一 provider；本地模型可经 OpenAI 兼容端点。
- 版本锁定/契约测试；LiteLLM 不可用不影响确定性 Workflow。

## 备选

- 自研 provider 适配：重复/维护高。
- 直接接各 provider：多套认证/计费/重试。
- LiteLLM 企业版：许可/身份边界冲突。

## 后果

- 多一个服务/DB/Redis 依赖；本地可选（无模型/本地模型可绕过）。
- 预算/成本双写/同步需要设计；避免两套真相。
- 版本升级/供应商适配变化；需要回归。
- 不把 LiteLLM 数据模型作为 Hisha 模型。

## 证据

[LiteLLM 报告](../01-research/projects/litellm.md)、[系统架构](../02-architecture/system-architecture.md)、[许可证分析](../01-research/license-analysis.md)。
