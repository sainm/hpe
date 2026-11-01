# ADR-0007：Local Runtime 采用 Rust 单二进制

- 状态：已废弃（由 ADR-0014 替代）
- 日期：2026-11-01（原 Phase 0；技术栈更新）
- 决策者：架构负责人

## 背景

本地运行必须无云、无外部数据库、跨平台、可离线；运行时需要与 IR/Worker 共享语义。早期方案选择 TypeScript/Node SEA + 本地 Web UI；技术栈约束现要求 Rust Core。

## 决策（替代方案）

- Local Runtime 用 **Rust** 实现；单二进制（`rust-embed` 嵌入 Web UI）+ SQLite（SQLx）+ Axum 本地 API + Wasmtime/进程沙箱。
- 本地控制面子集在 Rust；不依赖 NestJS/云。
- Python AI Worker/Node/Java 脚本为可选外部运行时，不打包进核心二进制。
- 跨平台：Linux 强沙箱；macOS/Windows 容器/受限进程；签名/公证/安装器。

## 历史说明

原 ADR-0007 选择 TypeScript/Node SEA；因新约束“Rust 为核心执行基础设施”被 ADR-0014 替代。旧方案不再有效，代码/文档不得按 Node SEA 实现。

## 后果

- 与 [ADR-0014](0014-rust-core-runtime.md)、[Local Runtime（Rust）](../13-tech-stack/local-runtime-rust.md) 一致。
- 需要 Rust 工具链/编译/签名/公证；本机未安装，未构建。
- 本地/云端控制面子集需契约测试。

## 证据

[ADR-0014](0014-rust-core-runtime.md)、[Local Runtime（Rust）](../13-tech-stack/local-runtime-rust.md)、[跨平台](../13-tech-stack/cross-platform.md)。
