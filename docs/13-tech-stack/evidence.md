# 技术栈证据

> 核查日期：2026-11-02。方法：crates.io API + 下载 `.crate` 源码；npm registry + 官方文档；PyPI/GitHub 源码与许可证；GitHub API 元数据。Rust/Cargo 1.99.1、Node 26.13、pnpm 12.16.1、Python 3.13 和 Podman 5.8.5 已完成本地编译/测试；Python Worker 为显式 `planner.kind = "python"` 的可选独立进程，不改变既有 deterministic/OpenAI planner。

## 1. 工具链证据

| 项 | 当前/目标 | 证据 | 结论 |
| --- | --- | --- | --- |
| Rust stable | 1.99.1（2026-10-16） | GitHub `rust-lang/rust` release | 目标工具链；项目 MSRV 取依赖最高值 1.97 |
| Cargo | 1.99.1 | `cargo --version` | 当前工作区可编译；MSRV 1.97 |
| Node.js | 全局 26.0.0；验证使用 26.13.0（2026-10-28） | `node -v`、`npx -p node@26.13.0 node -v`、nodejs.org/dist/index.json | 目标 Node 26 LTS（≥26.13） |
| TypeScript | 5.9.3（生产）/ 7.0.2（最新） | npm registry | 生产用 5.9.3；7.0.2 待 Nest/Vite/装饰器兼容验证 |
| pnpm | 12.16.1（经 npx 固定版本） | `npx -y -p pnpm@12.16.1 pnpm --version` | 当前门禁用固定 pnpm |
| Python | 3.13.18 | Homebrew `python3.13 --version` | LangGraph/LiteLLM Worker 已安装与测试 |
| uv | 0.12.18（本机固定） | `uv --version` | Python 依赖/虚拟环境/构建；清华镜像 |
| Podman | 5.8.5 rootless / AppleHV | `podman version`、container smoke | Worker 只读 rootfs/cgroup/network isolation 演练 |
| Go | 1.27.1 | `go version` | 国内 proxy 安装 cosign |
| cosign | 2.6.0 | `target/tools/cosign version` | Sigstore bundle provenance；二进制 SHA-256 `27b8044467aa832ef5e28840273a222db2d34903480ab46aa888d6351b3a7bd8` |
| Buf | 1.75.0（Apache-2.0） | GitHub release | Proto lint/breaking；代码生成由 tonic-prost-build |

## 2. Rust crates（核心）

| Crate | 版本 | License | MSRV | 更新 | 用途 | 结论 |
| --- | --- | --- | --- | --- | --- | --- |
| tokio | 1.54.1 | MIT | 1.85 | 2026-10-21 | Async runtime/process/time | 采用 |
| axum | 0.8.9 | MIT | 1.80 | 2026-04-14 | 本地/Worker HTTP API | 采用 |
| serde | 1.0.229 | MIT OR Apache-2.0 | 1.56 | 2026-07-18 | JSON/配置序列化 | 采用 |
| schemars | 1.2.3 | MIT | 1.74 | 2026-10-12 | Node/Tool JSON Schema | 评估过，未直接依赖 |
| jsonschema | 0.60.2 | MIT | 1.85 | 2026-10-26 | IR/工具 schema 校验 | 采用 |
| saphyr | 0.3.3 | MIT OR Apache-2.0 | 1.85 | 2026-10-27 | YAML 1.2/annotated span | 采用（YAML） |
| jaq-core / jaq-std / jaq-json | 3.1.2 / 3.0.4 / 2.0.3 | MIT | 1.69/1.70 | 2026-10-21 | jq 表达式 | 采用（表达式） |
| sqlx | 0.9.0 | MIT OR Apache-2.0 | 1.94 | 2026-05-21 | Postgres/SQLite 访问、迁移 | 采用 |
| tonic | 0.14.9 | MIT | 1.88 | 2026-10-29 | 内部 gRPC | 当前仅作为 OTel protobuf 测试的传递依赖 |
| tonic-prost / -build | 0.14.9 | MIT | 1.88 | 2026-10-29 | gRPC 代码生成 | 当前无内部 gRPC 服务 |
| tracing | 0.1.44 | MIT | 1.65 | 2025-12-18 | 日志/span | 采用 |
| opentelemetry | 0.33.1 | Apache-2.0 | 1.75 | 2026-10-09 | OTel API/SDK | 采用 |
| opentelemetry-otlp | 0.33.1 | Apache-2.0 | 1.75 | 2026-10-09 | OTLP 导出 | 采用 |
| tracing-opentelemetry | 0.34.0 | MIT | 1.75 | 2026-09-23 | tracing ↔ OTel（依赖 otel 0.33） | 采用 |
| wasmtime | 50.0.1 | Apache-2.0 WITH LLVM-exception | **1.97** | 2026-10-29 | WASM/组件/WASI | 评估过，未直接依赖 |
| wasmtime-wasi | 50.0.1 | Apache-2.0 WITH LLVM-exception | **1.97** | 2026-10-29 | WASI p2/p3 | 评估过，未直接依赖 |
| rmcp | 3.8.0 | Apache-2.0（MIT→Apache 过渡声明） | 1.88 | 2026-10-26 | MCP client/server | 评估过；当前使用项目自有 MCP 实现 |

