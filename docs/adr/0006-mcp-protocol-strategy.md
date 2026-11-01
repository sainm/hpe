# ADR-0006：MCP 以 2026-07-28 为目标 + 旧版兼容

- 状态：已接受
- 日期：2026-11-01
- 决策者：架构负责人（Phase 0）

## 背景

MCP 2026-07-28 是无状态重大修订（移除 initialize/会话、server/discover、MRTR、subscriptions/listen、resultType、tasks 扩展）。官方 TypeScript SDK 1.32.2 稳定版仍以 2025-11-25 为最新；但 **Rust RMCP 3.8.0 已支持 2026-07-28 与 2025-11-25**，因此 MCP 协议实现放在 Rust Core 是可行的。

## 决策

- Gateway/本地 Server 以 **Rust RMCP 3.8.0** 实现；目标 2026-07-28，同时支持 2025-11-25/2025-06-18/2025-03-26 兼容。
- 传输：stdio + Streamable HTTP；RMCP 负责会话/无状态/`server/discover`/MRTR/`subscriptions/listen`/resultType/tasks；Hisha 只加租户/策略/审批/注册表/审计。
- 认证：OAuth 2.1（RFC 9728/8414/8707/CIMD/DCR/9207）+ PAT/stdio env；禁止 token passthrough。
- 工具由 Capability Registry 生成；高风险审批在 Hisha 服务端强制。
- TypeScript Control Plane 只管理 Server/工具绑定/权限/审计，不实现 MCP 协议；不再依赖 TS SDK 作为主实现。

## 备选

- 只支持 SDK 稳定版 2025-11-25：落后规范/客户端，不符合需求。
- 只支持 2026-07-28：旧客户端不可用。
- 直接用第三方“最新 MCP”实现：协议/安全不可控。

## 后果

- 需要 RMCP 版本/许可/NOTICE 治理；MCP 规范/实现处于 MIT→Apache 过渡，需法务复核。
- 客户端兼容矩阵必须真实测试；不能假设一个配置适配所有。
- 无会话后，认证/限流/幂等必须按请求设计；不能用连接状态。
- RMCP 升级/规范演进需 conformance；TS SDK 不再作为主协议层。

## 证据

[MCP 2026-07-28 研究](../01-research/standards/mcp-2026-07-28.md)、[MCP Server](../06-mcp/mcp-server.md)、[外部客户端](../01-research/standards/external-ai-clients.md)。
