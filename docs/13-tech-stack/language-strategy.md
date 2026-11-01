# 开发语言策略

> 结论：**Rust 核心执行基础设施 + TypeScript Control Plane/Web + Python 可选 AI Worker**。三种语言只在契约边界交互，不共享内部对象。

## 1. 最终组合

| 语言 | 定位 | 必须性 | 主要产物 |
| --- | --- | --- | --- |
| Rust（1.99.x，MSRV 1.97） | Core Runtime / Execution Plane | 必须 | `hpa-runtime`、`hpa-local`、`hpa-worker`、IR/编译器/校验器、Skill/MCP/Plugin/Script/Process/Resource/Policy |
| TypeScript（5.9.3，Node 26 LTS） | Control Plane / Web UI / SDK | 必须 | NestJS API、React Designer、Skill/MCP/Plugin Center、Capability Registry、TS SDK/Connector SDK |
| Python（3.13，可选） | AI Worker（LangGraph） | 可选 | Agent 图执行、模型适配、Python Skill 脚本、AI 工具编排 |
| JSON Schema / Protobuf / OpenAPI | 跨语言契约 | 必须 | IR/Node/Tool schema、gRPC 服务、外部 REST API |
| SQL / YAML / CSS | 数据/配置/样式 | 必须 | 模型存储、Workflow 文本、Design Tokens |

## 2. 选择理由

### Rust

- 执行平面需要长期运行、低内存、可预测延迟、强隔离与资源回收；Rust 无 GC、单二进制、跨平台。
- Workflow IR/编译器/校验器/运行时是产品核心，放在 Rust 可同时服务 Local Runtime、Worker、未来边缘节点。
- MCP 官方 Rust SDK（RMCP 3.8.0）已支持 2026-07-28，比 TypeScript SDK 更贴近当前规范；MCP 协议实现放在 Rust 是合理选择。
- Wasmtime 提供 WASI p2/p3、资源限制与组件模型，适合不可信插件。
- 代价：开发速度/人才/编译时间；用模块边界、契约测试、MVP 范围控制，不把所有业务逻辑塞进 Rust。

### TypeScript

- Web 设计器/Agent/Skill/MCP/Plugin UI 需要 React 生态、XYFlow、Monaco、Tailwind/Radix；TypeScript 是事实上的前端标准。
- Control Plane 是管理/鉴权/版本/注册表/编排；NestJS 的模块化、DI、OpenAPI 支持适合 Modular Monolith。
- 前后端共享生成的类型/JSON Schema/OpenAPI，减少契约漂移。
- 代价：Node 内存/长驻进程不如 Rust；把执行/沙箱/资源密集任务放 Rust。

### Python

- LangGraph/模型 SDK/数据处理生态在 Python 更成熟；Agent 规划/工具调用需要快速迭代。
- 只作为可选 AI Worker，通过 gRPC 与 Rust 交互；没有 Python 时确定性 Workflow/Skill 仍可用。
- 不允许 LangGraph/Pydantic 类型进入平台核心数据模型；所有跨边界数据用 Proto/JSON Schema。

## 3. 模块职责

### Rust

