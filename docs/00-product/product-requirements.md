# 产品需求

> 需求分级：M（必须）/ S（应该）/ C（可以）。Phase 1 只实现 M0 子集；其他阶段见路线图。
> 每个需求有 ID，供验收与测试追踪。

## 1. 范围

Hisha Phase 0-6 的完整产品范围：

1. Workflow Center（可视化/YAML/JSON/AI 生成）
2. Agent Center（Agent 配置、运行时、测试、多 Agent）
3. Skill Center（标准 Skill 创建/导入/测试/发布/导出）
4. MCP Center（Client/Server/Gateway/Tools/Resources/Prompts）
5. Plugin Center（打包/安装/签名/市场）
6. Capability Registry（统一能力注册/绑定/关系图）
7. Connections（连接器/凭证/外部系统）
8. Execution Monitor（运行/日志/时间线/产物）
9. Approval Center（人工审批）
10. Marketplace（模板/插件/Skill 分发）
11. Settings（用户/权限/模型/部署/系统）
12. Local Runtime（本地注册、执行、离线、同步）
13. Enterprise Governance（多租户、OIDC、审计、配额、合规）

## 2. 功能需求

### 2.1 Workflow（WF）

| ID | 优先级 | 需求 |
| --- | --- | --- |
| WF-01 | M | Canvas、YAML、JSON 共享一个 Canonical IR；三者可互相转换，语义一致 |
| WF-02 | M | IR 支持顺序、条件、多路分支、循环、并行、子流程、变量、数据映射、异常、重试、超时、补偿、人工审批、定时/事件触发 |
| WF-03 | M | 每个节点有 ID、类型、类型版本、名称、描述、输入/输出/配置 Schema、权限、超时、重试、错误策略、文档 |
| WF-04 | M | 发布生成不可变 WorkflowVersion 快照；Execution 绑定明确版本 |
| WF-05 | M | 发布前执行 Schema、静态、权限、依赖、执行策略五类校验；任一失败禁止发布 |
| WF-06 | S | YAML/JSON 文本保真：未编辑的注释/格式可保留；Canvas 编辑后可生成规范化文本并保留头部注释 |
| WF-07 | S | 节点类型版本升级/迁移；旧版本可继续执行 |
| WF-08 | S | 静态分析：不可达节点、未绑定变量、类型不匹配、循环/并行语义、缺失依赖/凭证、子流程版本 |
| WF-09 | S | AI 生成 Workflow：JSON Schema 约束输出 → 校验 → 修复 → 预览/差异；不得跳过校验 |
| WF-10 | C | 自动布局、Minimap、分组、对齐、快捷键、撤销/重做 |
| WF-11 | M | 执行调试：输入、输出、日志、错误、时间线、重跑节点/从节点恢复（受策略限制） |
| WF-12 | M | 确定性 Workflow 在没有 AI、没有云端 Control Plane 时可执行 |

### 2.2 Agent（AG）

| ID | 优先级 | 需求 |
| --- | --- | --- |
| AG-01 | M | Agent 配置：名称/描述/指令/模型/温度/Skills/Tools/MCP/Workflow 访问/记忆/状态/预算/步数/工具调用/超时/Token/成本/审批策略 |
| AG-02 | M | Agent Runtime 支持任务理解、规划、Skill/Tool 选择、调用、观察、评估、重规划、检查点、中断、恢复、人工审批 |
| AG-03 | M | 支持单 Agent、多 Agent、Supervisor/Worker、Sub-agent、Handoff |
| AG-04 | M | 执行限制：最大步数、最大工具调用、超时、Token/成本预算；超限终止并可审计 |
| AG-05 | M | Agent 调用 Workflow/MCP/其他 Agent 必须遵守统一输入输出合同与权限 |
| AG-06 | M | Agent 不得绕过 Workflow 的权限、审批、执行策略 |
| AG-07 | S | Agent 测试：模拟工具、回放、断言、评分、轨迹查看 |
| AG-08 | S | 记忆/状态分层：会话状态、长期记忆、工具结果；可清除/导出 |

### 2.3 Skill（SK）

