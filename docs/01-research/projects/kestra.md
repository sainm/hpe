# Kestra 2.0.7 研究报告

> 仓库：`kestra-io/kestra` · tag `v2.0.7`（2026-10-20）· tarball SHA-256 `6ff9a1a7cebdaa5c...`<br>
> 研究方式：源码 tarball（5,410 文件）+ Apache-2.0 + 执行器/调度器/队列/插件/MCP 代码；未构建运行。<br>
> 结论：**Design Reference；可选 Integration（Kestra 导入/适配器）**，不作为 Hisha 内核。

## 1. 项目定位

声明式（YAML）+ 可视化的工作流编排平台，面向数据工程、基础设施运维和事件驱动任务。核心卖点是“YAML 即流程”、数百个插件、调度与执行历史、以及内置 MCP Server 和 AI Copilot。它更像“数据/运维编排引擎”，不是通用 AI Agent 平台。

## 2. 核心功能

- Flow/Task/Trigger YAML；命名空间、输入/输出、变量、重试、超时、并发、SLA、错误处理、`finally`、子流程。
- 调度器（cron/日期）、Webhook、Flow 触发、MCP Tool 触发。
- 执行器/Worker/队列分离；执行历史、日志、任务状态、replay（从失败任务重跑）。
- 插件系统：Task/Trigger 作为 Maven 工件；插件目录、文档生成、自动安装。
- 内置 MCP Server（按 tenant 的 `default` server、Flow-as-Tool、OAuth/base 认证）。
- AI Copilot / Agent Thread（`core/ai/agent/*`）与 UI Copilot 组件。
- 企业能力（治理策略等）在 OSS 中解析但忽略。

## 3. 技术架构

- Java 25 + Micronaut + Gradle 多模块：`core`、`executor`、`worker`、`scheduler`、`queue`、`queue-jdbc`、`jdbc`/`jdbc-postgres`/`jdbc-mysql`/`jdbc-h2`、`webserver`、`cli`、`ui`、`runner-memory`。
- `DefaultExecutor`、`ExecutionCommandMessageHandler`、`WorkerTaskResultMessageHandler`、`WorkerLoop`、`TriggerSchedulingLoop` 构成执行控制。
- 队列抽象 `QueueInterface`，实现含 JDBC（`JdbcDispatchQueue`/`JdbcKeyedDispatchQueue`），可替换为其他后端。
- 执行状态与 TaskRun/TaskRunAttempt 持久化；`Replay` 命令从指定 taskRun 创建新执行。
- 表达式：Pebble（`core/utils/PebbleUtil`、`RunContext.render/renderTyped`）。
- UI：Vue 3 + `@vue-flow/core` + Monaco + `monaco-yaml` + vue-i18n + ECharts。

## 4. 技术栈

Java 25、Micronaut、Gradle、Jackson（含 YAML）、Pebble、JDBC、H2/MySQL/Postgres、Vue 3、Vite、Vue Flow、Monaco。插件是 Java/Kotlin JAR（Maven 坐标）。部署以 Docker/Helm 为主。

## 5. 数据模型（从代码可见）

`Flow`（tasks、errors、finally、afterExecution、triggers、variables、inputs、outputs、concurrency、SLA、policyRefs、revision）；`Execution`；`TaskRun`（attempts、state）；`AbstractTrigger`；插件元数据；KV Store；Secrets；Tenant/Namespace。流程以 YAML 源 + 解析模型存储，Revision 支持版本。

## 6. 执行机制

- 控制面：Scheduler 计算触发器下一次执行，创建 Execution；Executor 处理命令/事件，推进任务状态。
- 执行面：Worker 拉取任务，执行插件 Task；结果通过队列回传；支持并发限制、Worker Selector（按标签路由）。
- 可靠性与恢复：TaskRun attempt、retry、timeout、`allowFailure`、replay from task；没有 Temporal 式的确定性事件重放，replay 是新执行。
- 控制流：`If`、`Switch`、`Parallel`、`Sequential`、`Loop`、`LoopUntil`、`Dag`、`Subflow`、`Pause`、`WorkingDirectory` 等容器/任务。
- MCP：`webserver/.../mcp/McpToolService` 将 Flow 暴露为工具，按 tenant 隔离，含工具访问控制。

## 7. 扩展机制

- Task/Trigger 继承 `Task`/`AbstractTrigger`，用 `@Plugin`、`@PluginProperty` 描述 schema；打包为 Maven 插件。
- `PluginCatalogService`、`MavenPluginDownloader`、`PluginScanner` 负责发现/加载/安装；插件可来自本地目录或 Maven。
- Flow 级扩展：子流程、插件任务、变量/输出、KV、Secrets。
- MCP Server 是平台能力对外的扩展面。

