# Auralis（hpa）

Auralis 是一个统一的自动化与 AI 能力平台：用一套 Workflow IR 构建确定性流程，用 Agent 处理动态决策，用标准 Agent Skills 分发能力，通过 MCP 向外部 AI 客户端开放。当前仓库已交付 Local 与单控制面的 Hybrid/Private Worker 形态；SaaS 多节点形态仍是目标架构。

**Build Once. Execute Anywhere. Publish Everywhere.**
**Automation First. AI Optional. AI Native.**

显示名为 **Auralis**，永久内部 ID 为 **hpa**；Hisha 是历史别名。所有包名、二进制、数据目录、环境变量、扩展 ID 与 schema 授权均使用 `hpa`，显示名可独立更换。品牌规则见 [`BRANDING.md`](BRANDING.md)。

![Auralis Workflow Designer](docs/assets/m0/designer-1440.png)

![Auralis Skill Center](docs/assets/phase2/skills-1440.png)

![Auralis MCP Center](docs/assets/phase3/mcp-center-1440.png)

![Auralis Agent Center](docs/assets/phase4/agent-center-1440.png)

![Auralis Plugin Center](docs/assets/phase5/plugins-1440.png)

![Auralis Enterprise Center](docs/assets/phase6/enterprise-1440.png)

![Auralis VS Code extension](docs/assets/phase7/sidebar-1440.png)

![Auralis Hybrid Worker Center](docs/assets/phase8/workers-1440.png)

![Auralis Browser SSO login](docs/assets/phase9/login-1440.png)

## 当前状态

**Phase 1 至 Phase 12 的本地 MVP 子集已实现并通过当前自动化验收。** 这不等于原路线图的全部企业/云端退出标准；逐项差异见 [实现状态与验收边界](docs/10-quality/implementation-status.md)。产品代码位于 `crates/`、`apps/web/` 与 `apps/vscode-extension/`。

**Phase 14 至 Phase 18：Skill、MCP、Connector、Plugin 元数据与 Agent 会话租约已完成本地验收。** Skill 包与版本、文件、运行和产物元数据，MCP PAT、审批、审计与外部服务器元数据，Connector 定义、连接与审计，插件包、安装状态、能力、事件、本地/远程 Marketplace 与插件贡献的 Connector/MCP server，以及 Agent 会话 owner lease，在 `HPA_POSTGRES_RUNTIME=1` 时切到 PostgreSQL，支持跨节点读取、启停、发布、审核、评分、撤回、token/审批/连接管理、审计与单写者会话执行；插件包文件、签名私钥与安装目录仍在节点本地，本地 SQLite 模式不变。Phase 13 的其余生产级工作包仍为 IN_PROGRESS。PostgreSQL 18 已建立 schema、SQLx checksum migration、角色分离、68 张租户相关表的 FORCE RLS（42 张 `hpa_tenant` + 26 张 `hpa_control`）、幂等 SQLite 导入、outbox/fencing lease 和持久 MCP 状态原语；`HPA_POSTGRES_RUNTIME=1` 时 Workflow/Execution/Trigger Repository、execution owner lease、协作取消请求与执行幂等键和 remote marker 已切到 PostgreSQL，`HPA_POSTGRES_EVENT_BUS=1` 再启用 Topic Event outbox consumer。Worker 注册/心跳/停用与 Worker Job API、平台令牌、审计、浏览器会话、OAuth 2.1 AS、企业租户/配额/成员/SCIM Group、Agent 版本/会话/事件/审批/工具调用、Skill 包/版本元数据、MCP PAT/审批/审计/服务器元数据、Connector 定义/连接/审计与 Plugin 生命周期/Marketplace 元数据均已切到 PostgreSQL（PostgreSQL 模式下对应 SQLite 表保持 0 行或仅作缓存）；Phase 13 的其余工作包不能视为整阶段完成。

