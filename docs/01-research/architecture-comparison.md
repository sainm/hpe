# 架构对比与执行引擎选型

> 目标不是选“最强项目”，而是回答：Hisha 的 Canonical IR 应该由什么执行，哪些能力必须自研，哪些可以集成。

## 1. 评估维度

| 维度 | 权重 | 说明 |
| --- | --- | --- |
| IR 兼容/可控 | 25% | 能否无损承载 Hisha IR 的分支/循环/并行/审批/版本 |
| 恢复与长任务 | 20% | 崩溃、重启、人工等待、定时器、Worker 离线 |
| 本地优先 | 15% | 单机/离线/无云可用，安装体积与运维 |
| 扩展模型 | 15% | 节点/连接器/Agent/Skill 的注册与版本 |
| 多租户/治理 | 10% | 租户/工作区隔离、权限、审计 |
| 生态与维护 | 10% | 语言栈、社区、升级、许可 |
| 许可 | 5% | 商业可用性 |

## 2. 候选对比

| 候选 | IR 兼容 | 恢复/长任务 | 本地优先 | 扩展 | 多租户 | 维护 | 许可 | 综合 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Kestra 2.0.7 | 中：自有 YAML Flow | 中：DB 状态 + replay from task | 中：JVM + DB | 强：Maven 插件 | 中：Tenant/Namespace | 高 | Apache-2.0 | 中 |
| Temporal 1.32.1 | 低：代码 Workflow | 强：事件溯源/确定性重放 | 中重：server+DB | 中：SDK/Activity | 中：Namespace | 高 | MIT | 高（作为适配器） |
| 自研轻量运行时 | 高：为 IR 而生 | 中：检查点/恢复，非确定性重放 | 强：单进程/嵌入式 DB | 高：Rust 节点 SDK/WASM | 自建 | 中 | 自有 | **v1 最高** |
| Graphon 0.8.0 | 中高：图/容器/变量池 | 中： snapshot/恢复 | 中：Python 库 | 中：节点 | 无 | 中 | Apache-2.0 | 参考/可选 Python 引擎 |
| LangGraph 1.2.19 | 低：Agent 图 | 中：checkpoint/interrupt | 强：库 | 中：节点/工具 | 无 | 中 | MIT | Agent 专用 |
| Airflow 3.3.2 | 低：Python DAG | 中 | 弱 | 强：Provider | 弱 | 高 | Apache-2.0 | 不适用 |
| Dagster 1.13.28 | 低：资产 | 中 | 弱 | 中 | 弱 | 高 | Apache-2.0 | 不适用 |
| Flowable 8.0.0 | 低：BPMN | 中 | 弱 | 中 | 中 | 高 | Apache-2.0 | 审批参考 |
| Node-RED 5.0.9 | 低：msg 图 | 弱 | 强 | 中 | 弱 | 低 | Apache-2.0 | 本地体验参考 |

## 3. 为什么不是 Kestra 作为内核

**优点**：YAML-first、成熟的调度/执行/历史、插件生态、MCP Server、UI、Apache-2.0、Helm。

**不选为内核的原因**：

1. Kestra Flow 是它自己的 YAML 模型（`Flow`/`Task`/`Trigger`/`Pebble`），不是 Hisha IR。双向映射会丢失 Hisha 的节点版本、布局、权限、审批、数据契约；Canvas 需要维护两套语义。
2. 插件是 JVM/Maven 工件。Hisha 的节点/连接器 SDK、Local Runtime、Worker 是 Rust/WASM；TS 只做 Control Plane/Web。为 Kestra 写 Java 桥接会引入双语言扩展体系。
3. 本地优先需要 JVM + 数据库 + 队列；“单机 H2”可行但体积/启动/跨平台体验不符合桌面本地运行时目标。
4. Kestra 的 replay 是“从某个 taskRun 新建执行”，不是事件溯源确定性重放；长任务/强恢复仍不如 Temporal。
5. Kestra 已经内置 AI Copilot/MCP，与 Hisha 的 Agent/MCP 能力重叠，直接叠加会造成两套能力面。
6. 企业治理能力在 OSS 中部分被忽略，不能把 Kestra 的 EE 当 Hisha 的治理层。

**保留的可能**：只做“Kestra YAML 导入器 / 可选 Kestra 执行适配器”，服务已有 Kestra 用户；不进入 Phase 1-4 核心。

## 4. 为什么 Temporal 只做长期适配器

**优点**：MIT、事件溯源/确定性重放、Signals/Updates、Timers、Child Workflow、Schedules、Nexus、成熟 SDK（含 TypeScript）。

