# OpenTelemetry Specification 1.62.0 研究报告

> 仓库：`open-telemetry/opentelemetry-specification` · tag `v1.62.0`（2026-10-22）· tarball SHA-256 `87d5741e92ed1201...`<br>
> 研究方式：规范源码（350 文件）+ Apache-2.0 + trace/metrics/logs/protocol 章节；未实现。<br>
> 结论：**Direct Reuse（可观测性标准与 SDK/Collector）**。

## 1. 项目定位

厂商中立的可观测性规范：定义 traces、metrics、logs、baggage、context、resources、OTLP 协议与 SDK 行为。SDK、Collector、语义约定是独立仓库/项目；本仓库只定义“应该是什么”。

## 2. 核心功能（规范范围）

- Trace：span、context propagation、sampling、exporters。
- Metrics：instrument、aggregation、temporality、exemplars。
- Logs：log record、severity、correlation。
- Baggage、Resources、Profiles（规范中的新信号/实验性内容需按 maturity 阅读）。
- OTLP：gRPC/HTTP 传输、protobuf 编码、重试/节流。
- SDK/API 分层、no-op、性能/错误处理指南。

## 3. 技术架构

规范文本 + JSON schema/模型；`specification/trace`、`metrics`、`logs`、`baggage`、`context`、`protocol`、`resource`、`configuration`、`compatibility`、`profiles`。SDK/Collector 实现不在本仓库。

## 4. 技术栈

跨语言规范；参考实现由各语言 SDK/Collector 提供。Hisha 将使用 Rust/Node OTel SDK + OTel Collector（Go）作为基础设施。

## 5. 数据模型

Trace/Span（trace_id、span_id、parent、attributes、events、links、status）；Metric（name、unit、attributes、aggregation、temporality）；LogRecord；Resource（service.name、deployment.environment 等）；Baggage；OTLP envelope。

## 6. 执行机制

Instrumentation 生成信号 → SDK processor/sampler/exporter → OTLP → Collector → backend。Context propagation 跨 HTTP/gRPC/消息队列；Collector 负责批处理/重试/路由。没有业务流程执行。

## 7. 扩展机制

- 自定义 instrumentation、processor、exporter、sampler。
- Collector receiver/processor/exporter/connector。
- 语义约定（semantic conventions）扩展属性命名；Hisha 应定义 `hisha.*` 命名空间并避免与通用约定冲突。

## 8. 插件开发方式

SDK 扩展用各语言接口；Collector 用 Go 组件。Hisha 的自定义 span/attribute 应在文档中注册，避免各模块随意命名。

## 9. API 设计

OTLP（gRPC/HTTP）；SDK API（Tracer/Meter/Logger）；环境变量配置规范。Hisha 的 API 不直接暴露 OTLP；由 Worker/Collector 上报。

## 10. 部署架构

Collector 可作为 sidecar、DaemonSet 或 gateway；后端可选 Prometheus/Grafana/Jaeger/Tempo/云厂商。Hisha Private/SaaS 部署应提供 Collector 配置样例与采样策略。

## 11. 前端交互设计

规范无 UI。Hisha Execution Monitor 可嵌入 trace/span 视图或链接到 Grafana/Tempo，不重复造可观测性 UI。

## 12. 许可证

根 `LICENSE` 为 Apache-2.0；规范文本与 schema 可复用，需保留版权/NOTICE；SDK/Collector 各自 Apache-2.0（需按实际版本核查）。无 copyleft。

## 13. 可直接复用

- OTel SDK（Rust/Node/Python）与 Collector（Apache-2.0）作为 Hisha 的唯一可观测性管道。
- 语义约定：HTTP/gRPC/DB/消息队列 span；Hisha 扩展 `hisha.tenant_id`、`hisha.workspace_id`、`hisha.execution_id`、`hisha.node_id`、`hisha.agent_session_id`、`hisha.skill_id`、`hisha.mcp_server_id`。
- Trace context 贯穿 Workflow → Agent → Skill → MCP → Connector；MCP 2026-07-28 文档化了 `_meta` 的 trace context。

## 14. 需要二次开发

- Hisha 语义约定与采样策略（租户/执行类型）。
- Collector 配置（本地/私有/SaaS；敏感字段脱敏）。
- 与审计日志的边界：Trace 不是审计，审计必须独立追加式存储。
- 成本控制：高基数属性/租户标签的 cardinality 限制。

## 15. 仅值得借鉴

- 信号分层（trace/metrics/logs/baggage）与 OTLP。
- Context propagation 跨进程/协议。
- 资源属性与语义约定治理。
- SDK 的批处理/采样/错误处理。

## 16. 不适合采用

- 把 OTel 当作审计日志或业务事件源。
- 无限制地把租户/用户 ID 作为 metric label（高基数）。
- 自研 tracing 协议/SDK。
- 把 Trace 数据长期全量存储而不做采样/保留策略。

## 17. 与 Hisha 的兼容性

高。Hisha 的所有运行时（Control Plane、Worker、Local Runtime、MCP Gateway）都应使用 OTel；ExecutionEvent 与 Trace 通过 `execution_id` 关联；Agent/Tool/Skill/MCP span 统一命名。OTel 是横向能力，不进入业务模型。

## 18. 引入后的维护成本

低-中：SDK/Collector 升级、语义约定演进、Collector 运维。比自研可观测性低一个数量级；SaaS 需控制成本/基数。

## 证据路径

`LICENSE`、`specification/overview.md`、`specification/trace/*`、`specification/metrics/*`、`specification/logs/*`、`specification/baggage/*`、`specification/context/*`、`specification/protocol/*`、`specification/resource/*`、`specification/configuration/*`、`specification/compatibility/*`、`specification/profiles/*`、`specification/schemas/*`、`CHANGELOG.md`。