- Rust Core：Canonical Workflow IR、JSON Schema 校验、Saphyr YAML 解析/输出、jaq 表达式、编译器与执行计划；Schema/静态/表达式/依赖/权限与执行策略五类校验；Node Registry（13 类节点、`typeVersion` 解析、旧版 `http.request@0.9.0` 迁移）。
- Runtime：顺序/条件/多路分支/循环/并行节点、节点重试/退避、节点超时、错误回退、人工审批暂停与恢复、终态执行重试、运行中协作取消、内联子流程、HTTP/HTTPS 客户端、jq 脚本与变量节点；并行分支真并发（`maxConcurrency`），分支进度随检查点持久化；审批 SLA 到点自动按 `onTimeout` 恢复；启动和重试支持 `Idempotency-Key`；工作流级 `errorHandling.graph` 在未处理失败后执行并保留原始错误。
- 安全边界：HTTP 默认仅允许回环；外部主机必须在节点 `permissions.network` 显式声明，重定向关闭，URL 禁止 userinfo；DNS 解析后拦截私网地址，除非规则显式允许；脚本仅执行 jq，禁止 `env`/`input`/`input_filename` 等越界函数。
- 持久化：SQLite 保存工作流、发布版本、执行快照、追加事件与输出产物；发布版本冻结 IR 与 Node Registry 版本，同版本内容不可改写；执行可按发布版本启动；进程被强杀后可从最近检查点恢复。
- Local API：127.0.0.1 上的 JSON API、SSE 事件流、审批中心接口、节点注册表 `/api/nodes`、OpenAPI `/api/openapi.json`、YAML validation SourceMap、技能源码/签名接口与嵌入式 Web UI。
- Skill 包与 Registry：标准 `SKILL.md` + 可选 `hpa.skill.json`；未知字段、`LICENSE`/`NOTICE` 原样保留；路径逃逸、符号链接、设备文件、超限包被拒绝；Skill 版本按内容哈希冻结，重复版本不同内容返回 `SKILL_VERSION_FROZEN`。
- Skill Runtime：Rust 执行 Python/Node/Bash/Sh 确定性脚本；独立临时工作区、输入与快照只读、artifacts 可写、argv 数组、占位符替换、环境白名单、命令白名单、超时/输出/产物/CPU 限制、进程组终止；产物内容寻址，审计写入 `logs/audit.jsonl`。
- 沙箱：macOS 使用 seatbelt 拒绝网络、工作区外写入与用户主目录读取；Linux 在可用时使用 bubblewrap 隔离网络与文件系统；导出产物与执行日志留在本地。
- Skill Center：导入、源码草稿编辑、校验报告、测试控制台、确定性运行、本地 Ed25519 签名、发布、导出；编辑必须生成新版本，已有版本保持不可变。
- MCP Gateway：stdio + loopback Streamable HTTP；2026-07-28 无状态 `server/discover`、`resultType`、MRTR `input_required`、`subscriptions/listen` 与 tasks handle 查询，兼容 2025-11-25 `initialize`/会话和 GET SSE；PostgreSQL 模式持久化 session/subscription/task，跨节点 task update 通过 owner-node 命令和 lease 消费后触发 tasks SSE；`MCP-Protocol-Version`、`Mcp-Method`、`Mcp-Name` 与 Origin 校验。
- MCP 注册与权限：已发布 Skill 脚本、`workflow.execute`、`workflow.list`、`execution.get` 成为工具；resources 覆盖 Skill/Workflow/Execution/Artifact，prompts 提供 Skill 指导与 Workflow 审查；PAT/stdio 能力令牌按 `mcp:read`/`mcp:execute` scope 过滤。
- MCP 治理：高风险 `workflow.execute` 使用服务端审批 + MRTR 重试；审批绑定参数哈希、TTL 与一次性消费；按 token 限流（默认 120/min，可 `HPA_MCP_RATE_LIMIT` 覆盖）、结构化审计、MCP Center 监控。
- OAuth 资源元数据：`/.well-known/oauth-protected-resource` 与 401 `WWW-Authenticate` challenge；Phase 10 另提供内置 Authorization Code + PKCE 授权服务器。
- MCP Client：stdio 与 Streamable HTTP、modern→legacy 回退、工具白名单、主机白名单与 SSRF/metadata 地址防护、不向工具透传凭证；外部服务器可探测、浏览工具并调用。
- MCP Center：工具/Prompt/Resource 浏览器、调用控制台、审批、PAT 管理、外部服务器探测与调用、审计。
- Web：React 19 + Vite + XYFlow Canvas + Monaco YAML/JSON 编辑器 + 调试面板；节点注册表、网络权限、Skill Center、MCP Center、Agent Center、Plugin Center、Connector Center 与 Enterprise Center；浅色/深色/深海三主题；日语/英语/中文；607px 窄屏布局。
- Agent Platform：不可变 AgentVersion（`hpa.agent/v1`）、Agent Center/Designer/Test、会话与审批；Rust Agent Runtime 支持离线确定性 planner 与可选 OpenAI 兼容模型 planner；Skill/Workflow/MCP 工具调用、步骤/工具/token/成本/时间/深度预算、重复计划防护、检查点与 interrupt/resume、多 Agent handoff 预算继承、确定性 trace 回放；高风险工具强制审批，Workflow 审批保持独立。
- Plugin & Connector Ecosystem：`hpa.plugin/v1` 声明式插件包、tar `.hplugin`、内容哈希与 Ed25519 签名/信任、依赖/兼容性/许可检查、安装/升级/回滚/启停/卸载；本地 Marketplace 的发布/审核/评分/撤回，以及签名 HTTP/离线远程索引、发布者/索引 key 轮换与撤回传播、Remote Marketplace Web/VS Code source/publisher/key/install 管理、持久 SSE 撤回推送；`hpa.connector/v1` 的 HTTP/File/PostgreSQL/Git/Redis MQ/S3 连接器；插件可贡献 Connector/MCP/Skill/Agent，且不在 API 进程加载插件代码。
- Enterprise 基座：`control.db` 管理租户、API 令牌、OIDC/SCIM 成员目录、组角色映射、配额、Worker、备份与审计；每个租户使用独立 SQLite 与独立 API Router，TenantGate 按令牌租户路由，跨租户访问通过测试验证为 0；固定 8 角色 RBAC（`owner/admin/developer/publisher/operator/approver/auditor/viewer`）按 read/write/execute/approve 动作判定；OIDC JWT/JWKS Bearer（EdDSA/RS256/ES256）与 `hpa_ent_` 本地令牌并存；[SCIM 2.0](docs/08-platform/scim.md) Users/Groups 支持 filter、PATCH、ETag 与事务化角色映射；配额覆盖 Workflow/日执行/Skill/Agent/插件/连接/并发；审计支持 JSON/CSV 导出；Worker 注册/心跳/离线派生/禁用；租户备份、恢复、保留、删除与清理。默认本地模式匿名映射为 `local` 租户 owner；`HPA_ENTERPRISE_MODE=1|true|required` 强制 Bearer。
- VS Code & Copilot 集成：`apps/vscode-extension`（`hpa.workbench`）薄客户端，10 个 Tree View、状态栏、Runtime 发现/重连、Workflow 运行与执行 Webview、事件/审批/取消/重试、产物预览；Skill 安装/更新/移除到 `.github/skills` 或个人目录，备份回滚与本地修改检测；官方 `mcpServerDefinitionProviders` 注册本地 MCP Server，PAT 仅存 SecretStorage；`X-HPA-Tool-Name-Style: slug` 适配 VS Code 工具名限制。扩展不执行工作区脚本，未信任工作区只读。
- Hybrid Worker：`hpa worker` 出站轮询控制面，同一 Rust Runtime 执行 Workflow 快照；一次性 `hpa_wrk_` token（租户绑定、SHA-256）、能力/并发、30s 租约、心跳续租、租约过期重排、旧 lease 409、协作取消；start/events/complete 先写本地持久 outbox，崩溃重启后继续上报；远程审批 SLA 自动恢复并重排；首次 start 写回租户 execution/events/artifacts，`remote_executions` 阻止本地 Runtime 接管。
- Browser SSO：OIDC Authorization Code + PKCE（S256，public client）、state/nonce/redirect 校验、EdDSA/RS256/ES256 ID Token、JWKS/OKP/RSA/P-256；服务端 session 只存 token 哈希，refresh/ID token 使用 AES-256-GCM 加密，支持刷新轮换、静默续期和 RP-initiated logout；`hpa_session` HttpOnly/SameSite=Lax Cookie、CSRF header、成员/租户状态复核与平台管理员 allowlist；本地匿名模式与 Bearer/Worker/VS Code 认证不回归。
- 可观测性：`/metrics` Prometheus 文本（HTTP 路由/状态/延迟、执行与节点直方图、调度器心跳与延迟、触发器发火、跨租户聚合 gauge）、`/api/ready` 真依赖探针、W3C/OTLP traces 与 Rust→Python/Worker/MCP 跨进程 traceId、执行详情步骤时间线（最慢步骤高亮）与设置页运行时状态卡片。
- 触发器与调度：`schedule`（自研 5 字段 cron、固定偏移时区、skip/run_once/catch_up 漏跑策略、乐观锁单飞）、`webhook`（HMAC-SHA256 签名、±300s 重放窗口、`X-HPA-Event-Id` 幂等、进程内限流）和 topic `event`（`/api/events/{topic}` 发布、事件 ID 幂等）真正驱动已发布版本；`trigger_state`/`trigger_fires` 持久化，Triggers 控制台展示事件发布地址、下次触发、最近触发与启停。
- OAuth 2.1 授权服务器：内置 AS，Authorization Code + PKCE(S256)、动态客户端注册（RFC 7591）、刷新令牌轮换与复用检测、吊销（RFC 7009）与内省（RFC 7662）、ES256/EdDSA 签发与 24h 轮换重叠的 JWKS；同意页与 Enterprise Center 的 OAuth 控制台；MCP Gateway 除 PAT 外接受 AS 签发的访问令牌。
- 品牌门禁：`scripts/check-brand.mjs` 保证技术标识不混入显示名。

已执行的验收：

```bash
npx -y pnpm@12.16.1 install --frozen-lockfile
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
npx -y pnpm@12.16.1 --filter @hpa/web typecheck
npx -y pnpm@12.16.1 --filter @hpa/web test
npx -y pnpm@12.16.1 --filter @hpa/web build
cargo build -p hpa-local
npx -y pnpm@12.16.1 test:e2e
npx -y pnpm@12.16.1 test:approval-branches
npx -y pnpm@12.16.1 test:postgres
npx -y pnpm@12.16.1 test:postgres:backup
npx -y pnpm@12.16.1 --filter workbench test
npx -y pnpm@12.16.1 --dir apps/vscode-extension run test:host
npx -y pnpm@12.16.1 --dir apps/vscode-extension run package
npx -y pnpm@12.16.1 check
```

Phase 1 回归：10 个 IR fixture 的 JSON/YAML 语义往返 100%；分支/循环/并行/重试/超时/错误回退/审批/子流程/HTTPS 均通过运行时测试；并行分支执行中途快照恢复后不会重放已完成请求；100 节点基准（debug 构建、本机）编译 20.3ms、执行 44.4ms（101 步）。

