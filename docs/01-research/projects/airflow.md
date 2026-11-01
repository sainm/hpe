# Apache Airflow 3.3.2 研究报告

> 仓库：`apache/airflow` · tag `3.3.2`（2026-09-17）· tarball SHA-256 `14710da45ccd36cf...`<br>
> 研究方式：Python monorepo 源码（11,642 文件）+ Apache-2.0 + scheduler/task-sdk/API/HITL/UI 代码；未构建运行。<br>
> 结论：**Design Reference（调度、Provider、回填、任务执行 API）**；不嵌入。

## 1. 项目定位

以 Python DAG 为中心的数据/批处理编排与调度平台：DAG 是代码，调度器按时间/依赖创建 TaskInstance，Executor 执行，Provider 提供 700+ 外部系统集成。它面向数据工程，不是通用业务自动化或 Agent 平台。

## 2. 核心功能

- Python DAG、Operator/Sensor/Hook、Connections/Variables/XCom、Pools、优先级。
- Scheduler、Dag Processor、Triggerer、API Server、Worker；回填/补数据、重试、SLAs、审计。
- Providers：本快照约 74 个 `provider.yaml`；连接类型、Hook、Operator、Trigger。
- Airflow 3：Task SDK + Execution API（任务通过 API 与调度器通信）、DAG 版本/资产（Assets）、React 19 UI。
- HITL：`api_fastapi/.../hitl.py`、`core_api/datamodels/hitl.py`、UI 路由，支持人工任务。

## 3. 技术架构

- Python monorepo：`airflow-core`（调度、API、模型、执行器、UI）、`task-sdk`（任务运行时）、`providers/*`、`chart`（Helm）、`docker-stack`。
- Scheduler 解析 DAG、创建 DagRun/TaskInstance；Task SDK 通过 Execution API 拉取任务/汇报状态；Triggerer 处理 deferrable operators。
- API：FastAPI（`api_fastapi/core_api`、`execution_api`），OpenAPI。
- UI：React 19 + Chakra 3 + XYFlow + Monaco + i18next（`airflow-core/src/airflow/ui`）。
- 元数据库：Postgres/MySQL；Executor：Local 在 core，Celery/Kubernetes 通过 providers/部署组件。

## 4. 技术栈

Python、FastAPI、SQLAlchemy/Alembic、React 19、TypeScript、Chakra UI、XYFlow、Monaco、i18next、Celery/Kubernetes（可选）、Postgres/MySQL、Helm/K8s。Provider 是独立 Python 包。

## 5. 数据模型（从代码可见）

DAG/DagModel、DagRun、TaskInstance、TaskInstanceHistory、Connection、Variable、XCom、Pool、Asset/AssetEvent、HITLDetail、SerializedDag、DagBundle/DagVersion。DAG 由 Python 代码序列化，版本与运行绑定。

## 6. 执行机制

- Scheduler 解析/序列化 DAG，按时间表/依赖/资产触发 DagRun，生成 TaskInstance。
- Task SDK 执行任务并与 Execution API 通信；Executor 决定任务落在本地/Celery/K8s。
- 重试、超时、SLAs、Deferrable Operator（Triggerer）、回填。
- HITL 通过 API/DB 保存待处理任务，UI 人工响应后继续。
- 不是事件溯源重放；DAG 执行状态存元数据库，恢复能力依赖调度器/执行器。

## 7. 扩展机制

- Provider 包：Hook、Operator、Sensor、Trigger、Connection 类型、UI 插件。
- 自定义 Operator/Hook、Plugins Manager、Listener。
- Task SDK 允许独立任务运行时。
- Airflow 3 的 DAG Bundle 支持多来源 DAG。

## 8. 插件开发方式

编写 Python Provider 包，声明 `provider.yaml`、Hook/Operator/Trigger、连接类型；发布到 PyPI；安装到 Airflow 环境。与 Hisha 的 Rust/TS SDK 与插件签名体系不兼容。

## 9. API 设计

FastAPI REST（Core API + Execution API），OpenAPI；DAG/运行/任务/连接/变量/资产/HITL；UI 使用 OpenAPI 生成客户端。API 以 DAG/DagRun/TaskInstance 为中心。

## 10. 部署架构

Docker Compose（官方 quickstart）、Helm Chart（`chart/`）、Kubernetes；Postgres/MySQL；Celery/K8s Executor；多组件（scheduler/api-server/dag-processor/triggerer/workers）。运维复杂。

## 11. 前端交互设计

React 19 新 UI：DAG 列表/网格、运行详情、任务日志、甘特/图、资产、HITL 页面；Chakra + XYFlow。信息密度高，适合数据运维；不是业务人员工作台。

## 12. 许可证

根 `LICENSE` 为 Apache-2.0；`NOTICE`、各 provider 有各自 LICENSE/NOTICE（大多 Apache-2.0）。可商用/修改；需保留 NOTICE。仍需对 provider 依赖做 SBOM。

## 13. 可直接复用

- 设计：调度器/回填/DAG 版本、Provider 连接类型、Task SDK 与 Execution API 分离、HITL 数据模型、OpenAPI UI 客户端。
- 代码：不直接复用（Python 调度器/元数据库/Provider 与 Hisha 栈不一致）。
- 可作为“数据管道/批处理”类 Workflow 的参考后端，但不是 Hisha 核心。

## 14. 需要二次开发

若以 Airflow 为执行后端：需要把 Hisha IR 编译为 DAG/Operator、映射权限/审批/多租户、引入 Python 运行时、处理两套调度语义；成本高。

## 15. 仅值得借鉴

- Scheduler 与 Task SDK 的职责分离。
- Provider（连接类型 + Hook + Operator）的集成模型。
- 回填/补数据、资产触发、DAG 版本。
- HITL 的 API/UI 闭环。
- React 19 + OpenAPI 的现代 UI 迁移经验。

## 16. 不适合采用

- Python DAG 作为 Hisha Canonical IR。
- 以 Airflow 元数据库/调度器为 Hisha 核心（重、Python 中心）。
- Provider 生态直接搬入（数量/质量/许可/维护不可控）。
- 把 Airflow 当低延迟业务工作流引擎（调度粒度/延迟不匹配）。

## 17. 与 Hisha 的兼容性

低（代码），中（调度/Provider 概念）。Hisha 的 Connector Registry 可借鉴 Provider 的连接类型/测试/文档，但用 Rust/WASM + TS 管理 SDK 和统一能力描述重做；Workflow 的定时触发可借鉴回填/资产触发语义。

## 18. 引入后的维护成本

作为依赖：高（Python/多组件/K8s/Provider 版本矩阵）。作为参考：低。Hisha 不引入。

## 证据路径

`LICENSE`、`NOTICE`、`airflow-core/pyproject.toml`、`airflow-core/src/airflow/{scheduler,dag_processing,executors,api_fastapi,models,assets}/*`、`task-sdk/src/airflow/sdk/*`、`airflow-core/src/airflow/api_fastapi/execution_api/routes/hitl.py`、`airflow-core/src/airflow/ui/package.json`、`providers/*/provider.yaml`、`chart/Chart.yaml`、`docker-stack*`。
