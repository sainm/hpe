# MVP 最小技术栈

> “MVP”指 **Phase 1 本地 Workflow MVP**：证明 Canonical IR + 本地执行 + 现代 Designer + 无云运行。Skill/MCP/Agent/Plugin 是后续增量，不进入 M0。

## 1. M0 最小栈

### Rust（本地运行时）

| 依赖 | 版本 | 用途 |
| --- | --- | --- |
| Rust | 1.99.x（MSRV 1.97） | 工具链 |
| tokio | 1.54.1 | async/process/time/sync |
| axum | 0.8.9 | 本地 API |
| serde / serde_json | 1.0.229 | JSON/配置 |
| saphyr | 0.3.3 | YAML 1.2/SourceMap |
| jaq-core / jaq-std / jaq-json | 3.1.2 / 3.0.4 / 2.0.3 | jq 表达式 |
| jsonschema | 0.60.2 | IR 校验（禁用远程 ref） |
| sqlx | 0.9.0（sqlite-bundled） | SQLite/迁移 |
| tracing + tracing-subscriber | 0.1.44 | 日志/span |
| thiserror | 2.0.17 | 错误 |
| rust-embed | 8.13.0 | 嵌入 Web UI |
| clap | 4.6.7 | CLI |
| uuid / time | 1.29.0 / 0.3.55 | ID/时间 |
| 静态 OpenAPI/JSON Schema | 仓库契约 | `/api/openapi.json` 与契约门禁；当前 TS DTO 仍为手写 |

不引入：Tonic/tonic-prost、Wasmtime、RMCP、Keyring（先用本地文件加密）、Temporal、Redis、MQ、Python。

### TypeScript（Web）

| 依赖 | 版本 | 用途 |
| --- | --- | --- |
| Node.js | 26.13 LTS | 构建/开发 |
| TypeScript | 5.9.3 | 类型 |
| Vite | 8.3.5 | 构建 |
| React / React DOM | 19.3.1 | UI |
| @vitejs/plugin-react | 6.1.2 | React |
| @xyflow/react | 12.12.0 | Canvas |
| monaco-editor / @monaco-editor/react | 0.57.0 / 4.7.0 | YAML/JSON |
| TanStack Query / Router | 5.105.0 / 1.170.46 | 数据/路由 |
| Zustand | 5.0.16 | 编辑器状态 |
| Zod / React Hook Form / resolvers | 4.6.7 / 7.91.0 / 7.0.0 | 表单/校验 |
| Tailwind CSS + @tailwindcss/postcss | 4.3.3 | 样式 |
| PostCSS | 8.5.32 | CSS 管线 |
| CSS Modules | Vite 内置 | 复杂组件 |
| radix-ui + shadcn | 1.8.1 / CLI 4.23.0 | UI primitives |
| class-variance-authority / clsx / tailwind-merge | 0.7.1 / 2.1.1 / 3.7.0 | variants |
| lucide-react | 1.66.0 | 图标 |
| i18next / react-i18next | 26.5.12 / 17.1.9 | ja/en/zh |
| openapi-typescript / openapi-fetch | 7.13.0 / 0.17.0 | 类型/客户端 |
| Vitest / Playwright | 以 lockfile 为准 | 测试 |

不引入：NestJS、Prisma、Redis、BullMQ、Python、LiteLLM、MCP、WASM、Temporal、NATS。

### 契约

- IR：`schema/ir/v0.1/workflow.schema.json` 与 `node-registry.schema.json`（JSON Schema 2020-12）。
- Local API：`schema/openapi/local-api.json`（OpenAPI 3.1 静态核心契约），运行时 `/api/openapi.json` 暴露；当前 TS DTO 手写。
- 其他契约：`schema/events/v1/event.schema.json`、`schema/capabilities/v1/tool.schema.json`。
- YAML SourceMap：`yaml_to_json_with_source_map` 返回 UTF-8 byte offset/line/column，并由 validation API 输出。
- 事件：JSON/SSE；本地 SQLite。
- 无 Proto/gRPC；本地单进程。

## 2. M0 范围