Phase 2 验收：CLI/API/Web 完成导入→源码草稿编辑→新版本→测试→运行→签名→发布→导出闭环；Python、Node、Bash 脚本均通过确定性测试；seatbelt 实测拒绝网络连接、工作区外写入与主目录读取；超时、输出上限、命令白名单、输入 Schema、无许可/专有 Skill 与篡改签名均有负向测试；MCP stdio 端到端测试完成 `tools/list` 与 `tools/call`；导出目录写入临时 HOME 下的 `.claude/skills/` 与 `$CODEX_HOME/skills/` 结构，Codex 通过 `codex debug prompt-input` 实际发现并列出 `invoice-audit` Skill。<br>Claude Code 的实际会话加载未在本机自动执行（未调用模型/登录会话），导出目录按官方 `.claude/skills/<name>/` 结构验证。

Phase 3 验收：HTTP Gateway 集成测试覆盖 modern `server/discover`/`resultType`、legacy `initialize`/会话与鉴权 GET SSE、PAT 401、`Mcp-Method` 失配 400、Origin 403、Skill 工具调用、`subscriptions/listen` 目录/task 变更通知、`tasks/get/update`、OAuth 资源元数据，以及企业租户 MCP token 路由；命中限流返回 429 + `Retry-After`；MRTR 高风险审批在 API/Web 完成“input_required → 审批 → 重试执行”，审批绑定参数哈希且一次性消费；stdio 与 HTTP 外部 MCP Server 均通过 hpa MCP Client 探测并完成只读工具调用，SSRF/主机白名单负向测试通过。Phase 13 新增双进程 PostgreSQL 测试：task owner 节点强停期间由另一节点提交取消命令，owner 使用相同 node ID 重启后恢复并在另一节点的 SSE 上产生 task status 通知。Claude Code/Codex 的外部模型工具调用仍未自动执行。

Phase 4 验收：Agent Center 完成 AgentVersion 创建→发布→测试运行→审批→会话→回放闭环；确定性 planner 调用已发布 Skill 并生成输出，trace replay 与原始输出一致；同版本不同内容返回 `AGENT_VERSION_FROZEN`；高风险 `workflow.execute` 先触发 Agent 审批，批准后 Workflow 内审批仍独立等待，Agent 审批不能替代 Workflow 审批；父 Agent handoff 继承剩余预算，子 Agent 工具超限与自引用深度均按预算失败；OpenAI 兼容 mock 验证工具调用、token/成本记账、连续计划检测与网络允许列表拒绝；服务重启后 running 会话恢复为 interrupted 并可从检查点继续，等待中的审批记录不变。界面在 1440px 与 607px 均无横向溢出，运行、审批、设计器和会话事件已在浏览器中实际操作验证。

实现与边界详见 [`docs/04-agent/local-agent-platform.md`](docs/04-agent/local-agent-platform.md)。

Phase 5 验收：Plugin Center 完成签名包校验→安装→能力导入→停用/启用→升级→回滚→卸载闭环；Ed25519 信任、内容摘要、未签名/未知键拒绝、许可白名单、依赖检查、路径穿越与符号链接拒绝均有负向测试；本地 Marketplace 完成发布→待审→审核→评分→安装→撤回闭环，未审核安装返回 `PLUGIN_FORBIDDEN`；Connector Center 完成 HTTP/File 连接创建、测试、调用与审计，覆盖 loopback、allowlist、SSRF 私网拒绝、429 限流、500→200 重试与 File 根路径限制；插件贡献的 Connector/Agent/Skill/MCP 能力分别进入对应注册表，Agent 调用 Connector 仍走独立审批。11 个 Phase 5 集成测试与 88 个全量 Rust 测试通过；Web typecheck、9 个 Vitest 与 build 通过；Plugin Center/Connector Center 在 1440px 与 607px 均无横向溢出，并实际在浏览器中调用 File 连接器读取文件。

实现与边界详见 [`docs/07-plugin/local-implementation.md`](docs/07-plugin/local-implementation.md)。

Phase 6 验收：控制面完成租户创建→API 令牌→OIDC 成员目录→配额→Worker 注册/心跳→备份→恢复→审计导出闭环；9 个 HTTP 企业测试与 3 个服务层测试覆盖认证边界、跨租户隔离、RBAC、令牌一次性与吊销、成员 CRUD/租户过滤、配额与并发 429、审计/CSV、租户删除保护、备份恢复/清理、Worker 离线派生与本地 JWKS 的 OIDC 校验；全量 100 个 Rust 测试通过。Web typecheck、9 个 Vitest 与 build 通过；Enterprise Center 的 1440px 与 607px 均无横向溢出、无未翻译 key，并在浏览器中实际切换成员角色；成员页同时验证了服务端角色持久化。截图见 `docs/assets/phase6/`。

实现与边界详见 [`docs/08-platform/local-enterprise.md`](docs/08-platform/local-enterprise.md) 与 [`docs/08-platform/enterprise-operations.md`](docs/08-platform/enterprise-operations.md)。

Phase 7 验收：VS Code 扩展完成 Runtime 连接→Workflow 运行→执行事件/审批→`succeeded`，以及 Skill 安装到 `.github/skills`；`MCP: List Servers` 显示 `Auralis Local MCP`，启动后 `Discovered 4 tools`，slug 别名 `workflow_list` 调用成功；Copilot Chat 读取了安装后的 `invoice-audit` Skill。60 个 Vitest、7 个 Extension Host 检查与 VSIX 临时目录安装通过；全量 Rust 测试 101 passed / 1 ignored。扩展侧栏在 1440px 与 607px 无页面横向溢出，代码/CSP/路径负向检查见实现文档。截图见 `docs/assets/phase7/`。

实现与边界详见 [`docs/14-vscode-copilot/local-implementation.md`](docs/14-vscode-copilot/local-implementation.md)。

Phase 8 验收：真实 `hpa worker` daemon 完成远程 Workflow `succeeded`、审批等待→人工审批/自动 SLA→resumed→`succeeded`，并在 Worker Center 取消审批等待 Job 后 execution 进入 `cancelled`；错误 token 401、租户/worker 隔离、版本/能力/并发槽、租约过期重排与旧 lease 409 均有测试。start/events/complete 持久 outbox 的单测覆盖顺序、去重、重开恢复与失败保留。Worker Center 在 1440/607 无页面横向溢出、无未翻译 key，截图见 `docs/assets/phase8/`。

实现与边界详见 [`docs/08-platform/hybrid-workers.md`](docs/08-platform/hybrid-workers.md) 与 [`docs/08-platform/hybrid-worker-deployment.md`](docs/08-platform/hybrid-worker-deployment.md)。

Phase 9 验收：浏览器完成 `/login` → mock IdP 授权码 → `/auth/oidc/callback` → 平台管理员 session → `/enterprise` → silent refresh → RP-initiated logout → 登录页全流程，`EdDSA`、`RS256`、`ES256` 三种签名均通过。Rust HTTP 集成测试覆盖 Cookie 属性、`/api/auth/session`、Cookie 认证写操作缺 `X-HPA-CSRF` 403、state 重放、redirect 逃逸、disabled 成员、nonce 失配、refresh 轮换/拒绝、logout state 重放与登出后 401。登录页、已登录 Enterprise、回调错误与 OIDC 配置在 1440/607 无页面横向溢出。

实现与边界详见 [`docs/08-platform/browser-sso.md`](docs/08-platform/browser-sso.md)。

