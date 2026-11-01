# 技术风险与维护成本

> 以“可发布、可维护、可跨平台”为优先。Rust 是核心，但核心范围必须受控；任何“为了纯 Rust”增加的系统复杂度都要拒绝。

## 1. 风险表

| ID | 风险 | 可能性 | 影响 | 缓解 | 触发信号 |
| --- | --- | --- | --- | --- | --- |
| T-01 | Rust 开发速度低于 TS/Python，MVP 延期 | 高 | 高 | 只把执行/IR/沙箱放 Rust；管理/UI/AI 用 TS/Python；M0 切片 | 迭代周期持续超估 |
| T-02 | Rust 人才/评审不足，质量问题 | 中 | 高 | 代码规范/评审/CI；关键模块外部评审；文档 | 缺陷率/评审积压 |
| T-03 | MSRV 1.97（Wasmtime）/1.94（SQLx）限制环境 | 中 | 中 | 固定工具链；CI 1.99；升级矩阵 | 用户/CI 无法安装 |
| T-04 | 编译时间/CI 成本高 | 高 | 中 | sccache、workspace 分层、减少 features、缓存 | CI > 30min |
| T-05 | Tonic/gRPC/Proto 契约漂移 | 中 | 高 | Buf lint/breaking；生成代码；三方契约测试 | 生成 drift/字段不兼容 |
| T-06 | Rust/TS/Python 三端错误码/超时/取消不一致 | 中 | 高 | 统一 ErrorCode/RequestContext；跨语言 fixture | 同一操作行为不同 |
| T-07 | RMCP 许可过渡/2026 规范演进 | 中 | 高 | 保留 NOTICE；法务；版本锁定；conformance | 许可/规范变更 |
| T-08 | Wasmtime API/WASI 生态变化 | 中 | 中 | 锁 50.x；WASI p2；升级测试；WASM 非唯一 | 升级失败/插件不兼容 |
| T-09 | 跨平台沙箱弱（macOS/Windows） | 高 | 高 | 高风险容器；WASM；风险提示；企业强制 | 安全评审/逃逸测试 |
| T-10 | SQLx 编译期查询/离线缓存复杂 | 中 | 中 | `.sqlx` 缓存；CI DATABASE_URL；显式 SQL | 构建失败/迁移冲突 |
| T-11 | 双控制面（Rust local / NestJS cloud）语义漂移 | 中 | 高 | 共享 schema/OpenAPI；契约测试；功能矩阵 | 本地/云端同一 fixture 结果不同 |
| T-12 | TypeScript 7/Nest 12/Node 26 兼容风险 | 中 | 中 | 生产 TS 5.9.3；TS 7 独立 spike；锁版本 | 装饰器/emit/构建失败 |
| T-13 | Vite 8/Tailwind 4/PostCSS 生态变化 | 中 | 低 | 锁版本；官方迁移；视觉回归 | CSS 构建/样式异常 |
| T-14 | Monaco/XYFlow 大图性能 | 高 | 中 | 虚拟化/分组/降级；性能预算 | 500 节点卡顿/内存 |
| T-15 | LangGraph 快速迭代/checkpoint 迁移 | 高 | 中 | 锁版本；AI Worker 独立；契约；迁移测试 | 升级失败/状态不兼容 |
| T-16 | Python 3.13/依赖/镜像体积 | 中 | 中 | uv lock；容器；可选；不自动安装 | 镜像过大/安装失败 |
| T-17 | LiteLLM 版本/EE 边界 | 中 | 中 | MIT 核心；版本锁定；排除 EE；法务 | 功能误用/许可 |
| T-18 | Node/Nest 内存/长驻 | 低 | 中 | 只做管理；执行 Rust；水平扩展 | OOM/延迟 |
| T-19 | CSS 全局污染/主题漂移 | 中 | 中 | CSS 架构规则/token/stylelint/视觉回归 | 样式冲突/对比度 |
| T-20 | 供应链（crate/npm/pip）漏洞/恶意包 | 中 | 高 | 锁文件/SBOM/审计/签名/审核 | 漏洞/异常依赖 |
| T-21 | 许可污染（SUL/EE/AGPL/专有） | 中 | 高 | cargo-deny 等门禁；法务；禁止复制 | 受限路径/依赖 |
| T-22 | 工具链缺失（Rust/uv/pnpm）影响交付 | 高 | 中 | 文档/doctor/CI；用户安装；不全局安装 | 本机无法构建 |
| T-23 | 过度工程（同时 gRPC/MQ/WASM/Temporal） | 高 | 高 | MVP 只选必要；Phase 门禁；ADR | 依赖/复杂度膨胀 |
| T-24 | 本地运行时打包/签名/公证 | 中 | 中 | cargo-dist；CI 证书；演练；回滚 | 安装失败/被拦截 |

## 2. 依赖风险

| 依赖 | 风险 | 缓解 |
| --- | --- | --- |
| Wasmtime 50 | MSRV 1.97、API 迭代 | 锁 50.x；升级测试；WASI p2 |
| SQLx 0.9 | 编译期查询/DB 版本 | 离线缓存；固定 Postgres/SQLite 版本 |
| Tonic 0.14 | gRPC/HTTP2/代码生成 | Buf/契约测试；只内部 |
| RMCP 3.8 | 许可过渡/规范 | 法务/NOTICE；conformance |
| tracing-opentelemetry 0.34 | 与 otel 0.33 配对 | 锁定组合；升级检查 |
| TypeScript 5.9/7 | 工具链兼容 | 5.9 生产；7 spike |
| NestJS 12 | Node 26 生态 | 锁版本；回归 |
| Tailwind 4/PostCSS | 新引擎/插件差异 | 单一管线；视觉回归 |
| LangGraph 1.2 | 快速迭代 | 锁版本；AI Worker 隔离 |
| LiteLLM | 版本/EE | MIT 核心；排除 EE |
| Python 3.13 | 依赖/ABI | uv lock/容器 |
| MinIO（如用） | AGPL | 替换/法务/隔离 |

## 3. 维护成本（相对）

| 区域 | 成本 | 说明 |
| --- | --- | --- |
| Rust 核心 | 高 | 2-3× TS 开发；编译/安全/跨平台；长期稳定 |
| TS Control Plane/Web | 中高 | 生态更新快；UI/画布/编辑器复杂 |
| Python AI Worker | 中 | 可选；依赖/模型迭代快 |
| 基础设施 | 中高 | Postgres/Redis/S3/OTel/Temporal（后续） |
| 安全/许可 | 持续 | SBOM/审计/法务/供应链 |
| 跨平台发布 | 中高 | 签名/公证/安装器/CI |

## 4. 回退/终止条件

- Rust 核心进度严重落后 → 缩小范围（只做 Runtime/IR/沙箱），管理/编排暂用 TS（但执行仍在 Rust）。
- Tonic/gRPC 复杂度过高 → 内部改用 ConnectRPC/HTTP（仅限内部，不影响 MCP）。
- Wasmtime 插件生态不成熟 → 插件先用容器/外部服务，WASM 延后。
- PyTorch/LangGraph 不稳定 → AI Worker 降级为简单 Agent/禁用，确定性 Workflow 不受影响。
- 跨平台签名/公证不可行 → 先发布 tar/zip，企业用容器；不虚假承诺安装器。
- 任何“多引擎同时引入” → 停止，回到 ADR/Phase 门禁。

## 5. 相关文档

- [语言策略](language-strategy.md)
- [Rust 技术栈](rust-stack.md)
- [MVP](mvp-stack.md)
- [风险登记](../11-roadmap/risk-register.md)
- [许可证分析](../01-research/license-analysis.md)
