# ADR-0010：PostgreSQL + Redis + 对象存储

- 状态：已接受
- 日期：2026-11-01
- 决策者：架构负责人（Phase 0）

## 背景

需要存储 IR/版本/执行事件/审批/Agent 状态/审计/产物；本地需 SQLite；云需多租户/扩展；不能过度建模/引入图数据库。

## 决策

- 主库 PostgreSQL；本地 SQLite；JSONB 存 IR/配置；查询字段规范化。
- Redis 用于队列/租约/缓存/限流（本地可不用）。
- 对象存储（S3/MinIO）存产物/大日志/大状态；内容寻址。
- 执行事件追加式；检查点单独表；审计独立。
- 多租户 tenant_id + RLS；UUIDv7；保留/归档策略。
- 不引入图数据库；能力关系用边表/递归 CTE。

## 备选

- 图数据库：复杂关系查询少；运维/一致性成本高。
- MongoDB/文档库：事务/RLS/查询不如 Postgres。
- 只文件/SQLite：云多租户/并发不足。
- 事件存储专用（Kafka/EventStore）：初期过度；用 outbox/事件表。

## 后果

- 大事件/产物需分页/引用；JSONB 索引需克制。
- 执行事件/审计增长快；分区/归档/保留必须设计。
- SQLite/Postgres 差异需契约测试；本地功能降级明确。
- RLS/多租户测试是门禁。

## 证据

[数据模型](../08-platform/data-model.md)、[部署架构](../02-architecture/deployment-architecture.md)、[性能要求](../10-quality/performance-requirements.md)。