Phase 10 验收：`/oauth/register` 注册公共客户端 → `/oauth/authorize` 302 到 `/consent` → 同意 → 回调收码 → `/oauth/token` 换取访问/刷新令牌 → 用访问令牌调用 `/mcp` `server/discover` 成功 → 内省 active → 刷新令牌轮换 → 吊销后 `/mcp` 401。负向覆盖未注册 `redirect_uri`（400 且不重定向）、缺少 PKCE、`plain` 方法、未注册 scope（带 `state` 回到客户端）、错误 `code_verifier`、授权码重放、刷新令牌复用触发整链吊销、拒绝授权 `access_denied`、错误客户端密钥 401、停用客户端后密钥轮换 409。Playwright 真实浏览器完成同一流程并在 1440/607 截图，两页均无横向溢出；控制台列出客户端与已签发令牌。Rust 新增 4 个单元测试与 4 个 HTTP 集成测试，`cargo clippy --workspace --all-targets -- -D warnings`、`cargo fmt --all --check`、品牌门禁与 IR 门禁通过；Web typecheck、10 Vitest（新增 Consent / EnterpriseOAuth 文案门禁）与 build 通过。

实现与边界详见 [`docs/08-platform/oauth-authorization-server.md`](docs/08-platform/oauth-authorization-server.md)。

Phase 11 验收：发布带 `schedule` + `webhook` + `event` 三个触发器的工作流后，`GET /api/triggers` 返回 3 行且事件触发器包含 topic 与发布地址，`next_fire_at` 落在 `09:00 +08:00` 的整分边界；带 `sha256=` 签名的投递返回 202 并生成执行，重复 `X-HPA-Event-Id` 返回 409，错误签名/过期时间戳/缺签名返回 401，未知触发器 404，停用后 409；`POST /api/events/{topic}` 生成事件订阅执行，重复事件 ID 返回 409，停用后 404；手动触发与 `/api/trigger-fires` 正常。调度语义用注入时钟测试：到期触发一次并推进、同一时刻重复 tick 不重复、`skip` 丢弃迟到 10 分钟的 slot、`catch_up` 按上限补齐并跳过剩余。Playwright 完成「建工作流→发布→Webhook 投递→事件发布→手动触发→停用/启用→界面触发」并在 1440/607 截图，无横向溢出。

实现与边界详见 [`docs/03-workflow/triggers.md`](docs/03-workflow/triggers.md)。

Phase 12 验收：`/api/ready` 从 `starting` 转为 `ready`，四项检查（`controlDb`/`tenantDb`/`signingKey`/`scheduler`）均为 ok；跑完一个三段工作流后 `/metrics` 含 `hpa_workflow_executions_total{status="succeeded",tenant="local"}`、`hpa_node_seconds_bucket{type="transform",le="+Inf"}`、`hpa_workflows_total{tenant="local"}`、`hpa_scheduler_last_tick_timestamp_seconds` 与 `route="/api/executions/{id}"` 标签；`content-type` 为 `text/plain; version=0.0.4`。企业模式下匿名抓取返回 401、带管理员令牌返回 200，`/api/ready` 仍对探针开放。浏览器验证执行详情时间线（3 个节点、最慢标记、607px 单列）与设置页运行时状态（4 项检查、运行时长、最近 tick、`/metrics` 链接），1440/607 均无横向溢出。新增 2 个指标渲染单元测试与 2 个 HTTP 集成测试；Web 11 Vitest 与 build 通过。

实现与边界详见 [`docs/08-platform/observability.md`](docs/08-platform/observability.md)。

## 运行

要求 Rust 1.99.1（MSRV 1.97）与 Node.js 26。

中国大陆网络可先启用国内下载源。锁文件和所有校验保持启用，详见 [国内依赖下载源](docs/13-tech-stack/domestic-download-mirrors.md)：

```bash
. scripts/use-domestic-mirrors.sh
```

```bash
npx -y pnpm@12.16.1 install --frozen-lockfile
npx -y pnpm@12.16.1 --filter @hpa/web build

cargo run -p hpa-local -- serve
```

打开终端输出的地址，默认是：

```
http://127.0.0.1:8787
```

数据默认写入 `~/.hpa/hpa.db`；也可使用 `--data-dir` 或 `HPA_DATA_DIR` 指定。首次打开可在工作台点击「サンプルを追加 / Add samples」加载 Workflow 示例；Agent Center 也提供示例 Agent 与发票审计 Skill 的导入入口。企业控制面与租户数据分别写入 `<data_dir>/control.db` 与各租户目录；默认本地模式可直接打开 `/enterprise`，设置 `HPA_ENTERPRISE_MODE=1|true|required` 后除健康检查外都需要 Bearer 令牌。

浏览器 SSO（本地验收可用 mock IdP）：

```bash
node scripts/oidc-mock.mjs
# 可选：OIDC_PORT=18970 OIDC_ALG=EdDSA|RS256|ES256
```

在 Enterprise Center → OIDC 填写 `issuer`、`clientId`、`authorizationEndpoint`、`tokenEndpoint`、`redirectUri`（`http://127.0.0.1:8787/auth/oidc/callback`）、`adminSubjects` 等，然后打开 `http://127.0.0.1:8787/login` 点击 SSO。mock 默认登录 `admin@example.test`（`local` 租户 owner）；它只用于本地验收，不要用于生产。完整契约与测试命令见 [`docs/08-platform/browser-sso.md`](docs/08-platform/browser-sso.md)。

SCIM 2.0 使用租户 `hpa_ent_` owner/admin token，端点前缀 `/scim/v2`。可重复 E2E：

```bash
HPA_BASE_URL=http://127.0.0.1:8787 \
HPA_TOKEN_FILE=/secure/scim-owner.token \
pnpm test:scim-e2e
```

完整 Users/Groups、PATCH、ETag、角色冲突和企业 IdP 验收边界见 [`docs/08-platform/scim.md`](docs/08-platform/scim.md)。

VS Code 扩展：

```bash
npx -y pnpm@12.16.1 --filter workbench build
npx -y pnpm@12.16.1 --dir apps/vscode-extension run package
code --install-extension hpa-workbench.vsix
```

开发模式也可使用 `code --extensionDevelopmentPath=apps/vscode-extension .`。扩展默认连接 `http://127.0.0.1:8787`；打开 Auralis 活动栏即可查看视图。

Hybrid Worker：

```bash
# 先在 Enterprise Center → ワーカー 注册 Worker，复制一次性 workerToken
HPA_WORKER_TOKEN="<worker-token>" \
  hpa --data-dir ~/.hpa-worker worker --control-url http://127.0.0.1:8787 \
  --max-concurrency 2 --capabilities workflow
```

Worker 主动出站轮询，不开放入站端口；token 只放环境变量，不要写命令行/日志。远程控制面必须 HTTPS。

命令行：

