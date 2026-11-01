# Flowable 8.0.0 研究报告

> 仓库：`flowable/flowable-engine` · tag `flowable-8.0.0`（2026-02-27）· tarball SHA-256 `c8ecb65911cd0deb...`<br>
> 研究方式：Maven 多模块源码（15,778 文件）+ Apache-2.0 + BPMN/CMMN/DMN/Task/REST 代码；未构建运行。<br>
> 结论：**Design Reference（企业流程/人工任务/审批）**；不嵌入。

## 1. 项目定位

企业级 BPM（BPMN 2.0）、CMMN、DMN 流程引擎，强调人工任务、审批、流程建模与企业身份/多租户。它是 Java 流程引擎，不是自动化/AI 平台；适合作为 Hisha 人工审批与流程语义的参考，不是执行内核。

## 2. 核心功能

- BPMN 2.0：用户任务、服务任务、网关、事件、子流程、补偿、定时器、多实例。
- CMMN（案例管理）、DMN（决策表）、表单、内容、事件注册、身份链接。
- 任务服务（TaskService）、查询、候选人/候选组、变量/本地变量、历史。
- 异步执行器（JobExecutor）、外部任务、REST API、Spring Boot starter。
- 多租户（tenantId）、流程定义部署/版本、审批/候选组。

## 3. 技术架构

- Java + Maven 多模块：`flowable-engine`、`flowable-bpmn-model`、`flowable-bpmn-converter`、`flowable-task-service`、`flowable-job-service`、`flowable-variable-service`、`flowable-identitylink-service`、`flowable-form-*`、`flowable-rest`、`flowable-spring-boot`、`flowable-cmmn-*`、`flowable-dmn-*`。
- 引擎核心：agenda、runtime、jobexecutor、deployer；命令模式；持久化实体。
- REST：`flowable-rest`（JAX-RS/Spring），流程定义/实例/任务/历史/表单。
- 身份：自带 IDM、LDAP、Spring Security。

## 4. 技术栈

Java、Spring、Maven、JAX-RS/REST、关系数据库（Postgres/MySQL/Oracle/SQL Server 等）、Docker/K8s。前端（Flowable Design/Work）主要是商业产品；本仓库以引擎/REST 为主。

## 5. 数据模型（从代码可见）

ProcessDefinition/Deployment、ProcessInstance、Execution、Task、IdentityLink、Variable、Job/TimerJob、EventSubscription、Historic* 表、Tenant、FormDefinition、DecisionTable。BPMN XML 是流程定义源，部署后版本化。

## 6. 执行机制

- Token/Execution 树推进 BPMN 元素；用户任务创建 Task 并等待完成；服务任务调用 Java delegate/表达式/外部任务。
- 异步执行器处理定时器、异步任务、重试；事务边界与作业持久化。
- 网关（排他/并行/包含/事件）与子流程/补偿实现复杂控制流；多实例支持并行/顺序。
- 多租户以 tenantId 隔离；流程定义按版本部署。
- 没有事件溯源重放/IR；流程语义由 BPMN 标准定义。

## 7. 扩展机制

- Java Delegate、表达式、自定义 Behavior、TaskListener/ExecutionListener、表单、REST 扩展。
- Spring Bean 集成；外部任务客户端。
- 通过 Maven 模块扩展引擎，不是独立插件市场。

## 8. 插件开发方式

编写 Java 类/Spring Bean，注册到引擎配置；或开发外部任务 Worker（HTTP 轮询）。开发体验面向 JVM 团队，与 Hisha 的 Rust Core/TS SDK 不一致。

## 9. API 设计

REST（`flowable-rest`）覆盖 repository/runtime/task/history/form/identity；资源以流程/任务为中心；支持 Basic Auth/Spring Security。没有 OpenAPI-first 的统一能力注册。

## 10. 部署架构

Docker 镜像、K8s 资源（`k8s/`）、Spring Boot 应用；关系数据库；可集群（异步执行器需协调）。Flowable Design/Work 是独立商业产品，不在本仓库。

## 11. 前端交互设计

本仓库没有现代流程设计器；Flowable Design（商业）提供 BPMN 建模。任务列表/表单是 Flowable Work 的产品能力。Hisha 不应期望复用其 UI。

## 12. 许可证

根 `LICENSE` 为 Apache-2.0；`distro/src/license.txt` 也是 Apache。第三方依赖各自许可；商业产品（Design/Work/Cloud）不在本仓库。法律上可复用/修改，但工程上不划算。

## 13. 可直接复用

无代码级复用。可复用的是：
- 人工任务/候选组/审批的领域模型；
- BPMN 网关/子流程/补偿的语义清单；
- 多租户 + 流程定义版本的工程经验；
- 外部任务（external task）作为 Worker 拉取模式的参考。

## 14. 需要二次开发

若引入 Flowable 作为审批引擎：需要 Java 服务、与 Hisha 的 IR/权限/审计/租户双向映射、BPMN 建模器、UI、身份同步。成本高，且会形成 BPMN 与 IR 两套模型。

## 15. 仅值得借鉴

- 用户任务的生命周期（创建/认领/完成/转办/委派）。
- 候选用户/候选组 + 租户的权限模型。
- 补偿/事件子流程/边界事件对异常处理语义的完备性。
- 流程定义部署与版本绑定。
- 外部任务模式（Worker 主动拉取）与 Hisha Hybrid Worker 的相似性。

## 16. 不适合采用

- JVM/BPMN 引擎作为 Hisha 核心（与 TS 栈、IR、可视化不一致）。
- BPMN XML 作为 Canonical IR。
- 依赖商业 Design/Work 才能得到完整 UI。
- 把审批建模为 BPMN 用户任务而绕过 Hisha 的统一权限/审计。

## 17. 与 Hisha 的兼容性

低（代码），中（概念）。Hisha 的 `human.approval` 节点与 Approval Center 可借鉴 Flowable 的任务生命周期和候选组语义，但必须以 Hisha 的 IR、RBAC、审计为准；不引入 BPMN 引擎。

## 18. 引入后的维护成本

作为引擎：高（JVM、数据库、集群、BPMN 建模器、商业 UI）。作为设计参考：低。Hisha 自研轻量审批服务更可控。

## 证据路径

`LICENSE`、`pom.xml`、`modules/flowable-engine/src/main/java/org/flowable/engine/impl/{agenda,runtime,jobexecutor,deployer}/*`、`modules/flowable-task-service/src/main/java/org/flowable/task/service/TaskService.java`、`modules/flowable-bpmn-model/src/main/java/org/flowable/bpmn/model/*`、`modules/flowable-rest/src/main/java/org/flowable/rest/*`、`modules/flowable-spring-boot/*`、`k8s/*`、`docker/*`。
