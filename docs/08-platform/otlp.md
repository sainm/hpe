# OTLP 与跨进程追踪

> 状态：**P13-07 IN_PROGRESS**。Rust API/Runtime、HTTP client、Python LangGraph Worker 的 OTLP traces/metrics/logs、W3C `traceparent` 传播、有界 processors/readers、error/latency/probabilistic tail sampling、敏感属性删除和 Collector 部署/告警示例已实现并通过本地 OTLP Collector 测试；真实 HA Collector/Grafana 多节点压测仍需外部部署验证。

## 1. 启用

```bash
export HPA_OTEL_ENABLED=1
export HPA_OTEL_ENDPOINT=https://collector.internal:4318
export HPA_OTEL_SAMPLE_RATIO=0.1
export HPA_OTEL_QUEUE_SIZE=2048
export HPA_OTEL_BATCH_SIZE=256
export HPA_OTEL_METRIC_INTERVAL_SECONDS=5
export HPA_OTEL_HEADERS='authorization=Bearer collector-token'
hpa serve
```

- Endpoint 可以是 Collector origin 或任一完整 `/v1/traces|metrics|logs` URL。
- 可用 `HPA_OTEL_TRACES_ENDPOINT`、`HPA_OTEL_METRICS_ENDPOINT`、`HPA_OTEL_LOGS_ENDPOINT` 分信号覆盖。
- 未设置 `HPA_OTEL_ENABLED` 时保留 JSON tracing，不导出 OTLP。
- TraceContext propagator 支持 W3C `traceparent`/`tracestate`。
- Python Worker 读取同一 `HPA_OTEL_*` 环境并接收 Rust 注入的 `traceparent`。

## 2. Barrier/有界性

Rust 使用官方 `opentelemetry-otlp` + `BatchSpanProcessor`：

- 队列默认 2048，可夹取 64–65536；
- batch 默认 256；
- 1 秒 export delay、3 秒 exporter timeout；
- Collector 不可达时 span 丢弃并告警，不阻塞 API/Runtime；
- readiness 不依赖 Collector。

Python 使用 OTel SDK `BatchSpanProcessor`、`BatchLogRecordProcessor` 和 `PeriodicExportingMetricReader`，同样限制 queue/batch/interval 与导出超时。

## 3. Span 链

当前已接入：

```text
external traceparent
  -> HTTP server
       -> workflow.trigger
            -> workflow.execute
                 -> workflow.step
       -> outbound http.client
       -> mcp.request
       -> Rust agent dispatch / Python agent.session
            -> agent.plan
            -> agent.tool_call
                 -> tool HTTP traceparent injection
```

- Rust→Worker 控制请求通过 JSON `traceparent` 注入。
- HTTP Worker/MCP 出站请求通过 W3C headers 注入。
- Python session/model/tool spans 使用同一 traceId。
- execution/tool/session 可作为 **span attributes**；Prometheus labels 不加入 tenant/execution/job/trace ID，避免高基数。

## 4. Metrics

- Rust 现有 Prometheus counter/gauge/histogram 同步写入 OTel MeterProvider；同一标签集和显式 bucket 同时用于两条路径。
- Python 导出 `hpa_ai_worker_sessions_total`、`hpa_ai_worker_session_seconds`、`hpa_ai_worker_tool_calls_total` 和 `hpa_ai_worker_tokens_total`。
- Prometheus 保持低基数：route 使用匹配模板，node type/status 是枚举；tenant 全局 gauge 先跨租户聚合，不为每个 tenant 或 execution 生成 series。
- MetricReader 默认 5 秒；`HPA_OTEL_METRIC_INTERVAL_SECONDS` 夹取 1–60。

## 5. Logs

- Rust `tracing` 事件通过 `OpenTelemetryTracingBridge` 导出到 OTLP logs；原始 JSON/文本 stderr 保留。
- Python session 完成、失败和取消写入 structured log，携带 status、duration、tool calls 和 token 数。
- 不导出 request/response body、Authorization、tool secret、模型 API key 或 checkpoint 内容。
- collector 示例同时配置 OTLP logs→Loki、traces→Tempo、metrics→Prometheus endpoint。
- 详细部署和 dashboard 见 [deploy/otel](../../deploy/otel/README.md)。