```bash
hpa serve --port 8787 --data-dir ~/.hpa
hpa doctor
hpa validate fixtures/01_sequence.yaml
hpa run fixtures/01_sequence.json --input '{"value": 41}'

hpa --data-dir ~/.hpa skill validate fixtures/skills/invoice-audit
hpa --data-dir ~/.hpa skill import fixtures/skills/invoice-audit
hpa --data-dir ~/.hpa skill publish skill_invoice_audit
hpa --data-dir ~/.hpa skill test skill_invoice_audit
hpa --data-dir ~/.hpa skill run skill_invoice_audit --input '{"invoiceId":"inv_1","items":[{"amount":12.5}]}'
hpa --data-dir ~/.hpa skill export skill_invoice_audit --target claude
hpa --data-dir ~/.hpa mcp token create external
hpa --data-dir ~/.hpa mcp probe -- hpa --data-dir ~/.hpa mcp serve
hpa --data-dir ~/.hpa mcp call --tool skill.invoice-audit.audit --args '{"invoiceId":"inv_1","items":[]}' -- hpa --data-dir ~/.hpa mcp serve

hpa --data-dir ~/.hpa agent import fixtures/agents/invoice-auditor.agent.json
hpa --data-dir ~/.hpa agent publish agent_invoice_auditor
hpa --data-dir ~/.hpa agent run agent_invoice_auditor --input '{"invoiceId":"inv_1","items":[{"amount":12.5}]}'
hpa --data-dir ~/.hpa agent sessions agent_invoice_auditor
hpa --data-dir ~/.hpa agent replay <sessionId>
hpa --data-dir ~/.hpa agent approvals --status pending
hpa --data-dir ~/.hpa agent decide <approvalId> --decision approved

hpa --data-dir ~/.hpa plugin sample
hpa --data-dir ~/.hpa plugin sign ./my-plugin
hpa --data-dir ~/.hpa plugin pack ./my-plugin ./my-plugin-1.0.0.hplugin
hpa --data-dir ~/.hpa plugin install ./my-plugin-1.0.0.hplugin
hpa --data-dir ~/.hpa plugin list
hpa --data-dir ~/.hpa plugin publish ./my-plugin-1.0.0.hplugin
hpa --data-dir ~/.hpa plugin review plugin_example 1.0.0 --status approved
hpa --data-dir ~/.hpa plugin install-marketplace plugin_example 1.0.0
hpa --data-dir ~/.hpa connector sample
hpa --data-dir ~/.hpa connector list
hpa --data-dir ~/.hpa connector connections
hpa --data-dir ~/.hpa connector test <connectionId>
hpa --data-dir ~/.hpa connector invoke <connectionId> read --args '{"path":"welcome.txt"}'
hpa --data-dir ~/.hpa connector audit --limit 50

hpa --data-dir ~/.hpa enterprise init --name bootstrap
hpa --data-dir ~/.hpa enterprise token --tenant local --name ci --role viewer
HPA_ENTERPRISE_MODE=required hpa --data-dir ~/.hpa serve --port 8787
```

MCP HTTP 客户端配置示例（使用 `hpa mcp token create` 输出的一次性令牌）：

```bash
export HPA_MCP_TOKEN="<token>"
claude mcp add hpa http://127.0.0.1:8787/mcp --transport http --scope user --header "Authorization: Bearer $HPA_MCP_TOKEN"
codex mcp add hpa --url http://127.0.0.1:8787/mcp --bearer-token-env-var HPA_MCP_TOKEN
```

OAuth 2.1 授权（不需要预先创建 PAT）：客户端先用 `GET /.well-known/oauth-authorization-server` 发现端点，再用 `POST /oauth/register` 注册，浏览器打开 `/oauth/authorize` 完成同意后即可调用 `/mcp`。支持 OAuth 的 MCP 客户端直接指向 `http://127.0.0.1:8787/mcp` 即可自动走这条流程：

```bash
# 查看 AS 元数据与签名公钥
curl -s http://127.0.0.1:8787/.well-known/oauth-authorization-server
curl -s http://127.0.0.1:8787/oauth/jwks
```

浏览器流程的自动化验收（Playwright，真实点击同意页）：

```bash
HPA_BASE_URL=http://127.0.0.1:8787 node scripts/phase10-oauth-e2e.mjs
```

触发器：在 Workflow IR 里加 `triggers`，发布后到 `/triggers` 查看下次触发时间；Webhook 用 HMAC 签名投递：

```bash
TS=$(date +%s)
BODY='{"action":"published"}'
SIG=$(printf '%s.%s' "$TS" "$BODY" | openssl dgst -sha256 -hmac "$WEBHOOK_SECRET" -r | cut -d' ' -f1)
curl -sS -X POST http://127.0.0.1:8787/hooks/<workflowId>/<triggerId> \
  -H "content-type: application/json" -H "x-hpa-timestamp: $TS" \
  -H "x-hpa-signature: sha256=$SIG" -d "$BODY"

HPA_BASE_URL=http://127.0.0.1:8787 node scripts/phase11-triggers-e2e.mjs
```

可观测性：抓取指标、查就绪、跑浏览器验收：

```bash
curl -s http://127.0.0.1:8787/metrics | head -40
curl -s http://127.0.0.1:8787/api/ready
HPA_BASE_URL=http://127.0.0.1:8787 node scripts/phase12-observability-e2e.mjs
```

开发模式：

```bash
cargo run -p hpa-local -- serve
cd apps/web && HPA_API_URL=http://127.0.0.1:8787 npx -y pnpm@12.16.1 dev
```

## Phase 1 行为边界

以下为当前本地 MVP 的明确边界：

- `http.request` 支持 `http://` 与 `https://`（rustls）；外部主机的访问必须显式声明 `permissions.network`。隐式回环不影响显式规则；私网地址在 DNS 解析后被拦截，除非规则本身允许。
- HTTP 客户端不跟随重定向、不使用系统代理、不接受 URL 中的 userinfo；响应体上限 8 MiB。
- 并行节点在分支内不含人工审批时真并发执行，`maxConcurrency` 限制同轮并发；含审批的分支回退为顺序执行以保证可恢复性。分支子状态随检查点持久化，恢复时已完成的分支不会重跑；同一节点在崩溃瞬间的副作用仍按“至少一次”语义处理。
- 审批 SLA 由本地驱动每秒扫描并自动按 `onTimeout`（`approve` / `reject`）恢复；`pending` 配置会被拒绝。
- 工作流级 `errorHandling.graph` 在根图未处理失败后运行一次，随后仍以原始错误结束执行。
- 发布版本冻结 IR 与 Node Registry 版本；同一 `workflowVersion` 不能改写已发布内容（API 返回 `VERSION_FROZEN`）。执行默认运行最新草稿，也可在请求中带 `version` 运行指定发布版本。
- Node Registry 当前版本 `2026.11.1`；手写配置必须先通过五类校验。`compensate` 错误模式、非 jq 脚本、未知权限与未注册版式会被拒绝。
- 本地运行时限制：100,000 步、每节点默认 30s 超时、工作流可选总超时、HTTP 响应 8 MiB、500 节点/2,000 边、嵌套深度受限。
- Phase 1/2 不包含 Agent；Phase 4 已加入本地 Agent Platform，Phase 5 已加入本地 Plugin/Connector/Marketplace，Phase 6 已加入本地 Enterprise 控制面，Phase 7 已加入本地 VS Code & Copilot 集成，Phase 8 已加入本地/私有 Hybrid Worker 执行，Phase 9 已加入本地 Enterprise Browser SSO。云端多节点 Control Plane、跨租户市场和计费仍属于后续阶段；浏览器 SSO refresh/logout 与 SCIM Users/Groups 已实现。

## Phase 2 行为边界