### 依赖证据路径

- Tokio：`process/mod.rs`（`kill_on_drop`）、`time`、`sync`、`signal`。
- Axum：`Cargo.toml` features（json/http1/tokio/tracing）、Tower 中间件。
- SQLx：`sqlite-bundled` feature（`sqlx-sqlite/bundled`）；`runtime-tokio`、`postgres`、`migrate`、`macros`。
- OpenTelemetry：项目启用 trace/metrics/logs + http-proto + blocking reqwest；Rust 使用 tracing bridge，Python 使用 OTLP 三信号 exporter。
- Tonic：仅通过 `opentelemetry-proto` 的 protobuf 类型进入测试，不是 HPA RPC 传输。
- Wasmtime/RMCP：保留在技术选型证据表中，但当前 Cargo workspace 未依赖，不能视为已采用。

### 核心 crate 源码哈希

| Crate | `.crate` SHA-256 |
| --- | --- |
| tokio 1.54.1 | `b52fd3255258d26fac7264ab5a76b53c13367136dc011d3d6ad4f8ed070396a3` |
| axum 0.8.9 | `31b698c5f9a010f6573133b09e0de5408834d0c82f8d7475a89fc1867a71cd90` |
| serde 1.0.229 | `4148590afebada386688f18773da617792bf2ef03ffc1e4cbd2b1d45b023e0ba` |
| schemars 1.2.3 | `ab508826f74a77ca9d5aba6ff19b522583ee3eaf28a19384ff3d0e5835fadf6e` |
| jsonschema 0.60.2 | `a42a11b47492e6a2cc76e9c7bcc8077776cb31dac444e0795d7f61bcc6441c08` |
| sqlx 0.9.0 | `378620ccc25c62c89d8be1c819e76a88d59bdcc3304733330788948e619bfd71` |
| tonic 0.14.9 | `e2cdfc673aabd30fe8fee92e65837b644c37c5d47f0f97ae6d5e0f1237d15e1b` |
| tracing 0.1.44 | `63e71662fa4b2a2c3a26f570f037eb95bb1f85397f3cd8076caed2f026a6d100` |
| wasmtime 50.0.1 | `fdfacce893df54d319b29720e79edb001cd1b48faaf78fdb3b9c608eaeeea29f` |
| wasmtime-wasi 50.0.1 | `fdc6ef7dfa261eda94e66b547cb93cced2d25eb7ea19f6621a5dfc42cb39fcae` |
| rmcp 3.8.0 | `f8be15815186873ea063bf005011d16c356804973489e40c39116ae3a2878d1f` |

### 关键发现

1. **当前 MCP 2026-07-28 支持来自项目自有 Rust HTTP/stdio 实现**（discover、MRTR、resultType、subscriptions/listen、tasks、Streamable HTTP 子集、OAuth）；RMCP 3.8.0 只作为替代方案证据，不在 Cargo 依赖中。
2. **Wasmtime 50 的 MSRV 是 1.97**，SQLx 0.9 是 1.94；项目最低 Rust 取 1.97，实际用 1.99。
3. **tracing-opentelemetry 0.33 依赖 otel 0.32**；必须用 0.34 + otel 0.33，不能混用最新版。
4. `jsonschema` 默认启用 HTTP/文件解析；处理不可信 schema 时应关闭远程/文件解析，只允许本地注册 ref。
5. RMCP/官方 MCP 处于 MIT→Apache 过渡；crate 声明 Apache-2.0，但 LICENSE 含混合条款，需保留 NOTICE/法务复核。
6. Rust JSONata 实现不成熟（`jsonata-rs` 0.3.4，2025-02 后无更新；`jsonata` 0.0.0），因此表达式引擎改为 **jq/jaq** 子集（纯 Rust、Active、MIT），而非 JSONata。Saphyr 0.3.3 提供 YAML 1.2 + span，替代已 deprecated 的 serde_yaml。

## 3. Rust 替代方案

