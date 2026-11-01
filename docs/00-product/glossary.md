# 术语表

> 这些术语贯穿全部文档。若实现与术语冲突，以术语表与 ADR 为准，并更新文档。

## 核心模型

| 术语 | 定义 | 不要混淆 |
| --- | --- | --- |
| **Canonical Workflow IR** | Hisha 的唯一流程中间表示；Canvas/YAML/JSON 都是它的视图/序列化 | 不是 Kestra Flow/BPMN/Node-RED flow JSON |
| **IR Version** | IR 结构本身的 schema 版本（如 `hpa.dev/v1`） | 不是 Workflow 业务版本 |
| **Workflow** | 一个可编辑、可版本化的流程实体 | 不是单次运行 |
| **WorkflowVersion** | 发布时的不可变快照（含 IR、节点版本、权限、依赖） | 不是草稿 |
| **Node** | Workflow 中的一个执行步骤 | 不是 Connector/Tool/Skill/Plugin |
| **Node Type** | 节点类别（HTTP/Condition/Agent/Skill/MCP 等） | 不是某次节点实例 |
| **Node Type Version** | 节点实现的语义版本；发布时固定 | 不是 IR Version |
| **Edge** | 节点之间的连接，可带端口/条件 | 不是数据映射 |
| **Scope / Container** | 循环/并行/分支/子流程等容器节点及其子图 | 不是普通节点 |
| **Trigger** | 启动 Workflow 的事件源（定时/Webhook/事件/MCP） | 不是 Workflow 内部节点 |
| **Execution** | 一次 Workflow 运行 | 不是 NodeExecution |
| **NodeExecution** | Execution 中一个节点的运行/尝试 | 不是 ExecutionEvent |
| **ExecutionEvent** | 追加式的运行事件（状态、日志、输入输出摘要、审批等） | 不是审计日志（审计独立） |
| **Artifact** | 执行产生的文件/对象（日志、报告、导出数据） | 不是 Secret |

## Agent 与 Skill

| 术语 | 定义 | 不要混淆 |
| --- | --- | --- |
| **Agent** | 具有指令/模型/工具/Skill/MCP/预算的执行主体，能动态规划 | 不是 Workflow |
| **AgentVersion** | Agent 配置的不可变版本 | 不是 AgentSession |
| **AgentSession** | 一次 Agent 交互/任务的会话状态与 checkpoint | 不是 Execution |
| **AgentPlan** | Agent 生成的计划（可重规划） | 不是 WorkflowVersion |
| **ToolCall** | Agent 对 Tool/MCP/Workflow/Connector 的一次调用记录 | 不是 NodeExecution |
| **Skill** | 标准 Agent Skills 知识包（SKILL.md + references/scripts/assets），不是可执行程序 | 不是 Plugin/Tool |
| **SkillVersion** | Skill 内容的不可变版本 | 不是 AgentVersion |
| **SkillExecution** | Skill 被加载/测试/脚本执行的一次记录 | 不是 Execution |
| **Skill Runtime** | 加载/解析/上下文/执行/沙箱/日志/产物的运行时 | 不是 Agent Runtime |
| **Skill Manifest** | Hisha 扩展清单 `hpa.skill.json`（权限/脚本/依赖/签名） | 不是 SKILL.md |
| **Progressive Disclosure** | 先加载 metadata/description，按需读取 references/scripts | 不是懒加载代码 |

## 能力与生态

| 术语 | 定义 | 不要混淆 |
| --- | --- | --- |
| **Capability** | 统一能力描述（ID/类型/版本/schema/依赖/权限/运行时/所有者/可见性/状态） | 不是具体入口 |
| **Capability Binding** | 同一能力在某个入口的绑定（Workflow Node / Agent Tool / MCP Tool / Skill / Plugin） | 不是能力副本 |
| **Entry Point** | 能力的调用入口；每个入口有独立权限/策略 | 不是实现 |
| **Connector** | 外部系统适配器（认证/操作/schema/限流/重试） | 不是 Node/Tool |
| **Tool** | Agent 可调用的操作；可来自 Connector/Workflow/MCP/内置 | 不是 Skill |
| **Plugin** | 能力分发包（可含 Skills/MCP 配置/Agents/Node/Connector/Scripts/文档） | 不是 Skill |
| **Marketplace** | 模板/插件/Skill 的分发与治理 | 不是 Registry |
| **Capability Registry** | 统一能力注册、依赖/版本/关系/权限视图 | 不是 Plugin Registry |
| **MCP Server** | 按 MCP 协议暴露 Tools/Resources/Prompts 的服务 | 不是 MCP Client |
| **MCP Client** | 连接/发现/调用外部 MCP Server 的运行时 | 不是 Gateway |
| **MCP Gateway** | 平台对外的 MCP 入口：协议/认证/授权/租户/限流/审计/注册表 | 不是单个 MCP Server |
| **MCP Tool / Resource / Prompt** | MCP 协议暴露的三类能力 | 不是内部 REST API |