| 模块 | 职责 | 不做什么 |
| --- | --- | --- |
| `hpa-ir` | Canonical IR 类型/序列化/哈希 | 不做 UI/权限 |
| `hpa-schema` | JSON Schema 加载/校验/注册 | 不解析 YAML 到任意对象 |
| `hpa-yaml` | YAML 1.2 解析/发射/SourceMap（Saphyr） | 不做语义校验 |
| `hpa-expression` | jq/jaq 子集、模板、Context | 不做 I/O/环境/时间访问 |
| `hpa-compiler` | IR → 执行计划/端口/作用域/表达式 AST | 不执行节点 |
| `hpa-validator` | 五类校验/静态分析/依赖解析 | 不做发布审批 UI |
| `hpa-runtime` | 确定性执行器/检查点/事件/恢复/审批暂停 | 不执行不可信脚本 |
| `hpa-node-api` | Node trait/上下文/结果/错误 | 不依赖具体连接器 |
| `hpa-connectors` | HTTP/DB/File/Git/MQ 等执行 | 不存明文凭证 |
| `hpa-skill` | Loader/Resolver/Context/Script/Artifacts | 不把 Skill 当可执行程序 |
| `hpa-mcp` | MCP Client/Server（RMCP） | 不暴露内部 REST |
| `hpa-plugin` | manifest/签名/安装/加载/WASM 实例 | 不在 API 进程加载插件 |
| `hpa-script` | Native/WASM/外部执行统一接口 | 不绕过沙箱 |
| `hpa-process` | 子进程监督/进程树终止/回收 | 不做业务调度 |
| `hpa-resource` | CPU/内存/时长/配额计量与限制 | 不做权限 |
| `hpa-sandbox` | 文件/网络/进程/资源限制 | 不信任用户声明 |
| `hpa-policy` | 权限/审批/配额/安全策略执行（Security & Permission Enforcement；PDP 客户端 + 缓存） | 不做身份存储 |
| `hpa-worker` | 注册/心跳/租约/拉取/事件上报 | 不接受公网入站 |
| `hpa-local` | 本地二进制/本地 API/嵌入式 UI/SQLite | 不依赖云 |
| `hpa-observe` | tracing/OTel/指标 | 不存审计为唯一来源 |
| `hpa-proto` | Proto 生成代码/gRPC 服务 | 不手写 wire 格式 |

### TypeScript

| 模块 | 职责 | 不做什么 |
| --- | --- | --- |
| `apps/control-plane` | NestJS：认证/租户/工作区/项目/角色、Workflow/Agent/Skill/MCP/Plugin 元数据、版本、发布记录、审批、审计查询、计费/配额 | 不执行 Workflow/脚本/沙箱 |
| `apps/web` | React SPA：Designer/Agent/Skill/MCP/Plugin/Registry/Execution/Approval/Settings | 不直接连 Rust 内部 gRPC |
| `packages/ts/ui` | Design System/组件/样式/tokens | 不包含业务规则 |
| `packages/ts/ir-types` | 从 JSON Schema 生成的 TS 类型 | 不手写重复 DTO |
| `packages/ts/api-client` | 从 OpenAPI 生成的 typed client | 不手写 URL/字段 |
| `packages/ts/sdk` | 外部 TypeScript SDK | 不暴露内部管理 API |
| `packages/ts/connector-sdk` | 声明式 Connector/节点 schema/表单/校验；可选 Node 沙箱 handler | 不把 TS 代码加载进 Rust 进程/不绕过 Policy |
| `packages/ts/plugin-sdk` | 插件 manifest/构建/签名辅助 | 不绕过 Rust 插件宿主 |
| `packages/ts/i18n` | ja/en/zh 资源/类型/校验 | 不允许硬编码文案 |

### Python

| 模块 | 职责 | 不做什么 |
| --- | --- | --- |
| `python/ai-worker` | LangGraph 图执行、模型调用、工具选择、checkpoint、interrupt | 不写平台执行库/不直接管租户 |
| `python/ai-worker/tools` | 通过 gRPC 调用 Rust Tool/MCP/Workflow/Skill | 不绕过权限/审批 |
| `python/sdk` | Python SDK（未来） | 不复制 Rust 类型 |
| `python/skills` | 示例 Skill/脚本 | 不依赖平台私有格式 |
| `python/ai-worker/http` | 可选 FastAPI 健康/调试 | 不作为主 RPC |

## 4. 职责边界

### 4.1 Rust 是唯一执行事实源