| ID | 优先级 | 需求 |
| --- | --- | --- |
| SK-01 | M | 遵循 Agent Skills 标准目录与 frontmatter；不创建私有 Skill 格式 |
| SK-02 | M | 创建/导入/编辑/搜索/校验/测试/发布/版本/启停/依赖/权限/市场 |
| SK-03 | M | 导入外部 Skill；导出标准目录；保留未知字段 |
| SK-04 | M | Skill Editor：基础信息、SKILL.md、Instructions、references/scripts/assets、元数据、依赖、权限、测试、发布；原始 Markdown 模式 |
| SK-05 | M | Local Skill Runtime：Loader/Resolver/Context/Execution/Script Executor/Permission/Sandbox/Logger/Artifacts |
| SK-06 | M | 支持 Python/Node.js/Shell 脚本；语言/运行时可通过配置扩展 |
| SK-07 | M | 本地 Skill 在无云 Control Plane 时加载/执行；支持离线 |
| SK-08 | M | 三种运行方式：Local/Hybrid/Cloud；模型可本地/云/无模型 |
| SK-09 | M | Skill Security：来源信任、完整性、沙箱、工作区限制、文件/网络权限、命令白名单、环境隔离、CPU/内存/超时、审批、审计 |
| SK-10 | S | Skill 测试控制台：mock 模型/工具、断言、快照、覆盖率 |
| SK-11 | S | 依赖管理：Skill/脚本/包/连接器依赖；版本范围与冲突检测 |
| SK-12 | S | 市场：发布/评分/举报/下架/签名 |

### 2.4 MCP（MC）

| ID | 优先级 | 需求 |
| --- | --- | --- |
| MC-01 | M | MCP Client：注册/连接/认证/传输/工具/资源/Prompt 发现/调用/超时/错误/监控 |
| MC-02 | M | MCP Server：暴露 Tools/Resources/Prompts；独立 MCP Gateway |
| MC-03 | M | Gateway：协议/认证/授权/租户隔离/注册表/限流/审计/监控 |
| MC-04 | M | 工具由能力注册中心动态生成；有输入输出 Schema；不无限制暴露内部 API |
| MC-05 | M | 高风险写操作需要独立授权和人工确认 |
| MC-06 | M | 本地 MCP Client/Server/Registry/Auth/Config；本地 Server 可独立运行 |
| MC-07 | M | 支持 MCP 2026-07-28（无状态、discover、MRTR、subscriptions/tasks）与旧版兼容 |
| MC-08 | S | MCP Explorer：连接、工具列表、schema、测试调用、资源/Prompt 预览 |
| MC-09 | S | 外部客户端集成：Claude/Copilot/Codex/自定义；按客户端生成配置/说明/兼容性测试 |
| MC-10 | S | Resources 读取范围/租户权限；Prompts 模板管理 |

### 2.5 Capability/Connector/Plugin（CP）

| ID | 优先级 | 需求 |
| --- | --- | --- |
| CP-01 | M | 统一 Capability：ID/名称/类型/版本/描述/输入输出 Schema/依赖/权限/运行时/所有者/可见性/状态/文档 |
| CP-02 | M | 同一底层能力可注册为 Workflow Node 与 Agent Tool/MCP Tool，但各入口独立权限/执行策略 |
| CP-03 | M | 能力发现、依赖分析、版本兼容检查、权限检查、关系视图 |
| CP-04 | M | Connector：HTTP/REST/GraphQL/DB/File/Git/SVN/GitHub/GitLab/Azure DevOps/Email/MQ/Object Storage/MCP |
| CP-05 | M | Connector：认证、凭证管理、连接测试、限流、重试、超时、错误、审计 |
| CP-06 | M | 凭证不得明文出现在 Workflow YAML/JSON/Skill 文件；使用引用 |
| CP-07 | S | Plugin：导入/导出/安装/升级/卸载/依赖/版本/许可/签名/完整性 |
| CP-08 | S | Plugin 可含 Skills/MCP 配置/Agents/Node/Connector/Scripts/文档 |
| CP-09 | C | Marketplace：模板/插件/Skill 分发、评分、审核、下架 |

### 2.6 执行与监控（EX）