- Workflow Designer：Canvas/Palette/Inspector/Debugger；YAML/JSON 面板。
- 节点：Start/End/Condition/Switch/Loop/Parallel/Join/HTTP/Script/Transform/SetVariable/Human Approval/Subflow。
- IR/编译器/校验器/轻量运行时；检查点/恢复/审批；HTTP 与 HTTPS、网络权限与 SSRF 防护。
- Node Registry（13 类节点、`typeVersion` 解析与迁移）；五类校验；发布版本冻结与按版本执行。
- 并行分支真并发与分支级检查点恢复；审批 SLA 自动超时；工作流级 `errorHandling.graph`。
- SQLite 存储；artifact/log；本地 API/UI；i18n；设计 tokens/CSS；100 节点性能基准。
- 无 AI、无云、无 MCP、无 Skill Center、无插件。

## 3. M0 验收

- Canvas/YAML/JSON 语义往返 fixture 100%。
- 分支/循环/并行（真并发）/重试/超时/错误/审批（SLA 自动超时）/子流程/错误处理图可执行。
- 五类校验阻止非法发布；发布版本冻结（同版本不可改写 IR，记录 Node Registry 版本）。
- HTTP 主机白名单、私网拦截、关闭重定向；脚本仅 jq。
- 断网运行；崩溃恢复（检查点），并行分支不会重放已完成分支。
- 100 节点工作流编译与执行基准通过（debug 构建：编译约 20ms、执行约 44ms）。
- 607px/三语/键盘可用。
- 无 n8n/Dify/EE/专有代码；许可证门禁通过。

## 3b. Phase 2 本地 Skill 层

### 范围

- `hpa-skill` crate：`SKILL.md` frontmatter、可选 `hpa.skill.json`、未知字段保留、路径/符号链接/大小限制、脚本/测试/依赖/许可校验。
- Skill Registry：SQLite 保存不可变版本快照（文件 BLOB + 内容哈希）、发布状态、测试与运行记录；同版本不同内容拒绝。
- 确定性执行：Python/Node/Bash/Sh；独立工作区、输入/快照只读、artifacts 可写、argv 占位符、环境/命令白名单、超时/输出/产物/CPU 限制、进程组终止。
- OS 沙箱：macOS `sandbox-exec` 拒绝网络、工作区外写入与 `/Users` 读取；Linux 有 `bwrap` 时提供网络/文件系统隔离，否则明确标记为进程级隔离。
- 产物与审计：内容寻址写入 `artifacts/`，审计 `logs/audit.jsonl`；运行记录/产物可通过 API/UI 查看。
- Skill Center：导入、详情、校验报告、测试、运行控制台、发布、导出；导出标准目录、Claude Code 与 Codex 路径。
- 本地 MCP：stdio `initialize`/`server/discover`/`tools/list`/`tools/call`；已发布 Skill 脚本作为工具；CLI probe/call 客户端。

### 验收

- 断网本地导入→校验→测试→运行→产物→导出闭环通过（CLI/API/Web）。
- 沙箱负向测试：网络、工作区外写入、用户主目录读取、超时、输出上限、命令白名单、输入 Schema 均被拒绝。
- 无许可/专有 Skill 拒绝发布与导出；未知字段、`LICENSE`/`NOTICE` 导出保留。
- 版本冻结：同 `skillId@version` 不同内容返回 `SKILL_VERSION_FROZEN`。
- MCP stdio 端到端测试通过；导出目录写入临时 HOME 的 `.claude/skills/` 与 `$CODEX_HOME/skills/`，外部客户端实际会话加载未自动执行。
- Phase 1 回归全部通过（60 passed，1 ignored）。

## 3c. Phase 3 本地 MCP 生态

### 范围

- 传输：stdio + loopback Streamable HTTP；2026-07-28 无状态 POST，兼容 2025-11-25 `initialize`/会话。
- 协议：`server/discover`、`resultType`、cache 字段、MRTR `input_required`、tools/resources/prompts；`MCP-Protocol-Version`/`Mcp-Method`/`Mcp-Name`/Origin 校验。
- 注册表：已发布 Skill 脚本、`workflow.execute`、`workflow.list`、`execution.get`；Skill/Workflow/Execution/Artifact resources；Skill/Workflow prompts。
- 认证与治理：PAT/stdio 能力令牌（scope）、OAuth 2.1 资源元数据、高风险审批、per-token 限流、审计、监控。
- MCP Client：stdio/HTTP、modern→legacy 回退、主机白名单与 SSRF 防护、工具白名单、无凭证透传。
- MCP Center：工具/Prompt/Resource、调用、审批、PAT、外部服务器探测/调用、审计；607px/三语。

### 验收

