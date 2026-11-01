# 15 个最终设计问题

> 每题按“推荐方案 / 选择依据 / 替代方案 / 风险 / 实施建议”回答。相关证据见各模块文档与 [研究](../01-research/README.md)。

## 1. 是否应该采用一个统一 Workflow IR？

**推荐**：是。一个 Canonical IR（`hpa.dev/v1`）作为唯一语义来源；Canvas/YAML/JSON 是视图/序列化。

**依据**：三套模型会导致语义漂移、版本/权限/执行不一致；开源项目的教训是模型与 UI 绑定后难以复用（Kestra/BPMN/n8n）。统一 IR 是能力注册/跨运行时/跨客户端分发的基石。

**替代**：Canvas 模型 + 导入/导出转换器（短期快，长期双模型）；或采用 BPMN/Kestra Flow 作为 IR（受其语义/生态约束）。

**风险**：IR 过度设计/节点专属字段污染；YAML/JSON 文本无法无损。缓解：PoC A 冻结、fixture/属性测试、布局/注释 sidecar、扩展字段保留。

**实施**：Phase 1 先做 IR v0.1 + 编译器 + 10 个 fixture；PoC A 通过后冻结 v1；破坏性变更走 ADR/迁移。

## 2. Kestra 和 Temporal 哪个更适合做底层执行引擎？

**推荐**：Temporal 作为长期 durable execution 适配器；Kestra 作为设计参考/可选 YAML 导入源，不作为核心引擎。

**依据**：Temporal 的事件溯源/确定性重放/Signal/Update/Timer 适合长任务、审批、故障恢复，MIT，TS/Go SDK 成熟（作为薄适配 Worker）；Kestra 的 YAML/插件/调度优秀，但 Flow 模型不是 Hisha IR、插件是 JVM/Maven、replay 不是确定性重放、本地需 JVM/DB、AI/MCP 能力与 Hisha 重叠。

**替代**：Kestra 内核 + Hisha IR 编译到 Kestra Flow（双模型/双语言插件）；或纯自研 durable（重造事件溯源）。

**风险**：Temporal 解释器/版本兼容/运维复杂度；Kestra 映射丢失语义。缓解：Phase 3 最小 PoC；不默认启用；Kestra 只做导入器。

**实施**：v1 自研轻量运行时；Phase 3 做 Temporal IR 解释器 + Activity + Signal PoC；通过后再提供 `durable` 选项。

## 3. 是否需要自研 Workflow Runtime？

**推荐**：是，但只自研轻量执行内核（IR 解释、检查点、恢复、审批、事件），不自研 durable execution。

**依据**：本地优先/IR 语义/可测试/TS 统一栈要求自有运行时；Temporal 解决长任务/强恢复，但本地体积/运维/IR 解释器成本高。两者职责不同。

**替代**：Temporal-only（本地打包 server）；Kestra-only；Node-RED/Activepieces 运行时。

**风险**：恢复语义被误解为 durable；检查点/幂等实现错误。缓解：明确边界、崩溃测试、幂等键、Temporal 适配器。

**实施**：先实现 LocalRunner/WorkerRunner 同核；`RuntimeAdapter` 契约；PoC A 验证；Temporal 适配器独立。

## 4. LangGraph 应该处于什么位置？

**推荐**：Agent Runtime 内核（库），封装在 `AgentRuntime` 契约后；不作为 Workflow IR/通用执行引擎，不暴露其图/状态类型。

**依据**：LangGraph 的 Pregel/checkpoint/interrupt/durability 三档与 Agent 需求吻合；MIT；Python 实现作为可选 AI Worker。它不提供多租户/调度/权限/审批/UI。

**替代**：自研 Agent 状态机；Dify Agent/n8n Agents（许可/栈不匹配）；纯 Prompt loop（能力不足）。

**风险**：版本迭代快/checkpoint 迁移；LangChain 生态扩散。缓解：锁定版本、限制使用面、契约测试、包装层。

**实施**：Phase 4 用 Python 3.13 + `langgraph` 1.2.x + Postgres/SQLite checkpointer，运行在可选 AI Worker；Rust 负责工具/预算/审批/模型网关；每季度评估升级。

## 5. Skill Runtime 是否应该独立于 Agent Runtime？

**推荐**：是。Skill Runtime 独立（Loader/Resolver/Context/Execution/Script/Sandbox/Artifacts），Agent Runtime 调用它；两者共享 Sandbox/Artifact/Policy。

**依据**：Skill 也需被 Workflow、MCP、Local CLI、外部客户端使用；若嵌入 Agent，会限制无 AI/确定性执行与本地分发。Skill 的脚本/权限/产物是独立生命周期。

**替代**：Skill 作为 Agent 内部模块（短期简单，长期耦合）；Skill 作为纯文件无运行时（无法执行脚本/产物/审计）。