## 执行与部署

| 术语 | 定义 | 不要混淆 |
| --- | --- | --- |
| **Control Plane** | API/认证/租户/注册表/版本/编译/校验/调度/审批/审计/监控 | 不直接执行不可信脚本 |
| **Execution Plane** | Workflow/Agent/Skill/MCP/Node/Connector/Script Runtime + Worker + Sandbox | 不是控制面 |
| **RuntimeAdapter** | Hisha 对执行后端的统一契约（local/worker/temporal） | 不是具体引擎 |
| **Local Runtime** | 本地运行的 Control Plane 子集 + 执行器 + 本地注册/存储/日志 | 不是云端 Worker |
| **Worker** | 执行 Plane 的工作进程；可本地/私有/云/混合 | 不是 Node |
| **Hybrid Worker** | 在客户内网、主动出站连接云端 Control Plane 的 Worker | 不是云 Worker |
| **Lease** | Worker 对任务的租约；超时重新调度 | 不是锁 |
| **Checkpoint** | 执行状态快照，用于恢复 | 不是事件日志 |
| **Durable Execution** | 事件溯源 + 确定性重放 + 长任务恢复（Temporal 提供） | 不是普通检查点 |
| **At-least-once** | 至少执行一次；配合幂等/补偿 | 不是 Exactly Once |
| **Idempotency Key** | 外部调用去重键 | 不是 Execution ID |
| **Compensation** | 失败后的补偿动作（Saga） | 不是重试 |
| **Approval** | 人工审批任务；暂停/恢复执行 | 不是通知 |
| **Audit Log** | 不可篡改的操作审计（谁/何时/对什么/结果） | 不是 ExecutionEvent |
| **Artifact** | 执行产物 | 不是日志 |

## 安全与治理

| 术语 | 定义 |
| --- | --- |
| **Tenant / Workspace / Project** | 租户（隔离边界）/ 工作区（团队/环境）/ 项目（资源分组） |
| **RBAC** | 基于角色的访问控制 |
| **ABAC / Policy** | 基于属性/策略的访问控制（资源级、时间、IP、风险） |
| **PDP / PEP** | 策略决策点 / 策略执行点 |
| **Credential Vault** | 凭证加密存储与引用系统 |
| **Sandbox** | 脚本隔离执行环境（文件/网络/CPU/内存/超时） |
| **Prompt Injection** | 恶意/错误指令注入到模型上下文；Skill/工具输出均不可信 |
| **Trust Level** | Skill/Plugin 来源信任等级（官方/已验证/社区/不可信） |
| **SBOM** | 软件物料清单（依赖/许可证/版本） |
| **NOTICE** | Apache-2.0 等许可要求的归属文件 |

## UI 与体验

| 术语 | 定义 |
| --- | --- |
| **Designer** | Workflow 可视化编辑器（Palette/Canvas/Inspector/Debugger/Topbar） |
| **Inspector** | 右侧节点配置面板 |
| **Palette** | 左侧节点库 |
| **Execution Debugger** | 底部运行调试面板（输入/输出/日志/时间线） |
| **Approval Center** | 人工审批工作台 |
| **Local Runtime Manager** | 本地运行时状态/日志/更新/同步管理 |
| **i18n** | 国际化；默认日语，支持日/英/中 |

## 缩略语

| 缩写 | 含义 |
| --- | --- |
| IR | Intermediate Representation |
| MCP | Model Context Protocol |
| HITL | Human-in-the-loop |
| PDP/PEP | Policy Decision/Enforcement Point |
| OIDC | OpenID Connect |
| OTel | OpenTelemetry |
| SLO | Service Level Objective |
| ADR | Architecture Decision Record |
| SBOM | Software Bill of Materials |
| DAG | Directed Acyclic Graph |
