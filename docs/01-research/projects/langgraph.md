# LangGraph 1.2.19 研究报告

> 仓库：`langchain-ai/langgraph` · tag `1.2.19`（2026-10-30）· tarball SHA-256 `eafecec6be9a3719...`<br>
> 研究方式：Python monorepo 源码（701 文件）+ JS SDK 包元数据 + checkpoint/interrupt/durability 代码；未构建运行。<br>
> 结论：**Integration（Agent Runtime 内核）**，不作为 Workflow IR 或通用执行引擎。

## 1. 项目定位

低层、有状态的 Agent 编排库：用图（节点/边/共享状态）表达 Agent 循环、工具调用、分支、子图与多 Agent 协作；提供 checkpoint、interrupt/resume、流式事件。它是“库 + 服务 SDK”，不是可视化工作流产品。

## 2. 核心功能

- `StateGraph` / `Pregel`：超步（superstep）执行、channels 合并、条件边、子图。
- Checkpointer：内存、SQLite、Postgres；线程（thread）与 checkpoint tuple。
- `interrupt()` + `Command(resume=...)`：人工介入/暂停恢复。
- Durability：`sync` / `async` / `exit` 三种检查点写入策略（`libs/langgraph/langgraph/types.py`）。
- 工具调用、预构建 ReAct Agent、流式输出、运行级配置。
- JS 包 `@langchain/langgraph`（1.6.0，MIT）提供同类能力与 SDK。
- `langgraph-sdk`/CLI 提供 threads/runs 服务接口。

## 3. 技术架构

- Python：`libs/langgraph`（Pregel、channels、graph、prebuilt）、`libs/checkpoint`（base/memory/serde/store/cache）、`libs/checkpoint-postgres`、`checkpoint-sqlite`、`sdk-py`、`sdk-js`、`cli`。
- Pregel 执行器：`pregel/_algo.py`、`_loop.py`、`_executor.py`、`_retry.py`；按超步调度节点，通道聚合状态。
- Checkpoint：每步保存状态与待执行任务；`interrupt` 通过特殊通道/异常暂停。
- 不是独立服务器；生产部署通常由 LangGraph Platform（商业）或宿主应用提供 threads/runs/队列/持久化。

## 4. 技术栈

Python ≥3.10（pydantic、langchain-core、xxhash）；JS/TS 包依赖 `@langchain/core`、zod。存储后端 Postgres/SQLite/内存。没有前端编辑器。

## 5. 数据模型（从代码可见）

图定义（节点、边、状态 schema）；`Thread`、`Run`、`Checkpoint`（channel values、versions、pending writes）、`Interrupt`；Store（长期记忆）。checkpoint 是可序列化状态快照，不是业务执行事件日志。

## 6. 执行机制

- 超步执行：每步执行就绪节点，合并通道写入，保存 checkpoint，再进入下一步。
- 节点失败可重试（`_retry.py`），但长期定时器、跨进程任务分发、多租户队列不在库内。
- interrupt：节点内调用 `interrupt(value)` 抛出暂停；宿主持久化后，用 `Command(resume=...)` 从 checkpoint 恢复。
- durability `sync` 每步同步写、`async` 异步写、`exit` 仅在退出时写；选择影响崩溃恢复。
- 多 Agent：子图、supervisor/handoff 通过普通节点/边实现；没有强制的组织模型。

## 7. 扩展机制

- `add_node`、`add_edge`、`add_conditional_edges`、`compile(checkpointer=..., store=...)`。
- 自定义 checkpointer/store；自定义事件流；interrupt 的 response schema。
- 工具以普通函数/`@tool` 注册；模型通过 LangChain 适配。
- JS/Python SDK 可远程调用 LangGraph Server。

## 8. 插件开发方式

没有插件市场；扩展方式是编写节点函数、工具、checkpointer、store 与图。对 Hisha：应把图定义封装在 `AgentRuntime` 接口后，不让业务直接依赖 LangGraph 的图/状态类型。

## 9. API 设计

Python/JS 库 API；`langgraph-sdk` 提供 threads/runs/assistants 的 HTTP/SSE 客户端；LangGraph Server 的 API 不在 OSS 核心仓库。没有通用 Workflow REST API。

## 10. 部署架构

库嵌入宿主进程；checkpoint 存 Postgres/SQLite；生产高可用/队列/多租户由宿主或商业平台负责。Hisha 将把它嵌入 Agent Worker，并用自己的 Postgres/队列/租约层托管运行。

## 11. 前端交互设计

OSS 仓库没有可视化编辑器；LangGraph Studio/Platform 是独立产品。Hisha 的 Agent Designer 不能用它现成 UI，只能参考“图 + 状态 + 运行轨迹”的表达。

## 12. 许可证

根 `LICENSE` 与各子包均为 MIT；直接依赖（`@langchain/langgraph-checkpoint`、`@langchain/langgraph-sdk`、`@langchain/protocol`）为 MIT，`@standard-schema/spec` 为 MIT。无 copyleft。仍需对传递依赖做 SBOM。

## 13. 可直接复用

- `langgraph`（Python）作为可选 AI Worker 的 Agent 状态图内核：interrupt、checkpoint、流式事件。
- `langgraph-checkpoint-postgres`/`sqlite` 作为会话检查点存储（AI 独立 schema；需验证租户隔离）。
- 设计：durability 三档、超步状态合并、子图 handoff。

## 14. 需要二次开发

- 多租户隔离（thread/run 归属、RLS、配额）。
- 工具权限、审批、预算、模型网关、MCP/Skill/Workflow 工具的 Hisha 适配。
- 运行队列、租约、超时、取消、崩溃恢复（库只提供 checkpoint，不提供调度）。
- 事件模型映射到 Hisha `AgentSession`/`ToolCall`。
- 版本升级与图状态迁移。

## 15. 仅值得借鉴

- 超步 + 通道合并的状态模型。
- interrupt/resume 与 durability 分档。
- 子图/多 Agent handoff 的组合方式。
- checkpoint 与长期 Store 分离。

## 16. 不适合采用

- 把 LangGraph 图当作 Hisha Workflow IR（语义、可视化、节点版本、人工审批都不同）。
- 把 LangGraph 直接暴露为平台 API/数据模型。
- 依赖它解决多租户、调度、长任务和计费。

## 17. 与 Hisha 的兼容性

高：MIT、Python 实现、状态图/interrupt/checkpoint 与 Hisha 可选 AI Worker 需求吻合。边界清晰：只做 Agent 执行内核，不做 Workflow 引擎、不做平台控制面。Python AI Worker 通过 gRPC 与 Rust Core 集成；无 Python 时 Agent 禁用。

## 18. 引入后的维护成本

中：版本迭代快（1.x），需要锁定版本、契约测试、checkpoint schema 迁移；但比自研 Agent 状态机便宜。必须限制使用面，避免 LangChain 生态扩散到 Hisha 核心。

## 证据路径

`LICENSE`、`libs/langgraph/langgraph/types.py`（`Durability`）、`libs/langgraph/langgraph/pregel/_loop.py`、`_algo.py`、`_retry.py`、`libs/langgraph/langgraph/types.py`（`Interrupt`）、`libs/checkpoint/langgraph/checkpoint/base/*`、`libs/checkpoint-postgres/*`、`libs/sdk-py/*`、`libs/sdk-js/*`、`libs/langgraph/pyproject.toml`、npm `@langchain/langgraph@1.6.0` 元数据。