**风险**：接口重复/权限边界模糊。缓解：统一 `SkillContract`；Agent 只负责选择/加载，执行/沙箱归 Skill Runtime。

**实施**：Phase 2 先做 Skill Runtime + 确定性入口；Phase 4 Agent 接入；MCP Tool 复用同一执行器。

## 6. 本地 Skill 是否可以在无云端连接时完整运行？

**推荐**：可以，且必须。本地 Runtime 包含 Registry/Loader/Resolver/Script/Sandbox/Artifacts/Logs；模型可选（本地/无）。

**依据**：Local-first 是产品差异化；云依赖会让离线/隐私/边缘场景失效。Skill 的 instructions/script 本身不需要云；只有云模型/市场/同步需要。

**替代**：云 Control Plane 瘦客户端（不符合需求）；仅缓存（无法安装/执行/审计）。

**风险**：本地沙箱弱/跨平台/依赖安装。缓解：容器/受限进程、doctor、离线包、明确风险；高风险默认容器。

**实施**：Phase 2 验收“断网导入→执行→产物→导出”；PoC B 用真实客户端验证。

## 7. 如何使 Skill 同时兼容平台和外部 AI 客户端？

**推荐**：以 Agent Skills 标准目录为唯一格式；平台只加 `hpa.skill.json` 扩展清单；导出时按客户端生成路径/安装说明，不写私有字段到 SKILL.md。

**依据**：Claude Code/Codex 等已支持 SKILL.md；私有格式会锁死分发。客户端路径/字段不同（`.claude/skills` vs `.agents/skills` vs VS Code），不能一个包通吃。

**替代**：平台私有 Skill 格式 + 导出转换（转换丢失/维护高）；为每个客户端维护独立格式（生态割裂）。

**风险**：客户端字段/版本差异；扩展字段被忽略。缓解：标准字段优先、未知字段保留、兼容性测试、导出适配器。

**实施**：Phase 2 导出标准目录；PoC B 在至少 2 个客户端加载；文档列明差异。

## 8. 如何让 Copilot 通过 MCP 使用平台能力？

**推荐**：提供 Hisha MCP Gateway（Streamable HTTP，2026-07-28 + 旧版兼容），按 VS Code/Copilot CLI 各自配置格式提供样例；OAuth 2.1/PAT；工具由 Capability Registry 生成；高风险审批。

**依据**：VS Code 工作区 `servers` 与 Copilot CLI `mcpServers` 不同；客户端版本/传输/认证差异真实存在；服务端审批不可省。

**替代**：只提供 stdio 本地服务器（远程/团队场景不足）；只提供 REST（Copilot 不直接消费）。

**风险**：客户端协议/配置变化；审批绕过；工具滥用。缓解：版本化适配器、真实测试、服务端强制审批、限流/审计。

**实施**：PoC B 用 VS Code/Copilot + 另一个客户端；生成配置样例/兼容矩阵；不承诺万能配置。

## 9. MCP Gateway 和本地 MCP Server 应如何设计？

**推荐**：共享协议/工具描述/权限模型；Gateway 面向远端多租户（OAuth/限流/审计/审批），本地 Server 面向 stdio/loopback（能力令牌/离线/本地工具）。两者都不直接暴露内部 REST。

**依据**：远端与本地威胁模型/认证/部署不同；但工具 schema/风险/审批语义应一致。Kestra/Flowise 的 MCP 产品化证明“平台既是 Server 又是 Client”可行。

**替代**：只做 Gateway（本地不可用）；只做本地 Server（团队/外部客户端弱）；直接映射 REST（工具过多/权限失控）。

**风险**：双实现漂移；协议版本；本地令牌泄漏。缓解：共享 Registry/SDK、conformance、令牌短期/可撤销。

**实施**：Phase 3 先 Gateway + 本地 Server 最小；PoC B 验证；工具由能力注册生成。

## 10. 如何实现云端 Control Plane + 本地 Worker？

**推荐**：Worker 主动出站 mTLS 注册/心跳/租约/拉取任务/上报事件；云端只下发任务元数据/短期凭证；Worker 本地执行/凭证隔离；离线队列/租约重排/幂等。

**依据**：客户内网不应开放入站；Hybrid 需要断线恢复/隔离。Temporal/Activepieces 的 worker 模式可借鉴，但协议自研以匹配 IR/权限。

**替代**：云端反向 SSH/端口映射（安全/运维差）；Worker 轮询 HTTP 无租约（重复/丢失）；VPN 全互联（重）。

**风险**：重复副作用/租约/网络分区/凭证。缓解：幂等键、租约 TTL、事件 sequence、离线队列、凭证本地。

**实施**：PoC C 最小闭环；先本地 Worker + 假云；再私有/云。

## 11. 如何实现 Local、Private、SaaS 三种部署模式？