| 领域 | 选择 | 版本 | License | 替代 | 结论 |
| --- | --- | --- | --- | --- | --- |
| HTTP | axum | 0.8.9 | MIT | actix-web 4.18.1（MIT/Apache）、rocket | Axum 与 Tokio/Tower/tonic 同生态；Actix 成熟但运行时/中间件风格不同 |
| 校验 | jsonschema + validator | 0.60.2 / 0.21.0 | MIT | garde 0.23.0（MIT/Apache） | JSON 契约用 jsonschema；DTO 用 validator；garde 备选 |
| DB | sqlx | 0.9.0 | MIT/Apache | sea-orm 2.0.4（MIT/Apache）、diesel 2.3.14（MIT/Apache） | SQLx 异步/显式 SQL/双驱动；ORM 增加抽象与迁移耦合 |
| RPC | tonic | 0.14.9 | MIT | connectrpc 0.9.1（Apache-2.0）、jsonrpsee 0.26.1 | 内部服务用 gRPC/Tonic；Connect 适合 HTTP/浏览器友好场景，但生态较新 |
| Protobuf | prost | 0.14.4 | Apache-2.0 | 手写 JSON | prost 与 tonic 配套；不手写 wire format |
| OTel | opentelemetry + tracing-opentelemetry | 0.33.1 / 0.34.0 | Apache/MIT | 仅 tracing/log | OTel 标准/跨语言；注意版本配对 |
| WASM | wasmtime | 50.0.1 | Apache-2.0 WITH LLVM-exception | wasmer 7.5.0（MIT） | Wasmtime 标准化/WASI p2/p3/组件；Wasmer 备选但生态/标准对齐较弱 |
| MCP | rmcp | 3.8.0 | Apache-2.0（过渡） | rust-mcp-sdk 2.0.0（MIT）、mcp-sdk 0.0.3（停滞） | RMCP 官方/当前规范/传输/OAuth；自研不必要 |
| OpenAPI（本地 API） | utoipa | 6.0.0 | MIT/Apache | 手写 OpenAPI | 可选；本地 API 文档/TS 生成 |

## 4. TypeScript / CSS

已下载并解包 `react@19.3.1`、`vite@8.3.5`、`@nestjs/core@12.3.2`、`@xyflow/react@12.12.0`、`monaco-editor@0.57.0`、`@tanstack/react-query@5.105.0`、`zustand@5.0.16`、`zod@4.6.7`、`tailwindcss@4.3.3`、`postcss@8.5.32` 的 npm tarball，核对 `package.json` 的 version/license/engines/peerDependencies。React 19、Vite 8（Node ^20.19/≥22.12）、NestJS 12（Node ≥20）、XYFlow（React ≥17）、Monaco/Query/Zustand 的 peer 约束均满足目标 Node 26。

| 包 | 版本 | License | 结论 |
| --- | --- | --- | --- |
| Node.js | 26.13 LTS（npx 验证；全局 26.0） | — | 目标运行时 |
| TypeScript | 5.9.3 生产 / 7.0.2 评估 | Apache-2.0 | 5.9.3 稳定；7.0.2 需工具链验证 |
| React / React DOM | 19.3.1 | MIT | 采用 |
| Vite | 8.3.5 | MIT | engines Node ^20.19 或 ≥22.12 |
| @vitejs/plugin-react | 6.1.2 | MIT | 采用 |
| NestJS core/common/platform-express | 12.3.2 | MIT | Node ≥20；Control Plane |
| @nestjs/swagger | 12.0.5 | MIT | OpenAPI 生成 |
| Prisma / @prisma/client | 7.10.1 | Apache-2.0 | Control Plane DB（Node ^20.19/^22.12/≥24） |
| ts-proto / @grpc/grpc-js | 2.18.3 / 1.14.7 | ISC / Apache-2.0 | gRPC 类型/客户端 |
| @xyflow/react | 12.12.0 | MIT | peer React ≥17；Canvas |
| monaco-editor | 0.57.0 | MIT | 代码编辑器 |
| @monaco-editor/react | 4.7.0 | MIT | peer React 16.8-19 |
| TanStack Query | 5.105.0 | MIT | 服务端状态 |
| Zustand | 5.0.16 | MIT | 编辑器局部状态 |
| Zod | 4.6.7 | MIT | 表单/API DTO |
| react-hook-form / resolvers | 7.91.0 / 7.0.0 | MIT | 复杂表单 |
| openapi-typescript / openapi-fetch | 7.13.0 / 0.17.0 | MIT | OpenAPI 类型/客户端 |
| Orval | 8.45.0 | MIT | 可选生成 hooks |
| i18next / react-i18next | 26.5.12 / 17.1.9 | MIT | ja/en/zh |
| Tailwind CSS | 4.3.3 | MIT | CSS-first `@theme`/CSS variables |
| @tailwindcss/postcss | 4.3.3 | MIT | PostCSS 管线（或 @tailwindcss/vite） |
| PostCSS | 8.5.32 | MIT | CSS 处理 |
| Sass | 1.107.1 | MIT | 可选；不作为主题系统 |
| lightningcss | 1.33.0 | MPL-2.0 | 可选，不进核心（许可证/非必要） |
| radix-ui | 1.8.1 | MIT | 无样式无障碍 primitives |
| shadcn CLI | 4.23.0 | MIT | 代码分发（非运行时） |
| class-variance-authority | 0.7.1 | Apache-2.0 | 组件 variants |
| clsx / tailwind-merge | 2.1.1 / 3.7.0 | MIT | 类名组合 |
| lucide-react | 1.66.0 | ISC | 图标 |
| pnpm / turbo | 12.16.1 / 2.11.10 | MIT | 包管理/任务编排 |

