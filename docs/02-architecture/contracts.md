# 统一契约

> 接口契约比具体框架重要。所有执行后端、入口、插件必须实现这些契约；任何偏离需 ADR。
> 本文的 TypeScript 片段是概念契约；实际 wire format 以 Proto/JSON Schema/OpenAPI 为准，由 Rust/TS/Python 生成。

## 1. IR Contract

**输入**：Canvas/ YAML/JSON/ AI 生成。
**输出**：Canonical IR（不可变版本）。

不变式：

1. 每个节点有唯一 `id`，类型 + `typeVersion`。
2. 边只能连接已声明端口；无悬空边。
3. 容器节点（loop/parallel/branch/subflow）拥有子图；子图是 DAG（循环由容器语义实现）。
4. 变量作用域显式：input/variable/node output/scope/secret ref；禁止隐式全局。
5. 表达式是纯函数（jq 子集），无 I/O/环境/时间访问；禁止任意代码。
6. Secret 只能是引用，不是值。
7. 发布版本冻结所有节点类型版本、依赖、权限摘要。
8. 布局元数据不参与执行语义。
9. 未知扩展字段保留（前向兼容），但执行器忽略未知语义。

## 2. Runtime Contract

> Rust Core 以 `hpa.runtime.v1` gRPC 实现；下面是语言无关概念契约。TS/Python 只使用生成的客户端，不共享内部类型。

```ts
interface RuntimeAdapter {
  start(spec: ExecutionSpec): Promise<ExecutionHandle>;
  resume(id: string, cmd: ResumeCommand): Promise<void>;
  cancel(id: string, reason: string): Promise<void>;
  approve(id: string, approvalId: string, decision: ApprovalDecision): Promise<void>;
  query(id: string): Promise<ExecutionSnapshot>;
  stream(id: string, fromEventId?: string): AsyncIterable<ExecutionEvent>;
  capabilities(): RuntimeCapabilities;
}

type ResumeCommand =
  | { kind: "approval"; approvalId: string; decision: ApprovalDecision }
  | { kind: "signal"; name: string; payload: unknown }
  | { kind: "retry"; nodeId: string }
  | { kind: "cancel" };

interface RuntimeCapabilities {
  durability: "checkpoint" | "event-sourced";
  maxDurationMs?: number;
  approvals: boolean;
  signals: boolean;
  artifacts: boolean;
  parallel: boolean;
}
```

事件不变式：追加式、每执行单调 sequence、至少一次投递、消费者幂等；大对象只存引用。

## 3. Skill Contract

```ts
interface SkillPackage {
  manifest: SkillManifest;        // hpa.skill.json（扩展）
  skillMd: string;                // 标准 SKILL.md
  files: SkillFile[];             // references/scripts/assets
}

interface SkillRuntime {
  import(pkg: SkillPackage, opts: ImportOptions): Promise<SkillVersion>;
  load(ref: SkillRef, ctx: SkillContext): Promise<LoadedSkill>;
  execute(req: SkillExecutionRequest): Promise<SkillExecutionResult>;
  export(ref: SkillRef, target: SkillExportTarget): Promise<SkillPackage>;
}
```

不变式：

- 标准字段/目录不变；`hpa.skill.json` 是扩展清单，缺失时仍可加载。
- 脚本在沙箱执行；默认无网络/只读/临时写。
- Skill 指令不得提升权限；Agent Runtime 将其视为不可信数据。
- 版本不可变；执行记录绑定 SkillVersion + ScriptVersion。
- 导出保留未知字段/许可/NOTICE；拒绝专有/无许可内容。

## 4. MCP Contract

```ts
interface McpGateway {
  discover(req: DiscoverRequest): Promise<ServerDescriptor>;
  listTools(tenant: TenantContext): Promise<ToolDescriptor[]>;
  callTool(req: ToolCallRequest, ctx: TenantContext): Promise<ToolCallResult>;
  listResources(...): Promise<ResourceDescriptor[]>;
  readResource(...): Promise<ResourceContent>;
  listPrompts(...): Promise<PromptDescriptor[]>;
  getPrompt(...): Promise<PromptContent>;
}
```

不变式：

- 支持 2026-07-28（无状态、`_meta`、`resultType`、discover、MRTR、subscriptions、tasks）与旧版协商。
- 每个工具/资源/Prompt 有 schema、权限、风险等级、审批策略。
- 不暴露内部 REST 全集；工具由 Capability Registry 生成。
- 高风险写操作服务端强制审批；客户端提示不可信。
- 认证：OAuth 2.1/PAT/stdio env；不 token passthrough。
- 租户隔离：token + tenant + 资源归属；列表结果按租户过滤。
- 结果 `ttlMs`/`cacheScope` 正确；租户私有数据 `private`。

## 5. Plugin Contract

```ts
interface PluginManifest {
  schemaVersion: "hpa.plugin/v1";
  id: string;
  name: string;
  version: string;
  license: string;
  compatibility: { hisha: string; os?: string[]; runtimes?: string[] };
  capabilities: CapabilityContribution[];
  dependencies: PluginDependency[];
  permissions: PermissionRequest[];
  integrity: { algorithm: "sha256"; digest: string };
  signature?: { algorithm: "ed25519" | "sigstore"; keyId: string; value: string };
}
```

不变式：