- Skill 脚本只支持本地确定性执行；`network` 权限非空、依赖在线安装、模型/Agent 推理均不在本阶段执行范围内。
- macOS 沙箱由 `sandbox-exec` 提供：拒绝网络、工作区外写入与 `/Users` 读取；Linux 仅在安装 `bwrap` 时提供网络/文件系统隔离，否则退化为进程级隔离并在执行结果中标记 `process`，不应视为强隔离。
- 工作区、输入与快照文件对脚本只读；`artifacts/` 可写。脚本参数使用 argv 数组与占位符替换，不经过 shell 拼接；stdout 默认上限 1 MiB，stderr 256 KiB，产物默认上限 50 MiB/100 个文件。
- Skill 导入只做本地目录读取；不自动安装 Python/Node 依赖，也不下载模型。依赖声明会保留并校验形状，Skill 类型依赖会检查本地 Registry；包管理器依赖需由用户在宿主环境自行准备。
- 发布版本冻结内容哈希；同一 `skillId@version` 不同内容会被拒绝。发布需要许可白名单内的 SPDX 许可；无许可/专有 Skill 可导入查看，但拒绝发布与导出。
- 导出可写标准目录、Claude Code `.claude/skills/<name>/` 或 Codex `$CODEX_HOME/skills/<name>/`。Codex 的 Skill 发现已通过 `codex debug prompt-input` 验证；Claude Code 未执行真实会话加载。
- MCP Gateway 提供 HTTP 与 stdio；legacy GET 提供带会话和鉴权的 SSE，modern `subscriptions/listen` 返回请求作用域 SSE 并推送目录/task 变更；tasks 扩展提供 `tasks/get` 与审批等待任务取消。PostgreSQL 模式中，其他节点提交的取消命令持久化并路由到 task owner，owner 重启后继续消费。现代请求必须携带 `MCP-Protocol-Version`/`Mcp-Method`（`tools/call`、`resources/read`、`prompts/get` 还需 `Mcp-Name`）。
- Skill Center 可编辑导入快照的文本文件并保存为新版本；已有版本目录与内容哈希不可变。
- Skill 可使用本地 Ed25519 密钥签名，API、CLI 与 Skill Center 会校验签名和内容哈希；Python/Node 离线依赖安装、容器沙箱与远程 Registry 同步尚未实现。

## Phase 3 行为边界

- 认证支持 tenant-scoped PAT/stdio 能力令牌、OAuth 2.1 资源元数据、401 challenge，以及 Phase 10 的 Authorization Code + PKCE、DCR 和刷新令牌。
- `/api/mcp/*` 管理接口与 Web UI 仅绑定 loopback，未额外加登录；HTTP MCP endpoint 必须携带 Bearer token。stdio 首次运行会生成权限 `0600` 的能力令牌文件（可用 `HPA_MCP_TOKEN` 覆盖）。
- 限流为进程内 per-token 滑动窗口，重启后清零；默认 120 请求/分钟，`HPA_MCP_RATE_LIMIT` 可覆盖。
- legacy 会话仅保存在进程内（1 小时 TTL）；modern 目录通知与 tasks 同属单进程实现，没有跨节点共享或断线重放。
- 高风险审批目前覆盖 `workflow.execute`；approval 与工具参数哈希绑定、10 分钟过期并一次性消费。MRTR 的 `inputResponses` 不作为授权依据，必须由本地审批中心批准。
- MCP Client 支持 stdio/HTTP 与 modern→legacy 回退；HTTP 默认只允许回环，远端主机必须显式加入网络白名单，并拒绝元数据地址；未实现上游 OAuth 登录或凭证透传。
- 本地工具只暴露“最新且已发布”的 Skill 版本与已发布 Workflow；企业模式按 Bearer/OAuth token 选择租户，MCP token 与资源 URI 均绑定租户。
- Claude Code 与 Codex 已通过官方配置连接并完成 initialize/tools/list 发现；外部模型会话中的工具调用未自动执行，只读调用由 hpa MCP Client 在同一 Gateway 上完成。

## Phase 4 行为边界

- 仅本地执行；不自动安装模型、Python/Node 依赖或外部 MCP Server。OpenAI 兼容 planner 只支持单个工具调用/回合。
- 非回环模型 endpoint 必须显式列入 `network`、使用 HTTPS，且解析到私网/链路本地/元数据地址会被拒绝；本机回环无需额外规则。HTTP 不跟随重定向、不使用系统代理、不接受 userinfo。
- 确定性回放仅覆盖 deterministic planner 的 trace replay；模型 planner 的响应与 token/成本仅审计，不承诺跨会话重新生成相同输出。
- 高风险 `workflow.execute`、`mcp.*` 固定需要 Agent 审批；Agent 审批不代替 Workflow 审批。子 Agent 需要审批时父 handoff 失败关闭，必须显式处理子会话或调整审批配置。
- 本地进程重启将 running 会话标记为 interrupted 并可从检查点继续；崩溃瞬间的外部副作用按至少一次语义处理。等待审批的会话与审批记录跨重启保留。
- Agent 版本/会话/事件/审批/工具调用在本地模式保存在 SQLite；`HPA_POSTGRES_RUNTIME=1` 时切到 PostgreSQL，可跨节点读取、恢复与取消。没有云端 Control Plane、Marketplace、多租户隔离或远程 Agent 调度。

## Phase 5 行为边界

- 插件是声明的能力包，不执行插件内 Node/TS/JS handler；贡献的 Skill 脚本仍须发布后由既有 Skill Runtime 沙箱执行。SDK 是 `hpa plugin sign/pack/validate/install` CLI 与 JSON manifest，未提供 `@hpa/plugin-sdk` 或 TS handler 运行时。Remote Marketplace 已支持签名索引、发布者 key、HTTP/离线镜像、撤回、OSV 门禁和 key-based cosign/OCI digest provenance；Fulcio/Rekor keyless 与 Registry attestation 拉取仍未实现。
- 签名信任只有 `verified`/`community`/`untrusted`；本地 Ed25519 私钥保存在 `data_dir/keys/`（Unix `0600`），没有密钥轮换/吊销或官方密钥层级。市场审核与签名信任是两套状态。
- 本地 Marketplace 的审核/评分是单用户本地操作，不具备反刷、SLA、付费或跨租户隔离；撤回不自动卸载已安装版本。
- Connector 提供 HTTP、File、PostgreSQL、只读 Git、Redis MQ 与 S3-compatible；HTTP/S3 关闭重定向/代理/userinfo，外部主机必须匹配 Connection `allowedHosts` 且 metadata/link-local 地址被拒绝。数据库只支持 PostgreSQL，Git 不执行 fetch/push，Redis 是 pub/sub 而非 durable stream，S3 仅 path-style 和单请求对象。完整边界见 [Connector 生产扩展](docs/08-platform/connector-implementation.md)。
- 连接凭证只保存环境变量名；限流为进程内 per-connection/operation 滑动窗口，重启清零；审计不保存响应正文或凭证。
- 插件 `permissions` 只做能力形状与一致性门禁，不是通用运行时权限系统；Connector 的主机白名单来自 Connection 配置。Skill/Agent 贡献仅导入草稿，卸载后仍作为用户资产保留。
- SBOM 是本地文件哈希与声明清单；静态扫描只做固定字符串警告。漏洞门禁需配置 OSV 或离线快照。Key-based Sigstore provenance 和同控制面 SSE 撤回推送已实现；恶意软件情报、Fulcio/Rekor keyless、Registry attestation 拉取与跨控制面 federation 仍未实现。

## Phase 6 行为边界

