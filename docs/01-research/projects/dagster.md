# Dagster 1.13.28 研究报告

> 仓库：`dagster-io/dagster` · tag `1.13.28`（2026-10-30）· tarball SHA-256 `75446eb942e5ee80...`<br>
> 研究方式：Python/TS monorepo 源码（14,574 文件）+ Apache-2.0 + asset/partition/daemon/GraphQL 代码；未构建运行。<br>
> 结论：**Design Reference（资产图谱、分区、血缘、数据质量）**；不嵌入。

## 1. 项目定位

以“软件定义资产（Software-Defined Assets）”为中心的数据编排平台：声明资产及其依赖，Dagster 负责物化、调度、分区、回填、血缘和质量检查。它面向数据平台团队，不是通用业务工作流或 Agent 平台。

## 2. 核心功能

- Assets、Jobs、Ops、Graphs、Resources、IO Managers、Config、Partitions。
- Schedules、Sensors、Auto-materialize、Backfills、Run coordinator/launcher/executor。
- 资产血缘/目录、Checks、Asset observations、事件日志。
- Dagster Pipes（外部进程协议）、gRPC code server、GraphQL API。
- Web UI（资产图、运行、时间线、分区矩阵）、Helm/K8s。

## 3. 技术架构

- Python：`python_modules/dagster`（core/definitions/executor/daemon/scheduler/storage/partitions/assets）、`dagster-graphql`、`dagster-webserver`、`dagster-pipes`、`dagit`。
- 存储：Postgres/MySQL/SQLite；事件日志/运行存储/调度存储；实例（Instance）配置。
- 执行：Run coordinator + launcher + executor；step 执行；gRPC code server 加载用户代码。
- UI：React/TypeScript `js_modules/dagster-ui`；GraphQL 客户端。
- 部署：Helm Chart（`helm/dagster`）、Docker、K8s。

## 4. 技术栈

Python、GraphQL（Graphene）、React/TypeScript、Postgres/MySQL/SQLite、gRPC、Helm/K8s。没有 Java/Go 核心。

## 5. 数据模型（从代码可见）

AssetDefinition/AssetKey、AssetMaterialization、Run、Job、Op、Step、Partition/PartitionSet、Schedule/Sensor/InstigatorTick、EventLogRecord、RunCoordinator/Launcher/Executor、Resource/IOManager。资产与分区是核心，运行事件是追加式日志。

## 6. 执行机制

- 调度器/传感器/自动物化产生 Run；Run coordinator 管理并发；Launcher 启动；Executor 执行 step。
- 资产物化写事件日志/IO Manager；分区支持按区间/多维物化与回填。
- 失败重试、step 级重执行；没有 Temporal 式确定性重放，但事件日志支持恢复/审计。
- 支持外部进程（Pipes）与多代码位置；不是通用流程控制（循环/审批/人工任务）平台。

## 7. 扩展机制

- 资产/Op/Resource/IO Manager/Sensor/Schedule 的 Python 装饰器与类。
- Dagster Pipes 协议用于外部进程上报。
- GraphQL API 与 UI 插件；自定义 run launcher/executor/storage。
- 没有独立插件市场/签名体系。

## 8. 插件开发方式

编写 Python 包，定义资产/资源/IO Manager/Sensor；通过 `Definitions` 注册；部署为 code location（gRPC server）。与 Hisha 的 Rust Core/TS SDK 不兼容。

## 9. API 设计

GraphQL 为主（`dagster-graphql`），Web UI 使用；CLI；Pipes 协议；没有全面 REST/OpenAPI。API 以资产/运行/事件为中心。

## 10. 部署架构

Helm（`helm/dagster`）、Docker、K8s；Postgres/MySQL；code server 与 webserver/daemon 分离；可扩展 run launcher/executor。运维复杂度中等偏高。

## 11. 前端交互设计

资产图（DAG）、资产详情/分区矩阵/物化历史、运行时间线、日志、调度/传感器、回填。信息密度高，数据平台风格；对业务用户不友好。

## 12. 许可证

根 `LICENSE` 为 Apache-2.0；各 Python 子包有 LICENSE；`python_modules/dagster-cloud` 是商业/云代码（需单独核查，不应默认复用）。可商用/修改；需保留 NOTICE。

## 13. 可直接复用

- 设计：资产图谱/血缘、分区与回填、事件日志、Checks、Pipes。
- 代码：不直接复用（Python/GraphQL/资产模型与 Hisha 不同）。
- 可借鉴“能力/资产血缘视图”的数据模型，用于 Hisha Capability Registry 的关系图。

## 14. 需要二次开发

若以 Dagster 为后端：需要资产模型 → Hisha IR 映射、权限/审批/多租户、业务工作流控制流（循环/人工）；成本高，且产品定位不同。

## 15. 仅值得借鉴

- 资产/分区的数据模型与物化语义。
- 事件日志 + 运行/step 状态。
- 血缘图与影响分析 UX。
- Checks/数据质量。
- Pipes 的外部进程协议。

## 16. 不适合采用

- 资产中心模型作为 Hisha 通用 Workflow IR（Hisha 以控制流/任务为中心）。
- Python/GraphQL 技术栈。
- 商业 dagster-cloud 代码。
- 把数据资产编排与业务流程混为一谈。

## 17. 与 Hisha 的兼容性

低（代码/模型），中（血缘/分区/质量概念）。Hisha 的 Capability Registry 可借用“节点-边-版本-依赖”视图；Workflow 的定时/事件触发可参考传感器/调度模型。

## 18. 引入后的维护成本

作为依赖：高（Python/GraphQL/资产模型/Helm）。作为参考：低。Hisha 不引入。

## 证据路径

`LICENSE`、`python_modules/dagster/dagster/_core/{definitions,executor,daemon,scheduler,storage}/*`、`python_modules/dagster/dagster/_core/definitions/{assets,partitions}/*`、`python_modules/dagster-graphql/dagster_graphql/*`、`python_modules/dagster-webserver/*`、`python_modules/dagster-pipes/*`、`js_modules/dagster-ui/*`、`helm/dagster/Chart.yaml`、`python_modules/dagster-cloud/*`。