官方文档核对：Tailwind v4 使用 `@theme`/CSS variables；Vite 内置 CSS Modules/PostCSS；React 19、NestJS 12、Vite 8 的版本/引擎以 registry 为准。

## 5. Python

已核对 PyPI 元数据，并下载 Pydantic 2.14.2 / FastAPI 0.162.0 的 sdist 检查 LICENSE；LangGraph/LiteLLM 使用前轮源码 tarball（`LICENSE` MIT / MIT+EE）；grpcio/protobuf 通过 PyPI 元数据与 grpcio-tools 依赖范围核对。

| 包 | 版本 | License | 用途 | 结论 |
| --- | --- | --- | --- | --- |
| Python | 3.13.18 | PSF | AI Worker | 已安装并通过测试 |
| LangGraph | 1.2.19 | MIT | Agent 状态图/checkpoint/interrupt | 采用（可选 Worker） |
| Pydantic | 2.14.2 | MIT | 请求/响应/配置模型 | 采用 |
| FastAPI | 0.162.0 | MIT | 可选健康/管理 HTTP | 仅辅，不承载核心 |
| LiteLLM | 1.106.6 | MIT 核心 + EE | 模型网关（独立服务） | 采用核心，排除 EE |
| grpcio / grpcio-tools | 1.85.1 | Apache-2.0 | gRPC 客户端/服务端/生成 | 采用 |
| protobuf | ≥7.35.1,<8 | BSD-3-Clause | Proto runtime | 由 grpcio-tools 约束 |
| openai | 3.42.0 | Apache-2.0 | Provider SDK（适配器内） | 可选 |
| anthropic | 2.3.0 | Apache-2.0 | Provider SDK（适配器内） | 可选 |
| langchain-core | 1.7.3 | MIT | LangGraph 依赖 | 随 LangGraph |
| langgraph-checkpoint-postgres / sqlite | 3.2.0 / 3.1.3 | MIT | Agent 检查点 | 采用（Worker 私有 schema） |
| psycopg / psycopg-pool | 3.3.6 / 3.3.3 | LGPL-3.0 | PostgreSQL 异步 checkpoint 连接/池 | 采用（仅 Worker 私有 schema） |
| uv | 0.13.6 | Apache-2.0 | 依赖/环境/构建 | 采用 |
| pytest | 9.2.0 | MIT | 测试 | 采用 |
| ruff | 0.17.3 | MIT | lint/format | 采用 |
| mypy | 2.4.1 | MIT | 类型检查 | 采用 |

> PyPI 的 `license` 字段常为空；以上许可通过源码/仓库 LICENSE 与已知 SPDX 核对，发布前仍需 SBOM/传递依赖扫描。

## 6. 未验证/未做

- Rust/Cargo/pnpm/uv/Python 3.13 已安装并完成本地全量测试。
- 未在 Windows/macOS/Linux 三平台构建/打包；跨平台结论是设计+官方支持信息。
- 未做性能基准；文档中的性能/资源数字是目标，不是实测。
- 未做全量传递依赖许可证扫描；只核查核心直接依赖与源码许可证。
- Mermaid/CSS 未在浏览器渲染验证。

## 7. 复现

```bash
# Rust crate 元数据
curl -H "User-Agent: hpa-research" https://crates.io/api/v1/crates/rmcp | jq '.crate.max_stable_version, .versions[0].license'

# 下载源码并核对许可证/MSRV
curl -L https://crates.io/api/v1/crates/rmcp/3.8.0/download -o rmcp-3.8.0.crate
mkdir rmcp && tar -xzf rmcp-3.8.0.crate -C rmcp --strip-components=1
grep -nE '^(name|version|rust-version|license)' rmcp/Cargo.toml
grep -RIn 'V_2026_07_28\|server/discover' rmcp/src | head

# TypeScript/CSS
npm view react version license
npm view tailwindcss version license

# Python
curl https://pypi.org/pypi/langgraph/json | jq '.info.version'
```