- 安装前校验 manifest/schema/签名/依赖/许可/权限。
- 插件内容只进入内容寻址存储；不在 API 进程加载代码。
- 升级/卸载做依赖影响分析；不可破坏其他能力。
- 插件可包含 Skills/MCP 配置/Agents/Node/Connector/Scripts/文档，但每类仍遵循各自契约。
- 市场来源信任/审核/下架；未知来源默认不可信。

## 6. Connector Contract

```ts
interface Connector {
  metadata(): ConnectorMetadata;          // id/version/auth/schema/docs
  test(ctx: ConnectorContext): Promise<TestResult>;
  operations(): ConnectorOperation[];     // name/input/output/schema
  invoke(op: string, args: unknown, ctx: ConnectorContext): Promise<unknown>;
}
```

不变式：

- 认证/凭证由 Vault 注入；连接器不持久化明文。
- 所有 HTTP 出站走 SSRF 防护/白名单；重定向校验。
- 超时/重试/限流/幂等键由运行时与连接器共同实现。
- 错误分类：可重试/不可重试/认证/权限/限流/超时。
- 连接器可绑定为 Workflow Node 与 Agent Tool，但权限/审批独立。

## 7. Permission Contract

```ts
interface PolicyDecision {
  effect: "allow" | "deny" | "require_approval";
  reasons: string[];
  obligations?: { approvalId?: string; ttlMs?: number; masked?: string[] };
}

interface PolicyEngine {
  evaluate(req: PolicyRequest): Promise<PolicyDecision>;
}
```

不变式：

- 每个入口（API/Node/Tool/MCP/Skill/Connector/Plugin）独立评估。
- 默认拒绝；高风险写操作默认 require_approval。
- 决策包含原因/义务；审计记录决策摘要。
- 租户/工作区/项目/资源作用域强制；跨租户直接拒绝。
- 策略可版本化；发布/执行快照绑定策略版本。

## 8. Execution Contract

```ts
interface ExecutionSpec {
  executionId: string;
  tenantId: string;
  workspaceId: string;
  projectId: string;
  workflowVersionId: string;
  ir: CanonicalIr;
  inputs: Record<string, unknown>;
  policySnapshot: PolicySnapshot;
  budget: Budget;
  traceContext: TraceContext;
}

interface ExecutionEvent {
  eventId: string;
  executionId: string;
  sequence: number;
  type: ExecutionEventType;
  nodeId?: string;
  attempt?: number;
  at: string;
  traceId?: string;
  data: Record<string, unknown>;
}
```

不变式：

- Execution 绑定 WorkflowVersion；不得执行草稿。
- 每次尝试独立 NodeExecution；重试不改写历史。
- 外部调用至少一次 + 幂等键；不承诺 Exactly Once。
- 事件追加；审计独立；产物内容寻址。
- 取消协作式；不可中断调用记录“可能已发生”。

## 9. Capability Contract

```ts
interface Capability {
  id: string;
  type: "workflow" | "agent" | "skill" | "tool" | "connector" | "node" | "trigger" | "mcp_server" | "plugin";
  version: string;
  name: string;
  description: string;
  inputSchema: JsonSchema;
  outputSchema: JsonSchema;
  dependencies: CapabilityDependency[];
  permissions: PermissionRequest[];
  runtime: RuntimeRequirement;
  owner: string;
  visibility: "private" | "workspace" | "tenant" | "public";
  status: "draft" | "published" | "deprecated" | "disabled";
  documentation?: string;
  bindings: CapabilityBinding[];
}
```

不变式：

- 一个实现只注册一次；多入口是 Binding，不是复制。
- 每个 Binding 有独立权限/策略/审批。
- 依赖解析在发布时完成；运行时不得要求缺失依赖。
- 版本兼容检查；废弃需迁移/通知。
- 能力关系图可查询（依赖/被依赖/绑定/入口）。

## 10. 版本与兼容

| 契约 | 版本字段 | 兼容策略 |
| --- | --- | --- |
| IR | `schemaVersion` | 向后兼容读取；破坏性变更升版本 + 迁移 |
| Node Type | `typeVersion` | 发布冻结；旧版本保留执行 |
| Workflow | `workflowVersion` | 不可变快照；回滚指针 |
| Skill | `version`（semver） | 不可变；依赖范围 |
| Agent | `version` | 不可变；会话绑定版本 |
| Plugin | `version`（semver） | 依赖/升级/回滚 |
| Connector | `version` | 能力描述；执行时固定 |
| MCP | 协议版本 | 协商 + 兼容层 |
| Event | `schemaVersion` | 消费者兼容；新增字段可选 |

## 11. 错误模型

统一错误码：

- `IR_*`：schema/静态/编译
- `PERM_*`：权限/审批
- `DEP_*`：依赖/版本/凭证
- `EXEC_*`：执行/超时/取消
- `SANDBOX_*`：沙箱/资源
- `MCP_*`：协议/工具/认证
- `CONN_*`：外部系统/网络/限流
- `SKILL_*`：加载/校验/脚本
- `PLUGIN_*`：安装/签名/依赖
- `SYS_*`：内部/基础设施

错误响应：RFC 9457 `application/problem+json`，含 `code`、`detail`、`traceId`、`remediation`；不泄漏 secret/内部路径。

## 12. 相关文档

- [Workflow IR](../03-workflow/workflow-ir.md)
- [节点规范](../03-workflow/node-specification.md)
- [Skill 规范](../05-skill/skill-specification.md)
- [MCP 架构](../06-mcp/mcp-architecture.md)
- [插件架构](../07-plugin/plugin-architecture.md)
- [权限模型](../08-platform/permission-model.md)