- HTTP 集成测试：modern discover、legacy 会话、401、header mismatch、Origin、工具调用、资源元数据。
- 限流返回 429 + `Retry-After`；审计记录请求与工具调用。
- MRTR 审批闭环（API/Web）：input_required → 本地审批 → 重试执行；参数哈希绑定、TTL、一次性消费。
- MCP Client 对 stdio/HTTP 外部服务器探测并完成只读调用；SSRF/metadata 地址拒绝。
- Claude Code `claude mcp list` 连接并完成 initialize/tools/list；Codex 官方配置通过 `codex debug prompt-input` 连接并发现工具。
- 全量回归通过；具体计数以当前测试输出与 [Phase 13–18 状态台账](../10-quality/phase-13-18-status.md) 为准。

## 3d. Phase 4 本地 Agent Platform

### 范围

- Agent IR：`hpa.agent/v1`、`hpa.agent.session/v1`；不可变 AgentVersion（规范化 JSON + SHA-256）；发布/会话/事件/审批表。
- Runtime：确定性 planner 与可选 OpenAI 兼容 planner；Skill/Workflow/MCP 工具；参数解析；步骤/工具/时间/token/成本/深度预算；重复计划防护；检查点与 restart recovery。
- 审批：高风险工具 Agent 审批（TTL、一次性、可恢复）；Workflow 内审批保持独立，不能被 Agent 审批代替。
- 多 Agent：声明式 subAgents、handoff、预算继承、深度限制、子会话引用；子 Agent 审批失败关闭。
- 回放：确定性 planner trace replay；模型 planner 审计但不承诺回放。
- Agent Center：Agent 列表、运行、设计器、版本、会话、事件、审批、工具目录；ja/en/zh；1440/607。

### 验收

- 版本冻结、确定性 Skill 工具、trace replay 一致。
- Agent 审批→Workflow 执行→Workflow 独立审批保留；重启后等待审批与会话保留。
- handoff 预算继承与深度失败；OpenAI mock 工具调用、token/成本、循环检测、网络允许列表。
- interrupted 会话从检查点继续。
- 全量回归通过（71 passed，1 ignored）；Web typecheck/Vitest/build、品牌与 IR 门禁通过。

## 3e. Phase 5 本地 Plugin & Connector Ecosystem

### 范围

- Plugin：`hpa.plugin/v1` manifest、tar `.hplugin`、内容 SHA-256、Ed25519 签名/信任、依赖/兼容性/许可检查、内容存储与安装/升级/回滚/启停/卸载；插件贡献 Connector/MCP/Skill/Agent。
- Marketplace：本地 SQLite listing、发布/审核/评分/撤回、下载计数；无远程服务。
- Connector：`hpa.connector/v1` 声明式 HTTP/File 核心连接器；Connection、环境变量凭证引用、测试/调用、输入 Schema、限流/重试/超时、SSRF/私网/路径逃逸防护、审计。
- Agent 集成：`connector.<connectionId>.<operation>` 工具；所有 Connector 工具按高风险审批；不绕过 Workflow 审批。
- UI：Plugin Center、Marketplace、Connector Center；1440/607；ja/en/zh。
- SBOM：文件哈希、能力/依赖/权限、license/signature/staticScan gates；无 CVE 数据库/远程漏洞源。

### 验收

- 11 个 Phase 5 集成测试覆盖签名/篡改/许可/路径/依赖、升级/回滚、市场审核/评分/撤回、HTTP 限流/重试/SSRF、File 边界、Agent Connector 审批。
- 全量回归 88 passed / 1 ignored；Web typecheck、9 个 Vitest、build、品牌与 IR 门禁通过。
- 1440/607 截图见 `docs/assets/phase5/`；实际浏览器调用 File 连接器。

### 边界

- 基础 MVP 为声明式插件；P13 已增加远程/离线 Marketplace、发布者 key 轮换、DB/Git/MQ/S3 Connector、OSV 漏洞门禁和 key-based cosign/OCI digest provenance。仍未实现 Node/TS handler、Fulcio/Rekor keyless、OCI Registry attestation 拉取或当前 SQLite 生产模式的租户隔离切换。
- 限流为进程内存；DNS 只校验一次；插件 permissions 不是通用运行时沙箱。

## 3f. Phase 6 本地 Enterprise 基座

### 范围

