# 多 Agent

> 多 Agent 不是默认架构。只有在任务可分解、角色边界清晰、单 Agent 预算/上下文不足时才使用；否则单 Agent + 工具更可靠。

## 1. 模式

| 模式 | 说明 | 适用 |
| --- | --- | --- |
| Supervisor | 一个主管规划/分派/汇总；Worker 执行子任务 | 复杂任务分解、统一预算/审批 |
| Handoff | Agent 把会话/任务交给另一个 Agent，附带上文 | 专业领域切换（售前→售后） |
| Pipeline | 固定顺序的 Agent 链（研究→写作→审校） | 可预测阶段；可用 Workflow 更确定 |
| Worker Pool | 多个同质 Worker 并行处理子任务 | 批量/并行 |
| Debate/Critic | 生成者 + 评审者 | 质量提升；成本高，慎用 |

> Pipeline/Debate 优先用 Workflow 表达确定性；Agent 只处理动态部分。

## 2. 合同

```ts
interface TaskEnvelope {
  taskId: string;
  parentSessionId?: string;
  fromAgent: AgentRef;
  toAgent: AgentRef;
  goal: string;
  input: unknown;
  constraints: { budget: Budget; deadlineMs: number; approvalPolicy: string; allowedTools?: string[] };
  context: { summary: string; artifacts?: ArtifactRef[] };
  traceContext: TraceContext;
}

interface ResultEnvelope {
  taskId: string;
  status: "completed" | "failed" | "blocked" | "needs_approval";
  output?: unknown;
  summary: string;
  artifacts?: ArtifactRef[];
  error?: AgentError;
  usage: { steps: number; tokens: number; costUsd: number; toolCalls: number };
}
```

规则：

- 任务输入/输出 schema 明确；上下文摘要有 token 上限。
- 子 Agent 预算 ≤ 父剩余预算；并发/深度限制。
- 子 Agent 不能获得父未授权的工具/数据；权限取交集。
- 结果必须结构化；失败/阻塞需说明原因。

## 3. 调度

- Supervisor 维护子任务表/状态；可并行/串行；超时/取消传播。
- Handoff：创建新 AgentSession，关联 parentSessionId；共享必要上下文（不共享 secret）。
- 子 Agent 失败：父可重试/重新分派/降级；预算计入父。
- 取消：父子传播；不可中断调用记录。
- 审批：子 Agent 触发的审批仍由 Hisha Approval Center 处理；不能由 Supervisor 代批。

## 4. 隔离

- 租户/工作区/项目隔离；子 Agent 不能跨租户。
- 工具权限交集；数据访问按任务范围最小化。
- 上下文不自动包含父会话全部消息；只传摘要/引用。
- 审计：parent/child session、task/result、预算、审批关联。

## 5. 何时不用多 Agent

- 任务可线性编排 → Workflow。
- 单 Agent 在预算内可完成 → 单 Agent。
- 需要确定性/强恢复 → Workflow + 审批。
- 角色边界不清/职责重叠 → 先理清流程。
- 成本/延迟敏感 → 单 Agent/小模型。

## 6. 反模式

- 多个 Agent 互相“讨论”无收敛条件 → 无限循环/成本爆炸。
- 子 Agent 继承父全部工具/数据 → 权限扩散。
- 把 Workflow 拆成多个 Agent 串行 → 不确定性与成本增加。
- Supervisor 让 Worker 自行审批 → 绕过治理。
- 没有预算/深度限制 → 递归/资源耗尽。
- 用多 Agent 弥补模型能力不足 → 先改 Prompt/工具/流程。

## 7. 可观测

- Trace：parent/child spans；task/result 事件；预算/工具/审批。
- 图：Agent 关系/调用次数/失败率/成本。
- 告警：递归深度、预算耗尽、子 Agent 失败率高、审批堆积。

## 8. 测试

- 任务分解/分派/汇总（mock 模型）。
- 预算继承/超限/并发/深度。
- 权限交集/跨租户拒绝。
- 失败/超时/取消传播。
- 审批/恢复。
- 上下文摘要不泄漏 secret/其他会话。

## 9. 相关文档

- [Agent 架构](agent-architecture.md)
- [Agent Runtime](agent-runtime.md)
- [Workflow 执行引擎](../03-workflow/execution-engine.md)
- [权限模型](../08-platform/permission-model.md)