- 默认本地模式无登录：匿名请求映射为 `local` 租户的 `owner` 且视为平台管理员；只有设置 `HPA_ENTERPRISE_MODE` 后才强制 Bearer。把本地端口暴露给他人等同于交出控制面。
- 默认本地运行路径仍是“每租户独立 SQLite 文件 + TenantGate 路由”；`HPA_POSTGRES_RUNTIME=1` 时 Workflow/Execution/Trigger、execution lease、协作取消请求、执行幂等键、Enterprise/Worker Job、令牌/审计、浏览器会话、OAuth AS、成员/SCIM、Agent、Skill、MCP、Connector 与 Plugin 元数据均以 PostgreSQL 为权威，SQLite 只作单节点模式或可重建缓存。TenantGate 缓存无 TTL，长期运行大量租户会增加内存与文件句柄。
- RBAC 是固定 8 角色 + 4 个路径近似动作（read/write/execute/approve），没有资源级 ACL、自定义角色或 ABAC；`owner/admin/developer/publisher` 对租户 API 实际等价。
- Phase 6 时 OIDC 只接受 EdDSA/Ed25519 Bearer JWT 与 JWKS，没有浏览器 SSO、授权码回调、登录会话、刷新令牌或登出；Phase 9 已加入浏览器 SSO 与 EdDSA/RS256/ES256。成员目录只覆盖 OIDC 主体的角色/禁用状态，没有密码、组、邀请或成员自服务；未登记 OIDC 主体按 claims 认证。
- 配额检查针对特定创建路径与当日计数；日计数按 UTC；并发数统计 `running` + `waiting_approval`。配额不是权限，也不含计量/计费或付费计划。
- Phase 6 的 Worker 注册/心跳在 Phase 8 扩展为远程执行；仍不下发凭证、不建立反向隧道。超过 30 秒无心跳显示 `offline`，数据库状态仍为 `active`。
- 备份使用 `VACUUM INTO` 写本地文件；恢复需要重启进程，且只恢复租户业务库，不恢复控制库中的租户/令牌/配额/审计。控制库本身不在备份范围。
- 审计写入是尽力而为，与业务操作不同事务；没有 WORM、哈希链、自动保留或 SIEM 投递。CSV 对公式前缀做转义，但仍应视为敏感数据。
- `/mcp` 从 token 解析租户并选择对应租户 Router，不依赖 `TenantGate` 的 `/api` 路径；`agent/skill/mcp/plugin/connector/run/doctor` CLI 仍直接操作本地租户库。
- 服务只监听 `127.0.0.1` 明文 HTTP；远程访问需要 TLS 反向代理。Phase 8 提供出站 Worker 执行，但仍没有 HA、在线迁移、租户暂停 API 或控制库在线备份。

## Phase 9 行为边界

- 浏览器 session 是额外认证路径；本地匿名模式、`hpa_ent_`/OIDC Bearer、Worker token 与 VS Code 集成保持不变。
- public client：没有 client secret、refresh token、silent renew、RP-initiated logout、SCIM 或组同步；session 过期即重新登录。
- 服务端 session 只存 token 哈希；Cookie 为 HttpOnly、SameSite=Lax，HTTPS 部署必须设置 `cookieSecure`。CSRF 只对 Cookie 认证的写请求强制，Bearer 不受影响。
- 平台管理员只来自 `adminSubjects`（sub 精确匹配、email 大小写不敏感），ID Token claims 不授予平台管理员；disabled 成员登录被拒绝，既有 session 在后续检查中被拒绝，`/auth/session` 会清除浏览器 Cookie。
- 授权/token/JWKS 端点沿用 HTTPS、network allowlist 与私网拦截；token 请求不跟随重定向、不使用系统代理。PKCE verifier 以明文存 `control.db`，依赖磁盘权限与加密。
- 不调用 UserInfo，只验证 ID Token；不支持第三方 Cookie、SameSite=None、IdP 侧登出或 IdP 会话管理。mock IdP 只用于本地验收。
- 单节点 SQLite：flow/session 表不跨节点共享；登录失败只返回稳定错误码，不向浏览器区分用户不存在与角色无效。

## Phase 12 行为边界

- 指标是**进程内**的：重启归零，多节点各自暴露，由抓取端相加；没有外部存储或聚合。
- 执行/节点指标只统计驱动器跑完的执行；API 手工取消、租户删除等路径不计入，避免重复计数。
- 并行分支里一个 `step()` 可能同时完成多个节点，节点耗时按完成事件各记一次，因此节点耗时之和可能大于执行总时长。
- OTLP traces/metrics/logs 为可选，需 `HPA_OTEL_ENABLED=1` 与 Collector；Rust Prometheus metrics 和 tracing events、Python session/token/tool metrics/logs 同步导出，Prometheus/JSON 本地路径仍保留。Collector 停机时按有界队列丢弃，业务继续运行。
- 直方图桶固定 13 档（5ms–60s）不可配置，不提供 exemplars；不采集 workflow/execution id 等高基数标签。
- `/metrics` 不做限流，也不提供租户视图：企业模式下要么全有（管理员令牌）要么 401。
- `/api/ready` 只探测 `local` 租户库，新增租户不会自动纳入探针；调度器心跳 15 秒过期即判定未就绪。
- 执行时间线由前端从事件推导，服务端不保存聚合耗时；事件被裁剪或延迟时时间线随之变化。

## Phase 11 行为边界

- `schedule`、`webhook` 与 `event` 都会真正执行；`event` 触发器按 `config.topic` 订阅，通过 `POST /api/events/{topic}` 发布并支持事件 ID 幂等，`mcp` 走 `workflow.execute` 工具。
- 触发器只运行**已发布版本**；改草稿不影响正在跑的调度，重新发布会重算 `next_fire_at` 并保留历史。
- cron 是自研 5 字段实现，不支持秒级、`@daily` 别名、`L`/`W`/`#` 扩展；日与周同时受限按并集处理。
- 时区只接受 `UTC` 与固定偏移（`+08:00`），没有 IANA 时区库，因此不处理夏令时。
- `skip` 的迟到阈值固定 60 秒；没有抖动、退避、并发上限或每触发器配额。
- Webhook 只支持 HMAC-SHA256 + 时间戳窗口 + 可选事件 ID 去重；不支持 mTLS、IP 白名单、请求体 schema 校验或同步返回执行结果。
- 签名密钥以明文存在租户库的 `config_json` 里（与 Workflow 快照同库），轮换需要改 IR 并重新发布。
- 调度器每节点各自 tick，靠 `next_fire_at` 乐观锁避免重复触发；没有 leader 选举、分布式队列或跨节点时钟对齐。
- 调度只在本机启动执行，不派发到 Hybrid Worker；远程派发仍走显式 Worker Job 流程。

## Phase 10 行为边界

- 只做授权服务器（AS），不做身份提供方：不签发 ID Token、没有 `/userinfo`、不支持 `openid` scope 或用户注册；登录仍由 Phase 9 的 OIDC SSO 或本地匿名模式提供。
- 签名默认 ES256；`HPA_OAUTH_SIGNING_ALG=EdDSA` 可切换为 Ed25519。不提供 RS256（ring 不能生成 RSA 私钥），需要 RS256 的客户端只能使用 PAT。
- 不支持 CIMD、PAR、JARM、DPoP、mTLS 令牌绑定、token exchange；`response_type` 仅 `code`，`code_challenge_method` 仅 `S256`。
- 访问令牌 1 小时、刷新令牌 30 天且每次使用轮换；刷新令牌复用会吊销整条 family，但没有 refresh token family 的跨节点同步。
- 校验同时验签与查库，吊销立即生效；代价是每个 MCP 请求多一次 SQLite 查询，且多实例部署时吊销状态不共享。
- 授权端点要求浏览器身份：有会话用会话主体，企业模式无会话跳 `/login`，本地模式使用匿名 `local-owner`。本地端口暴露给他人等同于交出授权能力。
- MCP Gateway 按 token 选择租户；资源 URI、工具目录与审计均留在该租户数据库，不能通过请求参数切换租户。
- 同意记录按 client/tenant/subject 覆盖 scope；没有按资源粒度、时间窗或单次授权的细粒度控制，`prompt=none` 不支持。
- 客户端与令牌管理接口属平台管理员操作；本地模式等价 owner，无额外登录。

## Phase 8 行为边界