| ID | 优先级 | 需求 |
| --- | --- | --- |
| EX-01 | M | Execution/NodeExecution/AgentSession/ToolCall 分别建模；执行事件追加式 |
| EX-02 | M | Execution 记录 ID/Workflow/版本/输入/输出/状态/开始/结束/时长/错误/Worker/Artifacts |
| EX-03 | M | 外部系统调用考虑幂等/重试/补偿；不承诺 Exactly Once |
| EX-04 | M | 长任务/人工等待/Worker 离线可恢复（标准运行时检查点；durable 适配器后续） |
| EX-05 | M | 实时事件流（SSE/WebSocket）与历史查询 |
| EX-06 | M | Execution Monitor：列表、详情、时间线、日志、错误、产物、重跑/取消/审批 |
| EX-07 | S | 指标/追踪（OTel）；告警/配额 |
| EX-08 | S | 节点级重跑/从节点恢复；补偿执行 |

### 2.7 平台与治理（PL）

| ID | 优先级 | 需求 |
| --- | --- | --- |
| PL-01 | M | Tenant/Workspace/Project/User/Membership/Role/Permission |
| PL-02 | M | 认证：本地账号 + OIDC；API Token/服务账号；Worker mTLS/JWT |
| PL-03 | M | RBAC + 资源级策略；租户/工作区隔离；审计 |
| PL-04 | M | Credential Vault、加密、轮换、引用；不透明日志 |
| PL-05 | M | Trigger：定时/事件/Webhook/MCP；调度管理 |
| PL-06 | M | Model Provider/Model Configuration；本地/云/无模型 |
| PL-07 | S | 配额/限流/数据保留/备份恢复 |
| PL-08 | S | 多租户 SaaS、Usage Metering、Billing（Phase 6） |
| PL-09 | M | UI 默认日语；支持日/英/中；所有文案 i18n；无硬编码 |
| PL-10 | M | 响应式：桌面高密度；窄屏（≈607px）可用；键盘/可访问性 |

## 3. 非功能需求

| ID | 类别 | 要求 |
| --- | --- | --- |
| NFR-01 | 本地优先 | 无云、无外部数据库时可运行 Workflow/Skill/Agent（模型可选） |
| NFR-02 | 安全 | 默认最小权限；脚本沙箱；网络/文件策略；秘钥不落盘明文；跨租户 0 |
| NFR-03 | 可靠性 | 执行事件不丢；至少一次；幂等键；崩溃恢复；Worker 断线重连 |
| NFR-04 | 性能 | 见 [性能要求](../10-quality/performance-requirements.md) |
| NFR-05 | 可观测性 | OTel trace/metrics/logs；execution_id 贯穿；审计独立 |
| NFR-06 | 可移植 | 同一 IR 在 Local/Private/SaaS/Hybrid 语义一致 |
| NFR-07 | 可扩展 | Node/Connector/Skill/Plugin/MCP 版本化；向后兼容策略 |
| NFR-08 | 许可 | 不使用 SUL/受限 EE/专有 Skill；SBOM；NOTICE |
| NFR-09 | i18n | 默认日语；日/英/中完整；无硬编码文案；日期/数字/时区本地化 |
| NFR-10 | 兼容 | 外部 AI 客户端按各自规范；MCP 2026-07-28 + 旧版 |
| NFR-11 | 可维护 | Modular Monolith；模块边界清晰；Worker 可独立部署 |
| NFR-12 | 可测试 | IR 往返/执行语义/安全/离线/多租户自动化测试 |

## 4. Phase 1 范围（M0）

必须交付：

- Workflow Designer（Canvas + Inspector + 调试）
- IR 1.0 + JSON Schema + Canvas/YAML/JSON 转换
- 核心节点：Start/End/Condition/Switch/Loop/Parallel/Join/HTTP/Script/Transform/Human Approval/Subworkflow
- 轻量运行时（本地）+ 执行事件/检查点
- Execution Monitor（基础）
- Node Registry + 静态校验 + 发布版本
- 本地文件/CLI 启动；无云依赖
- i18n（日/英/中）与基础设计系统

Phase 1 不做：Agent Runtime、Skill Center、MCP Gateway、Plugin Marketplace、多租户 SaaS、Temporal 适配。

## 5. 验收与追踪

- 每条 M 需求必须映射到 [验收标准](../10-quality/acceptance-criteria.md) 与测试用例。
- 未实现的需求必须标记“未做”，不得以文档/占位符冒充完成。
- 需求变更需更新本文件并记录 ADR/变更日志。

## 6. 开放问题

见 [open-issues](../12-decisions/open-issues.md)：Hisha 自身许可证、SaaS 计费、移动端支持、BPMN 导入、Kestra 导入、模型本地化范围。