**不作为 v1 唯一内核的原因**：

1. Workflow 是代码，不是数据。Hisha 必须实现一个通用 IR 解释器 Workflow + Activity 桥接，工作量和版本兼容复杂度高。
2. 本地/离线需要打包 server + 数据库（CLI dev server 可单机，但仍是额外进程/存储）；桌面体验不如嵌入式运行时。
3. 多租户主要靠 Namespace；高基数工作区/租户的运维与配额需要额外设计。
4. 对“短流程、本地、无云”的场景，Temporal 是过度设计；v1 的主要验收是 Canvas/YAML/JSON 可执行，而不是长任务恢复。
5. 需要数据库 schema/版本升级/可见性/归档的专门运维能力。

**结论**：Phase 1 用自研轻量运行时满足本地与标准流程；Phase 3 做 Temporal PoC（IR 解释器 Workflow + 节点 Activity + 审批 Signal + Worker 离线恢复）。只有 PoC 通过才把 `durable` 作为默认/可选后端。不要在 Phase 1 同时引入 Kestra/Temporal。

## 5. 为什么自研轻量运行时是 v1 正确选择

1. **IR 是产品核心**：执行语义必须与 IR 一一对应，才能保证 Canvas/YAML/JSON 一致；把语义外包给 Kestra/Temporal 会失去控制。
2. **本地优先**：Rust 单进程 + SQLite 可在无云、无 JVM、无外部数据库时运行；这是 Hisha 的差异化。
3. **执行语义可测试**：分支/循环/并行/重试/超时/审批的执行器可以在单元/属性测试中验证；PoC A 直接覆盖。
4. **扩展模型统一**：节点/连接器/Agent/Skill/MCP 的运行时在 Rust/WASM，跨语言契约用 Proto/JSON Schema；Worker 与 Local Runtime 共享 Rust crate。
5. **不等于重造 durable execution**：轻量运行时只承诺“检查点 + 至少一次 + 可恢复”，不承诺确定性重放；需要强 durable 的流程由 Temporal 适配器承接。我们在文档中明确边界，避免虚假承诺。

## 6. 执行栈目标形态

```
Canvas / YAML / JSON
        │
   Compiler + Validator
        │
  Canonical IR (immutable version)
        │
  RuntimeAdapter (interface)
   ├── LocalRunner (Rust, embedded SQLite, v1)
   ├── WorkerRunner (Rust, Postgres/Redis, private/SaaS)
   └── TemporalAdapter (IR interpreter Workflow + Activities, Phase 3+)
        │
  Node / Connector / Agent / Skill / MCP executors
```

- `RuntimeAdapter` 契约固定：start/resume/cancel/stream/query/approve。
- 同一 IR 快照、同一节点版本、同一权限策略在三类执行器上语义一致。
- 事件模型统一：`ExecutionEvent` 追加式；Temporal 的 history 映射到 Hisha 事件，不直接暴露。

## 7. PoC 比较计划（不提前引入多个引擎）

| 步骤 | 内容 | 通过标准 |
| --- | --- | --- |
| P0 | 用 10 个 IR fixture 在自研运行时执行 | 分支/循环/并行/重试/审批全部通过 |
| P1 | 同一 fixture 编译到 Temporal 解释器 | 结果与事件语义一致；恢复/审批通过 |
| P2 | 比较开发/运维成本 | 记录 LOC、依赖、启动时间、故障恢复、运维步骤 |
| P3 | 决策 | 只有 P1/P2 明显优于自研时才启用 Temporal 默认 |

Kestra 只做引擎对比 spike：同一流程在 Kestra 单机与 Temporal dev server 上验证，记录启动/资源/扩展/LOC/恢复/运维；不作为 Hisha 内核。另验证“导入 Kestra YAML”是否值得做。

## 8. 最终判断

| 问题 | 结论 |
| --- | --- |
| Kestra vs Temporal | Temporal 更适合作为 Hisha 的长期 durable 后端；Kestra 更适合作为 YAML/插件生态的设计参考/可选导入源 |
| 是否自研 Workflow Runtime | 是，v1 自研轻量运行时；长期 durable 用 Temporal 适配器 |
| LangGraph 的位置 | Agent Runtime 内核，不是 Workflow 引擎 |
| 是否同时引入多个引擎 | 否；Phase 1 只自研，Phase 3 才 PoC Temporal |
| 是否用 Kestra 插件生态 | 否；Hisha 自建 Rust/WASM 节点/连接器/插件体系，TS 只做 UI/SDK |