- 控制面：`control.db` 的租户、配额、API 令牌哈希、OIDC 成员目录、审计、Worker、备份记录与 OIDC 配置。
- 隔离：内置 `local` 使用 `<data_dir>/hpa.db`，其他租户使用 `<data_dir>/tenants/{tenantId}/hpa.db`；TenantGate 按令牌/OIDC 租户选择独立 Router，业务 API 路径不含租户 ID。
- 认证：`hpa_ent_` 本地 API 令牌（SHA-256 哈希存储、一次性 secret、过期/吊销）与 OIDC EdDSA/Ed25519 JWT/JWKS；默认本地模式匿名映射为 `local` owner，`HPA_ENTERPRISE_MODE=1|true|required` 强制 Bearer。
- 授权：8 个固定角色，read/write/execute/approve 四个动作；平台管理员控制面令牌绕过租户 RBAC，但业务 API 仍绑定令牌租户。
- 成员目录：OIDC subject/email 匹配、角色覆盖、启用/禁用；未登记主体按 claims 认证。
- 配额：Workflow、日执行、Skill、Agent、插件、连接、并发运行；超限 429。
- 审计：控制操作与 TenantGate 写操作、拒绝事件；JSON/CSV 导出与公式注入转义。
- Worker：本地注册、心跳、30 秒离线派生、禁用；不派发任务。
- 备份：`VACUUM INTO`、SHA-256、恢复需重启、按天/数量保留、删除；租户删除清理成员/配额/Worker/备份/目录。
- UI：Enterprise Center 的租户、令牌、成员、配额、Worker、备份、审计、OIDC 面板；1440/607；ja/en/zh。

### 验收

- 9 个 HTTP 企业测试 + 3 个服务层测试：认证边界、跨租户 0、RBAC、令牌、成员 CRUD/过滤、配额/并发、审计/CSV、租户生命周期、备份恢复/清理、Worker 心跳/离线、OIDC 本地 JWKS。
- 全量回归 100 passed / 1 ignored；Web typecheck、9 个 Vitest、build、品牌与 IR 门禁通过。
- 1440/607 截图见 `docs/assets/phase6/`；浏览器中实际切换成员角色并由服务端回读。

### 边界

- 无 PostgreSQL RLS、HA/多节点、在线迁移、浏览器 SSO、远程/Hybrid Worker 派发、计费或租户暂停。
- 固定角色粗粒度授权；成员目录不提供密码、组、会话或邀请。
- 备份不含 `control.db`；恢复不重启进程；审计非 WORM/防篡改。
- `/mcp` 按 token 选择租户；`agent/skill/mcp/plugin/connector/run/doctor` CLI 仍直接操作本地租户。

## 3g. Phase 7 本地 VS Code & Copilot 集成

### 范围

- `apps/vscode-extension`：TypeScript 薄客户端，esbuild bundle，VS Code 1.143.0+，官方 MCP provider。
- 10 个 Tree View、Workflow 运行、执行 Webview/事件/审批/取消、产物 JSON 预览。
- Skill 安装/更新/移除到 `.github/skills`/个人目录；Runtime export API 写快照，扩展校验路径/符号链接并做备份回滚。
- SecretStorage 保存 MCP PAT 与 API token；工作区信任、离线、CSP nonce、消息白名单。
- ja/en/zh 运行时消息 + package.nls；VSIX 打包与临时目录安装。

### 验收

- 60 个 Vitest、7 个 Extension Host 检查通过；全量 Rust 101 passed / 1 ignored。
- VS Code 1.143.1：MCP `Auralis Local MCP` Running，4 tools 无命名警告；slug `workflow_list` 调用成功。
- Workflow `Approval` 从树运行→Webview 审批→`succeeded`；`invoice-audit` 安装到 `.github/skills`；Copilot Chat 能读取项目 Skill。
- 截图 `docs/assets/phase7/`：sidebar 1440/607、execution 1440、MCP servers 1440。

### 边界

- 无模型驱动的 MCP `tools/call` 验证、浏览器 SSO、Custom Instructions/Agents、远程 Worker、Marketplace 发布或自动升级。
- `/mcp` 的租户由服务端 token 决定；扩展宿主与 Runtime 必须共享文件系统。
- 固定 VS Code 1.143.1 实测；未建立跨版本矩阵。

## 3h. Phase 8 本地 Hybrid Worker

### 范围

- control.db：worker token（SHA-256）、能力/并发、job 队列、30s 租约、心跳续租、过期重排、协作取消、审批重排。
- `hpa worker` daemon：出站轮询、版本/能力匹配、复用同一 `hpa-core` compile/step、逐步事件/状态上报、指数退避。
- 租户库 `remote_executions` 标记，阻止本地 Runtime 恢复/审批扫描接管远程 execution；远程 Job 首次 start 后写 execution/events/artifacts。
- Worker Center：注册/一次性 token/轮换、能力与并发、派发、队列、取消；ja/en/zh、1440/607。