## 6. Tail sampling 与属性

示例 Collector 对 traces 采用三项有限策略：

- `status_code=ERROR` 始终保留；
- 超过 5000ms 的慢 trace 保留；
- 其余 trace 以 10% 概率基线采样。

`decision_wait=10s`、`num_traces=100000`、`expected_new_traces_per_sec=100` 都是显式容量参数，不依赖无限内存。该 tail sampler 假设同一 trace 的 span 到达同一 Collector；多 Collector 生产部署必须在 tail sampling 前按 `traceID` 路由，不能用简单负载均衡把同一 trace 拆到不同实例。

`attributes/sanitize` 在 traces/metrics/logs 三条 pipeline 中删除 Authorization、Cookie、Set-Cookie、API key、bearer token、模型 key、SQL statement 和 checkpoint。允许进入后端的常规字段只包括 service/resource、HTTP method/route/status、node type/status、MCP method、模型/usage、错误类型和 trace/span ID。tenant/execution/tool/session ID 只能作为访问受控的 span/log 属性，不得进入 Prometheus label；请求体、响应体、prompt/completion、数据库参数和凭证不导出。

安全评审边界：

- 后端 Tempo/Loki/Grafana 必须启用租户访问控制、静态/传输加密和保留策略；
- Collector endpoint 必须使用 TLS/mTLS 或可信内网加网络策略；
- 属性删除是纵深防御，不替代应用侧“不采集 secret”；
- 新增属性必须先检查基数、敏感性和保留周期，并同步测试 `check:otel` 的删除清单。

## 7. 验证

```bash
cargo test -p hpa-local --test otel -- --nocapture
cargo test -p hpa-local --test otel_metrics_logs -- --nocapture
cd services/ai-worker && uv run --frozen pytest tests/test_telemetry.py -q
cd ../..
HPA_AI_WORKER_BIN="$PWD/services/ai-worker/.venv/bin/hpa-ai-worker" \
  cargo test -p hpa-local --test otel_cross_process -- --nocapture
pnpm check:otel
```

证据包括：

- 显式 traceparent 进入 API，Rust `http.server`/`workflow.execute`/`workflow.step` 保持同一 traceId；
- Rust `AiWorkerProcess` 从当前上下文注入 traceparent，Python `agent.session`/`agent.plan` 保持同一 traceId；
- Collector 停机时 health/readiness/普通 API 仍成功，队列不会无限增长；
- OTLP Collector 返回停止响应时 BatchSpanProcessor 不阻塞业务。
- Collector 同时收到 `/v1/metrics` 的 HTTP/workflow counters 与 `/v1/logs` 的 `execution finished`；
- Python Collector 同时收到 `/v1/traces`、`/v1/metrics`、`/v1/logs`，session metrics/log 使用低基数 status/direction。

## 8. SLO/Alerts

示例告警：

- HPA scrape target unavailable >2m；
- HTTP 5xx ratio >5% for 10m；
- Workflow execution p95 >300s for 15m；
- Collector scrape target unavailable。

还需要在 Collector 内配置：

- `otelcol_processor_batch_batch_send_size_count` / timeout；
- `otelcol_exporter_send_failed_spans`、queue size/drop；
- collector memory limiter refusal；
- Loki/Tempo/Prometheus storage 容量与保留；
- trace search latency / ingestion lag。

## 9. 尚未完成

- 当前未真实部署 Collector/Tempo/Loki/Grafana 容器并截图验证 dashboard 数据；配置文件已 pin 镜像但需外部环境演练。
- 多 Collector 下的 traceID 一致路由、tenant-aware sampling 和实际容量参数仍需在真实 HA 环境调优。
- 真实 Collector/Tempo/Loki/Grafana 容器部署、dashboard 截图、HA 故障切换和多节点压力/容量测试仍为 `BLOCKED_EXTERNAL` 或待基础设施。
