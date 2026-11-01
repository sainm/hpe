# ADR-0015：跨语言契约与通信

- 状态：已接受
- 日期：2026-11-01
- 决策者：架构负责人（Phase 0 技术栈）

## 背景

Rust/TypeScript/Python 三方需要共享 IR、执行、工具、策略、事件；如果各语言自定义 JSON/对象，契约会漂移。需要明确哪种通道用于哪种场景，以及错误/超时/取消/重试/版本策略。

## 决策

- **内部服务**：gRPC + Protobuf（Tonic/tonic-prost；TS ts-proto + @grpc/grpc-js；Python grpcio-tools）。
- **外部 API**：REST/OpenAPI 3.1 + SSE；浏览器不直连 gRPC。
- **数据契约**：JSON Schema 2020-12（IR/Node/Tool/Plugin/Connector/Agent 配置）。
- **AI 工具**：MCP（Rust RMCP；2026-07-28 + 2025-11-25）；不作为内部服务总线。
- **异步**：Postgres outbox + Redis Streams（MVP）；NATS JetStream 后续；不引入 Kafka 初期。
- **错误**：统一 `ErrorCode` 枚举；gRPC/REST/MCP 映射；`retryable`/`remediation`/`traceId`。
- **超时/取消**：deadline + 取消传播 + 进程终止；不可中断调用记录“可能已发生”。
- **幂等/重试**：稳定 `idempotencyKey`；只重试幂等/明确可重试；至少一次，不承诺 Exactly Once。
- **认证**：边缘 OIDC/PAT；内部 mTLS + 短期 JWT；tenant 绑定；禁止 token passthrough。
- **版本**：Proto 包版本 + Buf breaking；JSON Schema `$id`；OpenAPI `/v1`；MCP 协议协商。

## 备选

- 全 REST/JSON：简单但流式/取消/强类型弱；高频内部调用开销大。
- 全 gRPC：浏览器不友好；外部 SDK 生态差。
- 共享内存/FFI：GIL/崩溃/部署复杂；禁止作为主集成。
- MCP 作为内部总线：协议/权限/流式不适合平台控制流。
- Kafka：初期运维/复杂度高。

## 后果

- 需要 Proto/schema 生成/漂移检查/契约测试。
- 两套 API（内部 gRPC/外部 REST）需要映射/文档；避免重复业务逻辑。
- Rust 是执行事实源；TS/Python 不直接写 Rust schema。
- 错误/超时/取消跨语言一致需要 fixture/测试。

## 证据

[跨语言通信](../13-tech-stack/cross-language.md)、[Rust 技术栈](../13-tech-stack/rust-stack.md)、[证据](../13-tech-stack/evidence.md)。
