# 产品愿景

## 1. 一句话

Hisha 是统一的自动化与 AI 能力平台：用一套 Workflow IR 构建确定性流程，用 Agent 处理不确定性，用标准 Agent Skills 封装专业知识，通过 MCP 把能力分发给任何 AI 客户端，并能在本地、私有云、SaaS 与混合环境中执行。

## 2. 要解决的问题

今天的团队通常被迫组合多个系统：

- 可视化自动化（n8n/Activepieces）擅长连接器与触发，但 AI/Agent 能力弱、许可/扩展受限。
- AI 应用平台（Dify/Flowise）擅长 Prompt/RAG/Agent，但确定性流程、版本、审批、企业治理不足。
- 数据编排（Airflow/Dagster/Kestra）擅长调度/数据，但不是通用业务自动化，也缺少 Agent/Skill/MCP 的统一入口。
- Agent 框架（LangGraph）擅长状态图，但不是产品，不提供多租户/权限/可视化/市场。
- 能力分发（MCP/Agent Skills）正在成为外部 AI 生态的事实接口，但缺少“构建、治理、执行、分发”一体化的平台。

结果是：同一能力被重复实现为 Workflow 节点、Agent Tool、MCP Tool、Skill、插件；权限与审批在每个入口各自为政；本地运行依赖云；许可证与供应链风险不可控。

## 3. 产品主张

**Build Once. Execute Anywhere. Publish Everywhere.**

- **Build Once**：能力只定义一次（IR/描述/schema），通过绑定暴露为 Workflow 节点、Agent Tool、MCP Tool、Skill 或插件。
- **Execute Anywhere**：同一版本可在 Local Runtime、Private Worker、Cloud Worker、Hybrid Worker 执行。
- **Publish Everywhere**：通过 MCP、Agent Skills、插件包向 Claude、Copilot、Codex、企业 Agent 分发。

**Automation First. AI Optional. AI Native.**

- 确定性 Workflow 不依赖 AI 也能完整执行。
- AI 是可选增强：生成流程、选择工具、处理异常、总结结果。
- AI 不能绕过权限、审批、预算和审计。

## 4. 五根支柱

1. **Canonical Workflow IR**：Canvas/YAML/JSON 共享一个语义模型；编译、校验、版本、静态分析。
2. **双执行模型**：Workflow（确定性、可重放/可恢复）与 Agent（动态规划、工具选择、人工介入）分离，通过明确合同互相调用。
3. **Skill 标准与本地运行时**：遵循 Agent Skills 标准；本地无云可加载/执行；脚本沙箱；可导出给外部客户端。
4. **MCP 双向能力**：既作为 MCP Client 调用外部工具，也作为 MCP Server/Gateway 向外部客户端暴露能力；当前规范 2026-07-28 + 旧版兼容。
5. **统一能力注册与治理**：一个 Capability 描述，多个入口绑定；每个入口独立权限/审批；租户隔离、审计、配额、市场。

## 5. 目标用户

| 用户 | 目标 | 关键能力 |
| --- | --- | --- |
| 自动化开发者 | 快速构建/调试/发布确定性流程 | Designer、IR、YAML/JSON、执行调试、连接器 |
| AI 工程师 | 构建可治理的 Agent/Skill | Agent Designer、Skill Editor/Test、模型网关、MCP |
| 平台工程师 | 私有化/混合部署、接入企业系统 | Control/Execution Plane、Worker、OIDC、审计 |
| 安全/治理 | 防止越权、恶意 Skill/Plugin、数据泄漏 | 权限、审批、沙箱、审计、配额、供应链签名 |
| 业务审批人 | 处理人工任务 | Approval Center、表单、SLA、通知 |
| 外部 AI 用户 | 在 Copilot/Claude/Codex 中使用企业能力 | MCP 端点、Skill 导出、认证/审计 |

## 6. 差异化

| 对比 | 差异 |
| --- | --- |
| Dify/Flowise | Hisha 以确定性 IR 为一等公民；AI 节点/ Agent 是可选能力；本地/离线与 MCP/Skill 分发是核心；不受其许可证限制 |
| n8n/Activepieces | Hisha 有统一能力注册与多入口权限；IR 可版本化/静态分析；不绑定单一插件生态 |
| Kestra/Airflow/Dagster | Hisha 面向业务自动化 + AI，不是数据管道；IR 支持 Canvas/YAML/JSON 一致；本地运行时更轻 |
| Temporal | Hisha 提供产品层（设计、治理、MCP、Skills）；Temporal 只作为可选 durable 后端 |
| LangGraph | Hisha 提供平台/运行时/治理；LangGraph 只作为 Agent 内核 |
| MCP/Skills 工具 | Hisha 提供构建、版本、权限、审批、审计、市场，而不只是协议/文件格式 |

## 7. 成功指标

### 产品

- 新用户从登录到运行第一个 Workflow < 10 分钟（本地）。
- Canvas → YAML → JSON → Canvas 语义往返 100%（fixture）。
- 发布 Workflow 的静态校验拦截率 100%（未授权/未绑定变量/缺失依赖）。
- Skill 导入 → 本地执行 → 导出 < 5 分钟（无云）。
- 外部客户端（至少 Claude Code、VS Code/Copilot、Codex）能发现并调用 Hisha MCP 工具。

### 平台

- 执行事件可追溯 100%（execution → node → tool call → approval → artifact）。
- 跨租户访问 0（隔离测试）。
- 高风险写操作 100% 经过审批策略（可配置）。
- 本地离线运行不依赖云（断网测试）。
- Worker 断线恢复后任务不丢失（至少一次 + 幂等）。

## 8. 非目标

- 不自研大模型；通过本地模型/云模型/模型网关接入。
- 不追求完整 BPMN 2.0 兼容；人工审批借鉴其语义，不引入 BPMN 引擎。
- 不在 Phase 1 实现多租户 SaaS 计费/市场结算。
- 不承诺外部系统 Exactly Once；只承诺至少一次 + 幂等/补偿。
- 不做通用 IDE/代码托管；Workflow 代码节点是受限沙箱。
- 不把所有模块微服务化；初期 Modular Monolith + 独立 Worker。

## 9. 产品边界一句话

Hisha 不试图替代所有工具；它把“构建能力、执行能力、治理能力、分发能力”放在一个平台上，并让成熟组件各就各位。
