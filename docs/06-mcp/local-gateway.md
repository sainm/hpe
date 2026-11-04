# 本地 MCP Gateway 实现

> Phase 3 本地切片。实现位于 `crates/hpa-local/src/mcp.rs`、`mcp_http.rs`、`mcp_client.rs`，UI 位于 `apps/web/src/pages/McpCenter.tsx`。

## 1. 端点

- HTTP：`POST http://127.0.0.1:8787/mcp`；legacy 会话可建立鉴权 GET SSE，modern 使用请求作用域 `subscriptions/listen` SSE。`DELETE /mcp` 关闭 legacy 会话。
- stdio：`hpa mcp serve`。首次运行生成 `~/.hpa/mcp/stdio.capability`（权限 0600）；可用 `HPA_MCP_TOKEN` 替代。
- 资源元数据：`GET /.well-known/oauth-protected-resource`；401 响应带 `WWW-Authenticate: Bearer resource_metadata=...`。
- 管理 API/UI：`/api/mcp/*` 与 `/mcp-center` 仅绑定 loopback，不加登录；MCP endpoint 本身必须携带 PAT/能力令牌。

## 2. 协议

| 能力 | 2026-07-28 | 2025-11-25 兼容 |
| --- | --- | --- |
| 版本发现 | `server/discover`，`supportedVersions`/`resultType`/cache | `initialize` 选择 legacy 版本 |
| 会话 | 无状态，每个请求独立 `_meta` | `MCP-Session-Id`，进程内 1 小时 TTL |
| Header | `MCP-Protocol-Version`、`Mcp-Method`、`Mcp-Name` | 仅 session |
| 结果 | `resultType: complete`；MRTR 返回 `input_required` + `inputRequests` + `requestState` | 无 `resultType` |
| 取消/订阅/tasks | `subscriptions/listen` 目录通知；`tasks/get`；等待审批任务可取消 | JSON-RPC 取消通知可用；GET SSE 为进程内长连接 |

`tools/list`、`resources/list`、`prompts/list` 返回 `cacheScope: private` 与 `ttlMs`。`tools/call`、`resources/read`、`prompts/get` 校验 `Mcp-Name`。

### 2.1 VS Code 工具名兼容别名

VS Code 1.143 的 MCP 客户端只接受 `[a-z0-9_-]` 工具名，而规范工具名包含点号。HTTP 客户端可发送：

```text
X-HPA-Tool-Name-Style: slug
```

- 只影响该请求的 `tools/list` 名称与 `tools/call` 解析；stdio 与未带该头的客户端保持规范名。
- `.` 等非法字符转为 `_`，冲突追加 `_2`/`_3`；`_meta.hpa/canonicalName` 保留规范名。
- 示例：`workflow.list` → `workflow_list`，`skill.invoice-audit.audit` → `skill_invoice-audit_audit`。
- 审批记录仍绑定规范工具名；Phase 3 现有测试不改变。实现与 VS Code 证据见 [`docs/14-vscode-copilot/local-implementation.md`](../14-vscode-copilot/local-implementation.md)。

## 3. 注册表

- 工具：已发布 Skill 脚本（`hpa/risk=medium`）、`workflow.execute`（`high`，`approval=always`）、`workflow.list`、`execution.get`。
- 资源：`hpa://tenant/local/skills/{id}/versions/{version}`、workflows、executions、artifacts。
- Prompt：`skill.{name}`、`workflow.review`。
- 令牌 scope：`mcp:read` 覆盖发现/列表/读取；`mcp:execute` 覆盖工具调用。

## 4. 审批与治理

- 高风险调用首次返回 MRTR `input_required`，并写入 `mcp_approvals`；审批绑定 token、工具名与参数哈希，10 分钟过期，只能消费一次。
- 客户端重试携带 `requestState`；客户端提供的 `inputResponses` 不作为授权依据，必须由本地审批中心批准。
- 每 token 进程内滑动窗口限流，默认 120/min，`HPA_MCP_RATE_LIMIT` 可覆盖；超限 429 + `Retry-After`。
- 审计写入 `mcp_audit`；MCP Center 显示工具/Prompt/Resource、审批、令牌、外部服务器与最近审计。

## 5. MCP Client

