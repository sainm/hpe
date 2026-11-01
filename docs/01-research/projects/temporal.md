# Temporal 1.32.1 研究报告

> 仓库：`temporalio/temporal` · tag `v1.32.1`（2026-10-08）· tarball SHA-256 `9a1766efcdc2acf0...`<br>
> 研究方式：Go server 源码（3,789 文件）+ MIT + history/persistence/schedules/Nexus/CHASM 代码；未构建运行。<br>
> 结论：**Integration（长期 durable execution 适配器）**，不作为 v1 本地默认引擎，也不作为 Workflow IR。

## 1. 项目定位

Durable Execution 平台：以事件溯源和工作流重放保证长时间运行的业务逻辑在进程/节点故障后继续。适合订单、支付、审批、长任务、跨系统 Saga。它不提供可视化流程设计器，Workflow 是代码（Go/Java/Python/TS/.NET/PHP/Ruby 等 SDK）。

## 2. 核心功能

- Workflow / Activity、Timers、Retries、Timeouts、Signals、Updates、Queries、Child Workflows、Continue-As-New。
- Schedules、Namespaces、Multi-cluster、Archival、Visibility、Nexus（跨命名空间/团队调用）。
- 事件历史、确定性重放、任务队列、Worker 轮询。
- 新版 CHASM 执行模型（`chasm/`）与 component/archetype。
- Temporal Cloud（商业）与自托管。

## 3. 技术架构

- Go server，服务：`service/frontend`（API/授权/路由）、`service/history`（事件历史/状态机）、`service/matching`（任务队列匹配）、`service/worker`（系统工作流/调度器/归档）。
- gRPC/proto（`api/`、`proto/`）；持久化抽象 `common/persistence`；后端 Cassandra/Postgres/MySQL/SQLite（`schema/`）；可见性 Elasticsearch/SQL；归档 S3/GCS。
- SDK 不在本仓库；Worker 由各语言 SDK 实现，轮询 task queue、执行 Workflow/Activity。
- UI（Temporal Web）为独立项目；本仓库无前端。

## 4. 技术栈

Go 1.26、gRPC/protobuf、Cassandra/Postgres/MySQL/SQLite、Elasticsearch（可选）、Kafka（可选）、Docker。SDK 支持 TypeScript/Python/Go/Java/.NET/PHP/Ruby。部署以 Docker/K8s/Helm（外部）为主。

## 5. 数据模型（从代码可见）

Namespace、WorkflowExecution、EventHistory、MutableState、WorkflowTask/ActivityTask、TaskQueue、VisibilityRecord、Schedule、ClusterMetadata、Archival。执行是事件追加式历史；状态由历史重放得到。多租户主要靠 Namespace。

## 6. 执行机制

- Worker 长轮询 task queue；Workflow Task 触发确定性代码重放；Activity 执行副作用并由历史记录结果。
- 失败由事件历史 + 重试策略恢复；Timers/Signals/Updates 持久化；支持长时间等待。
- 没有 DAG/可视化语义；分支/循环/并行是宿主语言构造；Hisha IR 必须编译为 Workflow 或由通用解释器执行。
- 对非确定性代码、直接 I/O、随机数/时间等有严格约束。
- Namespace 适合租户/环境隔离，但高基数租户会产生运维成本。

## 7. 扩展机制

- SDK 侧：Workflow/Activity 定义、Interceptor、DataConverter、Search Attributes、Nexus Service。
- Server 侧：持久化/可见性/归档插件、动态配置、任务队列分区。
- 没有“节点/插件市场”；扩展以代码和服务配置为主。

## 8. 插件开发方式

不是插件平台。Hisha 若采用，需要提供：
- `HishaWorkflow` 通用解释器 Workflow（读取 IR 快照）；
- 一组 Activity（节点执行、连接器调用、Agent 调用、审批等待）；
- Worker 注册与版本化；
- IR/节点版本到 Workflow 代码的兼容策略。

## 9. API 设计