**推荐**：同一 IR/契约/运行时核心；Local 嵌入式（SQLite/文件）、Private 容器化（Postgres/Redis/S3/Worker）、SaaS 多租户（RLS/配额/计量）+ 可选私有 Worker。差异只在部署/认证/运维。

**依据**：语义一致是产品承诺；三套实现会漂移。开源项目通常只强于一种模式。

**替代**：三套代码（成本/漂移）；只做 SaaS（本地优先失败）；只做本地（企业/SaaS 失败）。

**风险**：本地沙箱/云多租户/运维复杂度。缓解：契约测试、同一 fixture 跨模式、Phase 6 再做 SaaS 治理。

**实施**：Phase 1 Local/Worker 同核；Phase 6 Private/SaaS；Hybrid Worker 复用。

## 12. 如何防止 Skill、Plugin 和 MCP Tool 的恶意执行？

**推荐**：来源信任 + 签名/完整性 + 最小权限 + 沙箱（容器/OS）+ 命令/文件/网络策略 + 高风险审批 + Prompt injection 防护 + 审计/限流 + 供应链 SBOM/审核。

**依据**：三类都是不可信内容/代码；单靠静态扫描/客户端确认不足。Dify 的 Landlock/Activepieces isolated-vm 说明运行时隔离必要。

**替代**：仅靠签名（签名不保证行为）；仅靠审核（绕过/供应链）；仅靠客户端确认（可绕过）。

**风险**：沙箱逃逸/本地弱隔离/供应链。缓解：高风险容器、默认拒绝网络、运行时监控、撤回/事件响应。

**实施**：Phase 2 Skill 沙箱；Phase 3 MCP 审批/SSRF；Phase 5 Plugin 签名/审核；安全测试门禁。

## 13. 如何避免 Workflow、Agent、Skill、MCP 重复定义能力？

**推荐**：统一 Capability Registry：一个实现/描述，多入口 Binding（Workflow Node/Agent Tool/MCP Tool/Skill/Plugin）；每个入口独立权限/审批；依赖/版本/关系图。

**依据**：重复定义会导致 schema/权限/版本漂移；开源项目各自生态的碎片化是反例。能力注册是平台核心差异。

**替代**：各模块独立注册表（短期快，长期重复）；只做 MCP 工具（Workflow/Agent 入口缺失）。

**风险**：过度抽象/入口策略复杂。缓解：只抽象描述/绑定，不抽象实现；入口策略显式；测试矩阵。

**实施**：Phase 1 基础 Capability/Binding；Phase 3/4 扩展；UI 关系图。

## 14. 哪些开源项目可以直接复用？

**推荐**：

- **Direct Reuse**：OpenTelemetry、Agent Skills 标准、MCP 规范、awesome-copilot（测试语料）。
- **Integration**：Temporal（durable）、LangGraph Python（Agent）、LiteLLM（模型网关）、Rust RMCP（MCP 协议）、MCP Servers（测试）。
- **Adaptation**：graphon（可选 Python 引擎）、Activepieces Piece/Waitpoint 概念、Flowise MCP 双向模式。
- **Design Reference**：Dify、n8n、Kestra、Flowable、Node-RED、Flowise、Airflow、Dagster。

**依据**：许可证、技术栈、模型契合度、维护成本；详见 [复用策略](../01-research/reuse-strategy.md)。

**替代**：直接嵌入 n8n/Dify/Kestra（许可/模型/栈不匹配）。

**风险**：许可/版本/供应链。缓解：SBOM/门禁/锁定/法务。

**实施**：按 [复用策略](../01-research/reuse-strategy.md) 的分类执行；任何新依赖记录用途/许可/退出方案。

## 15. 哪些模块值得自主开发，形成平台核心竞争力？

**推荐**：

1. Canonical IR + 编译器/校验器/静态分析。
2. 轻量 Workflow Runtime + 恢复/审批/事件。
3. 统一 Capability Registry + 多入口权限/审批。
4. Skill Runtime + 本地沙箱 + 标准导入/导出。
5. MCP Gateway + 2026-07-28 兼容层 + 工具治理。
6. Hybrid Worker 协议/凭证隔离。
7. 现代开发者工作台（Designer/Agent/Skill/MCP/Execution UI）。
8. 治理/审计/市场/供应链。

**依据**：这些是产品差异与许可/生态壁垒；成熟组件（模型网关/durable/Agent 状态图/可观测）可复用。

**替代**：全部自研（慢/重复）；全部依赖开源（无差异/受许可限制）。

**风险**：范围过大/质量不足。缓解：Phase/M0、复用边界、测试/安全门禁、团队配置。

**实施**：Phase 1 先 IR/运行时/Designer；后续按路线图；不自研模型/durable/可观测。

## 总结

Hisha 的核心不是“把开源项目拼起来”，而是：**一个 IR、一个能力注册、一个执行契约、一个治理层**，把成熟组件放在正确的位置。
