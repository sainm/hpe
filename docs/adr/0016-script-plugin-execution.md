# ADR-0016：Script 与 Plugin 执行

- 状态：已接受
- 日期：2026-11-01
- 决策者：架构负责人（Phase 0 技术栈）

## 背景

用户脚本/插件需要支持 Python/JS/TS/Shell/WASM，未来 Java；但不可信代码不能进入 Rust 进程。需要统一的 Executor 接口、隔离模式、资源/权限/审计与跨平台策略。

## 决策

- 统一 `Executor` trait（Rust）：execute/cancel/capabilities；请求含 runtime/entry/args/env/mounts/network/resources/timeout/secrets/trace/idempotency。
- 执行模式：
  1. Built-in Rust（受信）
  2. WASM（Wasmtime 50 + WASI p2/p3；**第三方首选**）
  3. Native Process（Python/Node/Shell/Java；OS 沙箱/容器）
  4. External Service（HTTP/gRPC）
  5. MCP Server（RMCP）
- 不在 Rust 进程内 `dlopen` 不可信 native 插件；不提供私有 native ABI。
- 安全：文件/网络/命令/环境/CPU/内存/超时/进程树/临时目录/审计；默认无网络/只读输入/临时写。
- 平台：Linux Landlock/seccomp/cgroups；macOS 容器优先；Windows Job Objects/AppContainer；WASM 跨平台。
- 插件 manifest/signature/依赖/权限/版本/审核；高风险默认容器/WASM。
- Python/Node/Java 运行时由用户/镜像提供，不自动安装。

## 备选

- 所有插件用 native 动态库：性能好但崩溃/安全/跨平台不可接受。
- 只用 WASM：安全但生态/复杂 SDK 限制；保留 native/容器。
- 所有脚本在 Rust 内嵌解释器：依赖/GIL/安全/体积；不选。
- 外部服务化所有插件：延迟/部署复杂；仅适合长驻。

## 后果

- 需要维护多执行器/沙箱/平台差异；WASM ABI/工具链成本。
- 高风险插件在 macOS/Windows 需容器；用户体验/依赖。
- 统一接口/测试/审计可复用；插件生态可控。
- 与 Plugin/Skill 文档的权限/签名/市场规则一致。

## 证据

[Script/Plugin 执行](../13-tech-stack/script-plugin-execution.md)、[插件架构](../07-plugin/plugin-architecture.md)、[证据](../13-tech-stack/evidence.md)。
