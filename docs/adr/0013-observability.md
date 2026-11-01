# ADR-0013：OpenTelemetry 作为可观测性标准

- 状态：已接受
- 日期：2026-11-01
- 决策者：架构负责人（Phase 0）

## 背景

需要跨 Control Plane/Worker/Local Runtime/Agent/Skill/MCP/Connector 的 trace/metrics/logs；不能用厂商绑定；审计与执行事件必须独立。OpenTelemetry 是厂商中立标准（Apache-2.0）。

## 决策

- 所有运行时使用 OTel SDK（TS/Python）；OTLP 到 Collector；后端可选 Prometheus/Grafana/Tempo/Jaeger/云。
- 定义 Hisha 语义约定：`hpa.tenant_id`、`hpa.workspace_id`、`hpa.execution_id`、`hpa.node_id`、`hpa.agent_session_id`、`hpa.skill_id`、`hpa.mcp_server_id`、`hpa.tool_call_id`。
- Trace context 贯穿 Workflow → Agent → Skill → MCP → Connector；MCP `_meta` 传播。
- 审计独立：AuditLog 追加/不可篡改；Trace 不是审计。
- 采样/保留/脱敏：不记录 secret/PII；高基数标签限制；本地默认关闭导出/可配置。
- 本地/私有/SaaS 提供 Collector 配置样例。

## 备选

- 厂商 APM 绑定：锁定/成本/隐私。
- 仅日志：无法跨服务关联/性能分析。
- 自研 tracing：重复/生态差。
- 把执行事件当 trace：语义/保留/审计不匹配。

## 后果

- 需要 Collector/后端运维（SaaS/Private）；本地可选。
- 语义约定/基数治理；否则成本/噪声。
- SDK 升级/导出器变化；需要兼容测试。
- 审计/事件/Trace 三者边界必须清晰。

## 证据

[OpenTelemetry 报告](../01-research/projects/opentelemetry.md)、[安全架构](../02-architecture/security-architecture.md)、[执行平面](../02-architecture/execution-plane.md)。
