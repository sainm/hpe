# ADR-0003：Control Plane 采用 Modular Monolith

- 状态：已接受
- 日期：2026-11-01
- 决策者：架构负责人（Phase 0）

## 背景

平台范围大（Workflow/Agent/Skill/MCP/Plugin/Registry/Governance），但团队规模/Phase 1 交付要求快速迭代。微服务会引入网络/部署/一致性/调试成本；单一大泥球会失去边界。

## 决策

- Control Plane 采用 NestJS + TypeScript Modular Monolith。
- 模块：Identity、Tenancy、Capability、Workflow、Agent、Skill、MCP、Plugin、Connector、Execution、Trigger、Governance、Model、Observability、Notification。
- 每个模块拥有 controller → application service → domain → repository；跨模块只通过应用服务/outbox 事件。
- Execution Plane 的 Worker/Local Runtime 独立进程/包，可独立部署。
- 共享包只放契约/类型/纯函数，不放数据库实体。
- 微服务拆分留到 Phase 6 后按负载/团队边界。

## 备选

- 微服务：隔离强，但 Phase 1 过度；事务/事件/部署复杂。
- 单一 NestJS 模块：快，但边界混乱；后期拆分成本高。
- 多语言后端（Python/Go/Java）：与 TS IR/Worker 不一致；运行时已有多语言脚本。

## 后果

- 需要严格的模块边界/代码评审/依赖规则（禁止跨模块 ORM）。
- 单体扩展受限；Worker/API 可水平扩展缓解。
- 事件/outbox 必须从 Phase 1 建立，避免后期改造。
- 多租户/权限/审计作为横切模块，不可散落。

## 证据

[系统架构](../02-architecture/system-architecture.md)、[Control Plane](../02-architecture/control-plane.md)。