gRPC 为主（frontend service），SDK 封装；CLI `temporal`/`tctl`；Visibility 查询；Schedules API；Nexus。没有面向业务的 REST/OpenAPI；Hisha 必须自建 Control Plane API 并映射到 Temporal。

## 10. 部署架构

- 开发/本地：Temporal CLI dev server（单机，SQLite）。
- 生产：Temporal server + Cassandra/Postgres + 可选 ES；K8s/Helm 由外部仓库提供；Temporal Cloud。
- 运维复杂度高：数据库 schema、历史大小、任务队列、归档、版本升级、命名空间配额。

## 11. 前端交互设计

无内置编辑器。Temporal Web 提供 Workflow 列表、事件历史、堆栈、查询；对业务用户不友好。Hisha 的 Workflow Designer 必须自建，Temporal 只做后端执行事实源。

## 12. 许可证

根 `LICENSE` 为 MIT（Temporal Technologies Inc. / Uber / Datadog 版权）。直接依赖 `@temporalio/client` 的 `@grpc/grpc-js` 为 Apache-2.0，其余 MIT。无 copyleft；仍需 SBOM 与 NOTICE 保留。Temporal Cloud 是商业服务，与本仓库许可无关。

## 13. 可直接复用

- Temporal Server + 各语言 SDK（MIT）作为 durable execution 后端。
- TypeScript SDK 的 Workflow/Activity/Worker 模型与 Hisha Worker 契约契合。
- Signal/Update 用于人工审批、外部事件；Timer 用于 SLA/超时；Child Workflow 用于子流程。
- Namespace 用于租户/环境隔离的候选。

## 14. 需要二次开发

- 通用 IR 解释器 Workflow（不能在 Workflow 内直接执行任意脚本）。
- Activity 与 Hisha Node/Connector/Agent/Skill 的桥接、幂等键、重试映射。
- 多租户命名空间策略、配额、可见性、审计。
- Workflow 版本兼容（IR schema/节点版本/代码版本三重版本）。
- 本地打包（CLI dev server）与离线行为。
- 与 Hisha 轻量运行时的双栈一致性测试。

## 15. 仅值得借鉴

- 事件溯源 + 确定性重放作为 durable 语义的黄金标准。
- Signal/Update/Query 的区分（命令 vs 状态读取）。
- Child Workflow/Continue-As-New 对长流程的处理。
- Schedules 与 Namespace 的运维模型。
- “Workflow 代码确定性、Activity 承担副作用”的边界。

## 16. 不适合采用

- 作为 v1 唯一运行时（本地体积/运维、IR 解释器工作量大）。
- 把 Temporal 的代码 Workflow 当作 Hisha 的 Canonical IR。
- 依赖 Namespace 解决所有多租户/计费问题。
- 在 Workflow 内直接执行不可信脚本或任意 HTTP。

## 17. 与 Hisha 的兼容性

高（作为可选 durable 后端），中（作为唯一后端）。Hisha 的 `RuntimeAdapter` 可在 `standard`（轻量、本地）与 `durable`（Temporal）之间选择；同一 IR 编译到两种执行契约。PoC C 只验证最小闭环（Workflow → Agent → Workflow + 审批 + Worker 离线恢复），不先引入完整 Temporal 集群。

## 18. 引入后的维护成本

高：服务端 + 数据库 + 可见性 + 版本升级 + 运维；但低于自研 durable execution。建议 Phase 3 以后再做，且由专门的基础设施负责人维护。若客户不需要长任务/强恢复，可长期只用轻量运行时。

## 证据路径

`LICENSE`、`go.mod`、`service/frontend/*`、`service/history/*`、`service/matching/*`、`service/worker/scheduler/*`、`common/persistence/*`、`common/persistence/visibility/*`、`schema/{cassandra,postgresql,mysql,sqlite}/*`、`api/*`、`proto/*`、`chasm/*`、`docker/config_template.yaml`、`cmd/*`。
