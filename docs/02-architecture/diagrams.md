# 图集

> 9 张关键图：系统架构、部署架构、Workflow 执行、Agent 调用、Skill 本地执行、MCP 通信、Hybrid Worker、核心 ER、页面导航。图使用 Mermaid，源文即文档。

## 1. 系统架构

```mermaid
flowchart TB
  U["Web / CLI / 外部 AI 客户端"] --> CP["Control Plane"]
  CP --> REG["Capability Registry"]
  CP --> WF["Workflow Service"]
  CP --> AG["Agent Service"]
  CP --> SK["Skill Service"]
  CP --> MC["MCP Gateway"]
  CP --> EX["Execution Service"]
  EX --> EP["Execution Plane"]
  EP --> RT["RuntimeAdapter"]
  RT --> NODE["Node Executor"]
  RT --> ART["Agent Runtime"]
  RT --> SKR["Skill Runtime"]
  RT --> MCPR["MCP Runtime"]
  RT --> CONN["Connector Runtime"]
  RT --> SCRIPT["Script Runtime"]
  CP --> PG[("PostgreSQL")]
  EP --> PG
  CP --> REDIS[("Redis")]
  EP --> S3[("Object Storage")]
  EP --> OBS["OpenTelemetry"]
  ART --> LLM["LiteLLM / Models"]
  RT -.-> TEMP["Temporal（可选）"]
```

详见 [系统架构](system-architecture.md)。

## 2. 部署架构

```mermaid
flowchart TB
  subgraph Local["Local"]
    LUI["localhost UI"] --> LRT["Local Runtime"]
    LRT --> LDB[("SQLite")]
    LRT --> LFS["artifacts/logs"]
  end
  subgraph Private["Private"]
    ING["Ingress"] --> API["API"]
    API --> PG[("Postgres")]
    API --> W["Workers"]
    W --> SBX["Sandbox"]
  end
  subgraph SaaS["SaaS"]
    EDGE["WAF"] --> CAPI["Multi-tenant API"]
    CAPI --> CPG[("Postgres RLS")]
    CAPI --> CW["Cloud Workers"]
    CAPI --> GW["MCP Gateway"]
  end
  subgraph Hybrid["Hybrid"]
    HW["Customer Worker"] -. "outbound mTLS" .-> CAPI
  end
```

详见 [部署架构](deployment-architecture.md)。

## 3. Workflow 执行时序

```mermaid
sequenceDiagram
  participant U as User/Trigger
  participant CP as Control Plane
  participant DB as Event Store
  participant W as Worker
  participant N as Node Executor
  participant X as External System
  U->>CP: start(workflowVersion, inputs)
  CP->>DB: create execution + event
  CP->>W: dispatch node
  W->>N: execute(node, input)
  N->>X: call with idempotencyKey
  X-->>N: result/error
  N-->>W: output
  W->>DB: append NodeExecution/events
  alt approval required
    W->>CP: request approval
    CP-->>U: Approval Center
    U->>CP: approve
    CP->>W: resume
  end
  W-->>CP: complete/fail
  CP->>DB: final event
  CP-->>U: stream result
```

详见 [执行引擎](../03-workflow/execution-engine.md)。

## 4. Agent 调用时序

```mermaid
sequenceDiagram
  participant U as User
  participant AR as Agent Runtime
  participant M as Model Gateway
  participant SR as Skill Runtime
  participant TR as Tool/MCP/Workflow
  participant AP as Approval
  U->>AR: task
  AR->>M: plan/select
  M-->>AR: plan/tool calls
  AR->>SR: load skill instructions
  SR-->>AR: context
  AR->>TR: invoke tool/workflow
  TR-->>AR: observation
  alt high risk
    TR->>AP: request approval
    AP-->>AR: approved/denied
  end
  AR->>M: evaluate/replan
  AR-->>U: result + trace
```

详见 [Agent Runtime](../04-agent/agent-runtime.md)。

## 5. Skill 本地执行时序

```mermaid
sequenceDiagram
  participant U as User
  participant LR as Local Runtime
  participant SL as Skill Loader
  participant SR as Skill Runtime
  participant SB as Sandbox
  participant FS as Workspace/Artifacts
  U->>LR: run skill
  LR->>SL: resolve skill version
  SL->>SL: parse SKILL.md + manifest
  SL-->>SR: loaded skill
  SR->>SB: execute script (policy)
  SB->>FS: read input/write artifacts
  SB-->>SR: output/error
  SR-->>LR: result + logs
  LR-->>U: show result
```