- Workflow/Agent/MCP/Skill/Plugin 的**执行**在 Rust；Control Plane 只编排/管理。
- Rust 生成 `ExecutionEvent`，通过 gRPC/SSE 上报；Control Plane 不直接写执行表（或只读索引，见数据模型）。
- Local Runtime 是 Rust 的完整本地实现；没有 NestJS/Python 也能运行确定性 Workflow/Skill。

### 4.2 TypeScript 是唯一管理入口

- 用户/外部 API 的认证/租户/权限在 NestJS；Rust 校验签名/租户上下文，不重复实现 OIDC。
- Workflow/Agent/Skill/MCP/Plugin 的元数据/版本/发布审批在 NestJS；Rust 接收已冻结版本/执行计划。
- 外部 MCP 客户端由 Rust Gateway 处理；NestJS 管理 Gateway 配置/工具绑定/审计查询。

### 4.3 Python 是可选 AI Worker

- Agent 节点调用 Python AI Worker；没有 Python 时 Agent 功能禁用，Workflow 其他节点不受影响。
- AI Worker 不直接访问 Control Plane DB；只通过 gRPC 获取任务/工具/模型配置（短期凭证）。
- LangGraph checkpoint 存 AI 独立 schema/本地 SQLite；Rust 存执行事件；两者通过 sessionId 关联。

### 4.4 本地控制面子集

Local Runtime 需要独立运行，因此 Rust 实现一个**本地控制面子集**（Workflow/Skill/MCP/Agent 配置/版本/执行/审批/设置）。这不是复制 NestJS 全部功能：

- 本地：单用户/本地工作区、SQLite、无 SaaS 计费/多租户/复杂 RBAC。
- 云端：NestJS 多租户/SSO/配额/市场/审计。
- 两者共享 OpenAPI/JSON Schema/IR 契约；本地 API 必须通过同一套契约测试。
- 避免“本地一套云一套”的语义漂移：核心编译/校验/执行全部在 Rust，本地/云端只是 API/存储不同。

## 5. 不做什么

- 不用 Rust 重写 Temporal/LangGraph/LiteLLM/OTel；这些是集成组件。
- 不让 Python 成为核心运行时或唯一 Agent 引擎。
- 不让 TypeScript 执行 Workflow/脚本/插件；不做 Node 沙箱作为主执行路径。
- 不在语言之间共享内存对象/内部序列化；只用 Proto/JSON Schema/OpenAPI。
- 不把 Workflow IR 绑定到 Rust 类型；IR 是语言无关 JSON Schema。
- 不为“纯 Rust”而重复实现成熟库（HTTP、gRPC、WASM、MCP、OTel）。

## 6. 关键权衡

| 决策 | 收益 | 代价 | 缓解 |
| --- | --- | --- | --- |
| Rust 核心 | 性能/单二进制/沙箱/跨平台 | 开发慢、人才少、编译慢 | 模块化、MVP 切片、契约测试、sccache |
| TS Control Plane | 生态/开发速度/OpenAPI | Node 内存/长驻 | 只做管理，执行下沉 Rust |
| Python AI Worker | AI 生态/快速迭代 | 额外进程/依赖/ABI | 可选、gRPC、容器化、无核心 DB 访问 |
| gRPC 内部 | 强类型/流式/跨语言 | 代码生成/调试复杂 | Buf/tonic-prost/契约测试 |
| WASM 插件 | 安全/跨语言/可移植 | 工具链/生态不成熟 | WASI p2、MVP 后启用、Native 备选 |
| 双控制面（Rust local/TS cloud） | 本地独立/云端治理 | 子集重复 | 共享 schema/API、契约测试、功能矩阵 |

## 7. 相关文档

- [Rust 技术栈](rust-stack.md)
- [TypeScript 技术栈](typescript-stack.md)
- [Python 技术栈](python-stack.md)
- [跨语言通信](cross-language.md)
- [Script/Plugin 执行](script-plugin-execution.md)
- [Local Runtime](local-runtime-rust.md)
- [CSS 架构](css-architecture.md)