- stdio：启动声明的命令，legacy initialize、tools/resources/prompts 列表与调用；凭证通过环境变量注入，不进入参数。
- HTTP：先尝试 modern discover，失败回退 legacy initialize；默认只允许 loopback，远端主机必须配置 allowlist，拒绝 userinfo 与 `169.254.169.254` 等 metadata 地址；重定向关闭。
- 工具 allowlist 在探测与调用两侧执行；上游凭证只发往 MCP endpoint，不向工具参数透传。

## 6. PoC B 证据

- Claude Code：`claude mcp add ... --transport http --header "Authorization: Bearer ..."` 后 `claude mcp list` 显示 `✔ Connected`，服务端审计记录 `initialize` 与 `tools/list`。
- Codex：`codex mcp add ... --bearer-token-env-var HPA_MCP_TOKEN` 后 `codex debug prompt-input` 连接服务端，审计记录 `initialize` 与 `tools/list`。
- 只读工具调用由 hpa MCP Client 在同一 Gateway 上完成（`workflow.list` 与 Skill 脚本）；外部模型会话未自动执行，避免未授权的模型/凭证消耗。

### 6.1 官方 conformance

固定使用 npmmirror 下载的 `@modelcontextprotocol/conformance@0.2.0-alpha.12`：

```bash
cargo build -p hpa-local
pnpm test:mcp-conformance
pnpm test:mcp-conformance:2026
pnpm test:mcp-conformance:2026-content
pnpm test:mcp-conformance:2026-mrtr
```

脚本建立临时 Runtime、PAT，并通过只监听 loopback 的认证代理运行官方 server 场景。当前 2025-11-25 核心矩阵 9/9 场景 PASS：

- `server-initialize`
- `ping`
- `logging-set-level`
- `tools-list`
- `tools-call-simple-text`
- `tools-call-error`
- `resources-list`
- `prompts-list`
- `dns-rebinding-protection`

2026-07-28 的 `server-stateless` 场景 28/28 PASS（0 failed，2 warnings），覆盖 per-request `_meta` 校验、`server/discover`、版本/Header 错误码（-32020/-32022）、缺失客户端能力（-32021）、移除方法 404/-32601、订阅确认与 subscriptionId 标记。

`HPA_MCP_CONFORMANCE=1` 只用于该临时进程，额外注册 `test_simple_text`、`test_error_handling`、`test_missing_capability`、`test_streaming_elicitation`、`test_logging_tool`、`test_trigger_tool_change` 以及内容、Header、MRTR fixture；未设置时测试工具不会出现在工具目录。结果保存在 `target/mcp-conformance/<timestamp>/`，每个场景都有 `checks.json`。

MRTR fixture 覆盖 8 个工具与 1 个 prompt：elicitation、sampling、`roots/list`、requestState 完整性、多输入、多轮、能力过滤、缺失/非法响应与 `prompts/get`；`test:mcp-conformance:2026-mrtr` 的 14 个场景、40 项检查全部 SUCCESS（0 failed、0 warnings）。

完整 2026 requirements 可通过 `HPA_CONFORMANCE_REQUIREMENTS=2026-07-28 pnpm test:mcp-conformance` 运行；工具媒体内容、Resources/Prompts、completion、caching、JSON Schema、Header 与 MRTR 已由专项脚本覆盖，计分 server 场景 37/37。总报告为 162 passed / 29 failed，失败全部位于不计分的 tasks extension（SEP-2663）。

## 7. 已知边界

- OAuth AS/PKCE/DCR 已由 Phase 10 提供；设置 `HPA_POSTGRES_MCP=1` 后 session/subscription/task 持久化，notification 支持跨节点轮询、慢消费者断开和 sequence 重放。未启用时仍为单进程内存路径。
- legacy 会话在 PostgreSQL 模式可跨重启保留；未启用 PostgreSQL 时只存在内存，重启后需要重新 initialize。
- 2026-07-28 官方计分 server requirements 37/37 已通过（含 MRTR 14 场景）；tasks extension 不计分且未实现，见 [Phase 25](../11-roadmap/phase-25-mcp-mrtr-conformance.md)。
- 管理 API 依赖 loopback；生产需增加本地登录/CSRF 防护。
- 外部客户端真实模型会话中的工具调用尚未在本机自动验证。