详见 [本地执行](../05-skill/local-execution.md)。

## 6. MCP 通信时序

```mermaid
sequenceDiagram
  participant C as External Client
  participant GW as MCP Gateway
  participant P as Policy
  participant T as Tool Executor
  participant A as Audit
  C->>GW: server/discover or initialize
  GW-->>C: versions/capabilities
  C->>GW: tools/list
  GW-->>C: tools + schemas
  C->>GW: tools/call
  GW->>P: evaluate tenant/tool/risk
  P-->>GW: allow/deny/approval
  GW->>T: execute
  T-->>GW: result
  GW->>A: audit
  GW-->>C: result
```

详见 [MCP 架构](../06-mcp/mcp-architecture.md)。

## 7. Hybrid Worker 时序

```mermaid
sequenceDiagram
  participant W as Private Worker
  participant CP as Control Plane
  participant J as Job/Lease
  participant X as Internal System
  W->>CP: register(token, capabilities)
  CP-->>W: worker credentials
  loop heartbeat
    W->>CP: heartbeat + capacity
  end
  CP->>J: enqueue job
  W->>CP: lease job
  W->>X: execute (local credentials)
  X-->>W: result
  W->>CP: events + result
  alt network down
    W->>W: queue locally
    W->>CP: replay on reconnect
    CP->>J: lease expiry/reassign
  end
```

详见 [部署架构](deployment-architecture.md) §5。

## 8. 核心实体关系

```mermaid
erDiagram
  TENANT ||--o{ WORKSPACE : contains
  WORKSPACE ||--o{ PROJECT : contains
  PROJECT ||--o{ WORKFLOW : owns
  WORKFLOW ||--o{ WORKFLOW_VERSION : publishes
  WORKFLOW_VERSION ||--o{ EXECUTION : binds
  EXECUTION ||--o{ NODE_EXECUTION : contains
  EXECUTION ||--o{ EXECUTION_EVENT : emits
  EXECUTION ||--o{ APPROVAL : requires
  EXECUTION ||--o{ ARTIFACT : produces
  PROJECT ||--o{ AGENT : owns
  AGENT ||--o{ AGENT_VERSION : publishes
  AGENT_VERSION ||--o{ AGENT_SESSION : runs
  AGENT_SESSION ||--o{ TOOL_CALL : records
  PROJECT ||--o{ SKILL : owns
  SKILL ||--o{ SKILL_VERSION : publishes
  SKILL_VERSION ||--o{ SKILL_EXECUTION : executes
  PROJECT ||--o{ MCP_SERVER : registers
  MCP_SERVER ||--o{ MCP_TOOL : exposes
  PROJECT ||--o{ CONNECTOR : registers
  CONNECTOR ||--o{ CREDENTIAL : uses
  PROJECT ||--o{ PLUGIN : installs
  PLUGIN ||--o{ CAPABILITY : contributes
  CAPABILITY ||--o{ CAPABILITY_BINDING : exposes
```

详见 [数据模型](../08-platform/data-model.md)。

## 9. 页面导航

```mermaid
flowchart LR
  HOME["Overview"] --> WF["Workflow Center"]
  WF --> DESIGNER["Workflow Designer"]
  HOME --> AG["Agent Center"]
  AG --> AGD["Agent Designer"]
  HOME --> SK["Skill Center"]
  SK --> SKE["Skill Editor"]
  SK --> SKT["Skill Test Console"]
  HOME --> MCP["MCP Center"]
  MCP --> MCPE["MCP Explorer"]
  HOME --> PL["Plugin Center"]
  PL --> MARKET["Marketplace"]
  HOME --> CAP["Capability Registry"]
  HOME --> CONN["Connections"]
  HOME --> EX["Execution Monitor"]
  HOME --> AP["Approval Center"]
  HOME --> LR["Local Runtime Manager"]
  HOME --> SET["Settings"]
```

详见 [信息架构](../09-ui/information-architecture.md)。

## 10. 相关文档

- [系统架构](system-architecture.md)
- [部署架构](deployment-architecture.md)
- [Workflow 执行引擎](../03-workflow/execution-engine.md)
- [Agent Runtime](../04-agent/agent-runtime.md)
- [Skill 本地执行](../05-skill/local-execution.md)
- [MCP 架构](../06-mcp/mcp-architecture.md)
- [数据模型](../08-platform/data-model.md)
- [信息架构](../09-ui/information-architecture.md)