## 8. 插件开发方式

Java/Kotlin 编写 Task 类，使用 Kestra 注解生成配置 schema 与文档，发布 Maven 工件；在 Kestra 配置中安装插件坐标。开发体验对 JVM 团队友好，但与 Hisha 的 Rust 本地运行时/TS 管理面不同。

## 9. API 设计

REST `/api/v1`（Micronaut controllers）、Webserver；Flow CRUD、执行、日志、回放、触发器、插件、KV、Secrets、MCP；OpenAPI/Swagger 由 Micronaut 生成。API 以 Flow/Execution/Namespace 为中心。

## 10. 部署架构

Dockerfile、Docker Compose、Helm Chart（`charts/kestra`、`charts/kestra-starter`）、Kubernetes；数据库可选 H2/Postgres/MySQL；队列 JDBC；Worker 可水平扩展；单机模式适合本地开发。

## 11. 前端交互设计

Flow 列表、YAML 编辑器 + 可视化拓扑（Vue Flow）、执行详情/日志/重试、插件文档、蓝本（Blueprints）、Copilot 侧栏。YAML 与画布并存，但画布不是唯一编辑源；插件表单由 schema 渲染。

## 12. 许可证

根 `LICENSE` 为 Apache-2.0。企业治理能力（如 policyRefs）在 OSS 中“parsed but ignored”；具体 EE 实现不在本仓库。插件是独立工件，必须逐个检查其许可证；不能假设整个生态都是 Apache-2.0。

## 13. 可直接复用

- 概念：YAML-first + 可视化、Flow-as-Tool 的 MCP 暴露、Worker Selector、插件目录/文档生成、Replay UX。
- 代码：不直接复用（JVM/Micronaut/插件 ABI 与 Hisha 不匹配）。
- 可选集成：实现 Kestra YAML → Hisha IR 的只读导入器和执行适配器，服务已有 Kestra 用户；不进入核心。

## 14. 需要二次开发

若采用 Kestra 作为内核，需要：IR 双向映射（Hisha → Kestra 会丢失 Hisha 语义）、插件桥接（TS → Java）、多租户/权限对齐、MCP 去重、AI/Agent 模型整合。成本高，且形成两套模型。

## 15. 仅值得借鉴

- YAML 与可视化共享一套语义的工程实践（但 Kestra 模型不是 Hisha IR）。
- 执行状态、TaskRun attempt、replay from task 的产品化。
- 插件 schema/文档自动生成。
- Flow-as-Tool、按 tenant 的 MCP Server、工具访问控制。
- Worker Selector 与队列抽象。

## 16. 不适合采用

- JVM 插件体系作为 Hisha 的扩展 ABI。
- Kestra Flow 模型作为 Canonical IR。
- 与 Hisha 重复的 AI Copilot/MCP 能力直接叠加。
- 把 Kestra 的 replay 当作 durable execution（它不是事件溯源重放）。

## 17. 与 Hisha 的兼容性

作为独立执行引擎可通过适配器集成：Hisha IR → Kestra Flow（编译）或 Kestra YAML → Hisha IR（导入）。但双向语义不等价，且本地运行时需要 JVM。Hisha 的核心执行引擎不应选它；PoC 只比较“作为可选后端”的成本。

## 18. 引入后的维护成本

作为内核：高（JVM、插件生态、双模型、版本升级、租户/权限差异）。作为可选适配器：中（导入器 + 映射测试）。作为设计参考：低。

## 证据路径

`LICENSE`、`core/src/main/java/io/kestra/core/models/flows/*`、`core/src/main/java/io/kestra/core/models/tasks/Task.java`、`core/src/main/java/io/kestra/core/models/executions/*`、`core/src/main/java/io/kestra/core/executor/command/Replay.java`、`executor/src/main/java/io/kestra/executor/*`、`worker/src/main/java/io/kestra/worker/*`、`scheduler/src/main/java/io/kestra/scheduler/*`、`queue/src/main/java/io/kestra/queue/*`、`queue-jdbc/src/main/java/io/kestra/queue/jdbc/*`、`core/src/main/java/io/kestra/core/plugins/*`、`core/src/main/java/io/kestra/plugin/core/flow/*`、`core/src/main/java/io/kestra/core/mcp/*`、`webserver/src/main/java/io/kestra/mcp/*`、`core/src/main/java/io/kestra/core/ai/agent/*`、`ui/package.json`、`charts/kestra/Chart.yaml`。