- Worker 可由共享 PostgreSQL 的控制面派发；没有 Redis Stream、自动扩缩容、mTLS 或跨主机自动故障切换。Worker 必须能通过 loopback HTTP 或 HTTPS 访问控制面。
- `hpa_wrk_` token 绑定 worker/tenant，明文只返回一次；轮换/禁用/租户删除会吊销。Worker 不得跨租户 lease；任务 snapshot 含 Workflow 定义，禁止把 secret 写进 Workflow。
- 同一 Rust Runtime 执行，不引入第二执行引擎；副作用按至少一次语义。daemon 使用 `<data_dir>/worker-outbox.db` 持久化 start/events/complete，进程重启后先排空 outbox；HTTP 成功但删除确认前崩溃会安全重放，旧 lease 的迟到上报返回 409 并清理。
- 审批等待释放并发槽；审批后 Job 重新入队并从保存 state 继续。远程审批 SLA 由控制面后台扫描后自动按 `onTimeout` 恢复并重新入队。
- 取消对 queued/leased/waiting 立即生效；running 为协作取消（step 之间检查，长 HTTP/脚本不可中断）。本地 Runtime 不接管 `remote_executions`。
- 当前能力类型为 `workflow`，未派发 Agent/Plugin/Connector 等非 Workflow 任务；没有远程凭证注入、对象存储或 mTLS。
- Worker daemon 的本地状态只包含 outbox SQLite；远程/私有网需 TLS 反向代理；Worker 与控制面版本必须匹配。系统服务示例见 `deploy/`。

## Phase 7 行为边界

- 扩展是薄客户端，不在 VS Code 内执行 Workflow、Skill 脚本或插件代码；所有执行仍走 Rust Local Runtime/MCP Gateway。
- 扩展固定连接现有 `/api/*` 与 `/mcp`；没有远程 Worker 或 Marketplace 发布。`/mcp` 的租户由服务端 token 决定，扩展不提供租户切换。
- VS Code 工具名限制通过 `X-HPA-Tool-Name-Style: slug` 的请求级别名解决；stdio 与其他 HTTP 客户端保持规范点号名。没有为所有客户端强制改名。
- Skill 安装调用 Runtime 的 export API 写快照，扩展与 Runtime 必须共享文件系统；没有在线 Marketplace 下载、自动更新或跨设备签名信任同步。替换前备份、失败回滚，安装后本地修改需显式确认。
- MCP PAT 与 API token 只在 SecretStorage；不写 settings/workspace/日志/Webview。未信任工作区只读，运行/审批/取消/Skill 写入均被拒绝。
- 实测 VS Code 1.143.1；未建立 1.140–1.143 的运行时/高对比/Remote 兼容矩阵。Copilot Chat 可发现 `.github/skills` 中的 Skill，但模型驱动的 MCP `tools/call` 未验证，也未准备登录会话。
- 未实现 Custom Instructions/Custom Agents 生成、完整 Designer、远程工作区高级支持、Marketplace 发布或自动升级；执行重试已覆盖命令、Tree View 和执行 Webview。

## 技术栈

- **Rust**：Core Runtime、IR/编译器/校验器、Skill Loader/Registry/Sandbox Runner、Local API、MCP stdio 服务、Hybrid Worker daemon、SQLite 持久层。
- **TypeScript**：React + Vite Web、Canvas、Monaco、Skill Center、i18n 与调试界面；VS Code 扩展（Tree View/Webview/MCP provider/VSIX）。
- **Python/Node/Bash**：Skill 确定性脚本运行时（由 Rust 启动并沙箱化）；显式 `planner.kind = "python"` 可使用 Python 3.13 + LangGraph Worker，工具调用回到 Rust ToolService 执行；既有 deterministic/OpenAI-compatible planner 与确定性 Workflow 不依赖 Python。见 [Python Worker](docs/04-agent/python-worker.md)。

完整选型、版本、许可证与证据见 [`docs/13-tech-stack/`](docs/13-tech-stack/README.md)。

## 文档入口

| 读者 | 从这里开始 |
| --- | --- |
| 产品与决策 | [`docs/00-product/product-vision.md`](docs/00-product/product-vision.md) |
| 架构师 | [`docs/02-architecture/system-architecture.md`](docs/02-architecture/system-architecture.md) |
| 工作流开发 | [`docs/03-workflow/workflow-ir.md`](docs/03-workflow/workflow-ir.md) |
| 技术栈与 M0 | [`docs/13-tech-stack/mvp-stack.md`](docs/13-tech-stack/mvp-stack.md) |
| 本地 MCP Gateway | [`docs/06-mcp/local-gateway.md`](docs/06-mcp/local-gateway.md) |
| 本地 Plugin & Connector | [`docs/07-plugin/local-implementation.md`](docs/07-plugin/local-implementation.md) |
| 本地 Enterprise 控制面 | [`docs/08-platform/local-enterprise.md`](docs/08-platform/local-enterprise.md) |
| PostgreSQL schema/RLS 基线 | [`docs/08-platform/postgres-control-plane.md`](docs/08-platform/postgres-control-plane.md) |
| 分布式 outbox/lease 原语 | [`docs/08-platform/postgres-distributed-runtime.md`](docs/08-platform/postgres-distributed-runtime.md) |
| Connector 生产扩展 | [`docs/08-platform/connector-implementation.md`](docs/08-platform/connector-implementation.md) |
| Remote Marketplace | [`docs/07-plugin/remote-marketplace.md`](docs/07-plugin/remote-marketplace.md) |
| OTLP 与跨进程追踪 | [`docs/08-platform/otlp.md`](docs/08-platform/otlp.md) |
| 企业部署与运维 | [`docs/08-platform/enterprise-operations.md`](docs/08-platform/enterprise-operations.md) |
| Hybrid Worker 协议 | [`docs/08-platform/hybrid-workers.md`](docs/08-platform/hybrid-workers.md) |
| Hybrid Worker 私有部署 | [`docs/08-platform/hybrid-worker-deployment.md`](docs/08-platform/hybrid-worker-deployment.md) |
| OAuth 2.1 授权服务器 | [`docs/08-platform/oauth-authorization-server.md`](docs/08-platform/oauth-authorization-server.md) |
| 触发器与调度 | [`docs/03-workflow/triggers.md`](docs/03-workflow/triggers.md) |
| 可观测性（指标/就绪/时间线） | [`docs/08-platform/observability.md`](docs/08-platform/observability.md) |
| VS Code / Copilot 集成 | [`docs/14-vscode-copilot/README.md`](docs/14-vscode-copilot/README.md) |
| VS Code Phase 7 本地实现 | [`docs/14-vscode-copilot/local-implementation.md`](docs/14-vscode-copilot/local-implementation.md) |
| 研究证据与许可证 | [`docs/01-research/README.md`](docs/01-research/README.md) |
| 关键决策与 15 个结论题 | [`docs/12-decisions/final-questions.md`](docs/12-decisions/final-questions.md) |

## 强制约束

1. Canvas、YAML、JSON 共享同一 Canonical Workflow IR，不维护三套流程模型。
2. 确定性 Workflow 在无 AI、无云端 Control Plane 时也能完整执行。
3. Agent 与 Workflow 是两种执行模型，可互相调用但不可混同。
4. Skill 遵循 Agent Skills 开放标准，不使用平台私有 Skill 格式。
5. MCP 以当前规范及真实客户端兼容性为准，不假设一个配置适配所有客户端。
6. 未通过 Schema、静态、权限、依赖、执行策略校验的 Workflow 不得发布或执行。
7. 本地运行不强制依赖云端；云端 Control Plane 不直接执行不可信脚本。

## 许可证状态

Auralis/hpa 自身许可证尚未确定。开源复用边界、义务与禁用清单见 [`docs/01-research/license-analysis.md`](docs/01-research/license-analysis.md) 和 [`docs/adr/0012-open-source-reuse-and-licensing.md`](docs/adr/0012-open-source-reuse-and-licensing.md)。在许可证决策完成前，不得把标记为 Design Reference 或 Do Not Use 的代码并入产品。