### 验收

- 全量 Rust 114 passed / 1 ignored；Worker queue 5、daemon 6、驱动隔离 1、真实 daemon 集成 1。
- 集成验证：租户/token 隔离、版本/能力/并发、租约过期与旧 lease 409、审批恢复、取消、远程执行持久化。
- 截图 `docs/assets/phase8/`；最终服务实际运行 `hybrid-worker-1`，队列包含成功与审批等待 Job。

### 边界

- 单节点 SQLite 控制面；无 Redis/多节点/HA/mTLS。
- daemon 使用 `<data_dir>/worker-outbox.db` 持久化上报；已完成但尚未确认删除的结果在重启后重放，未完成执行仍依赖租约过期重排，副作用至少一次。
- 控制面自动执行远程审批 SLA；没有远程凭证注入/对象存储；不派发 Agent/Plugin 等非 Workflow 任务。
- Worker 与控制面版本必须匹配；能力当前为 `workflow`。

## 4. M0 构建/运行

```bash
rustup toolchain install 1.99.1
(pnpm install --frozen-lockfile && pnpm --filter @hpa/web build)
cargo run -p hpa-local -- serve
(pnpm --filter @hpa/web dev)
cargo test --workspace
(pnpm --filter @hpa/web test)
hpa skill import fixtures/skills/invoice-audit
hpa skill test skill_invoice_audit
hpa skill run skill_invoice_audit --input '{"invoiceId":"inv_1","items":[]}'
hpa skill export skill_invoice_audit --target claude
hpa mcp probe -- hpa mcp serve
```

- 前端构建产物由 `rust-embed` 嵌入 local binary；开发模式读取 `apps/web/dist/`。
- `hpa validate <file>` 与 `hpa run <file> --input '<json>'` 可用于 CI。
- `hpa skill validate|import|test|run|publish|export` 与 `hpa mcp serve|probe|call` 支持离线本地闭环。
- 本地 API 127.0.0.1；数据目录平台标准；`hpa doctor`。

## 5. 增量 M1（团队/云端）

加入：

- NestJS 12.3.2 Control Plane + Postgres + Redis + Prisma 7.10.1（control schema）。
- Tonic/Tonic-prost + Proto；NestJS ↔ Rust gRPC。
- docker-compose/Helm；OTel Collector。
- Skill Center/Skill Runtime（Rust）；Python/Node/Shell 脚本沙箱。
- 不引入 Temporal/WASM/RMCP，除非 PoC 通过。

## 6. 增量 M2（差异化）

加入：

- RMCP 3.8（Rust MCP Gateway/Client/Server；2026-07-28 + legacy）。
- Wasmtime 50（WASM 插件/Skill）。
- Python AI Worker（LangGraph 1.2.19 + gRPC）。
- LiteLLM（模型网关）。
- Temporal（仅 durable PoC 通过后）。
- Plugin/Marketplace/签名。

## 7. 不进入 MVP 的技术

| 技术 | 原因 | 何时 |
| --- | --- | --- |
| Temporal | 长任务/重放；MVP 标准流程不需要 | Phase 3 PoC |
| Wasmtime | 插件沙箱；MVP 无第三方插件 | M2 |
| RMCP | MCP 是 Phase 3 | M2 |
| Python/LangGraph | Agent 是 Phase 4 | M2 |
| NATS/Kafka | 单机/小团队不需要 | Phase 6 |
| Prisma/Redis | M0 单机 SQLite | M1 |
| TypeScript 7 | 工具链风险 | 评估后 |
| Sass/Lightning CSS | 非必要 | 按需 |
| Kestra/Temporal 双引擎 | 复杂度 | 禁止 |

## 8. 团队/周期

- 2 Rust + 2 前端 + 1 架构/全栈；设计/安全按需。
- M0 约 8-12 周（与 Phase 1 一致）；先 PoC A。
- 关键路径：IR/编译器 → 运行时 → Designer → 校验/版本 → 执行调试。

## 9. 相关文档

- [语言策略](language-strategy.md)
- [Rust 技术栈](rust-stack.md)
- [TypeScript 技术栈](typescript-stack.md)
- [CSS 架构](css-architecture.md)
- [路线图](../11-roadmap/implementation-roadmap.md)
- [PoC A](../11-roadmap/poc-plan.md)
