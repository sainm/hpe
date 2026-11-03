# API 规范

> 风格：REST + JSON + OpenAPI 3.1；执行事件用 SSE；MCP 是独立协议端点，不混入 REST。
>
> 当前本地实现使用无版本前缀的 `/api/*`；`schema/openapi/local-api.json` 覆盖 `ApiState::router` 的全部路径和方法，并由 `/api/openapi.json` 暴露，`pnpm check:contracts` 同时检查路由/方法漂移。下文 `/api/v1` 是目标云契约。
> 版本：`/api/v1`；破坏性变更走 v2。内部 Worker API 使用 `/internal/v1` + mTLS/JWT。

## 1. 基础

| 项 | 约定 |
| --- | --- |
| Base URL | `https://api.hpa.example.com/api/v1` |
| 认证 | `Authorization: Bearer <OIDC/PAT>`；服务账号同理 |
| 租户 | token 绑定；路径可含 workspace/project；不接受客户端任意 tenant |
| 内容 | `application/json`；问题响应 `application/problem+json` |
| 版本 | URL `/v1`；OpenAPI `info.version` |
| 时间 | RFC 3339 UTC |
| ID | UUIDv7 + 前缀（`wf_`/`exec_`/`skill_`） |
| 追踪 | `X-Request-Id`、`traceparent` 透传 |
| 幂等 | `Idempotency-Key`（POST 创建/执行） |
| 并发 | `ETag`/`If-Match`（可编辑资源） |
| 分页 | cursor：`?limit=50&cursor=...`；响应 `nextCursor` |
| 过滤/排序 | 明确白名单；不接受任意 SQL |
| 限流 | `X-RateLimit-*`；429 + `Retry-After` |

## 2. 错误模型

```json
{
  "type": "https://hpa.dev/problems/validation",
  "title": "Validation failed",
  "status": 422,
  "code": "IR_VAR_UNBOUND",
  "detail": "变量 orderId 未绑定",
  "instance": "/api/v1/workflows/wf_.../validate",
  "traceId": "...",
  "errors": [{ "path": "nodes/2/config/url", "message": "..." }],
  "remediation": "在 Start.inputs 或上游 outputs 中定义 orderId"
}
```

- 错误码稳定；不泄漏 secret/内部路径。
- 4xx 客户端错误；5xx 服务端；`remediation` 可操作。
- 批量校验返回 `issues[]`（error/warning）。

## 3. 资源端点

### 组织

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/me` | 当前用户/租户/权限 |
| GET | `/tenants` | 可访问租户 |
| GET/POST | `/workspaces` | 列表/创建 |
| GET/POST | `/projects` | 列表/创建 |
| GET/POST | `/members` | 成员/角色 |
| GET | `/roles` | 角色/权限 |

SCIM 2.0 使用独立租户 Bearer 端点：

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/scim/v2/ServiceProviderConfig` | PATCH/filter/ETag 能力 |
| GET/POST | `/scim/v2/Users` | 用户查询/创建 |
| GET/PUT/PATCH/DELETE | `/scim/v2/Users/{id}` | 用户生命周期 |
| GET/POST | `/scim/v2/Groups` | 组查询/创建 |
| GET/PUT/PATCH/DELETE | `/scim/v2/Groups/{id}` | 组和成员生命周期 |

完整映射、角色冲突和 If-Match 语义见 [SCIM](scim.md)。

### Workflow

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET/POST | `/workflows` | 列表/创建 |
| GET/PATCH/DELETE | `/workflows/{id}` | 草稿/元数据 |
| GET | `/workflows/{id}/versions` | 版本列表 |
| POST | `/workflows/{id}/validate` | 五类校验 |
| POST | `/workflows/{id}/publish` | 发布（需校验通过） |
| POST | `/workflows/{id}/import` | YAML/JSON/外部格式 |
| GET | `/workflows/{id}/export?format=yaml` 或 `format=json` | 导出 |
| POST | `/workflows/{id}/ai-generate` | AI 生成草稿 |
| GET/PUT | `/workflows/{id}/draft` | 草稿 IR |
| GET | `/node-types` | 节点注册表/版本 |

### Execution

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| POST | `/workflows/{id}/executions` | 启动；支持 `Idempotency-Key`，相同内容重放原 execution，不同内容返回 409 |
| GET | `/executions` | 列表/过滤 |
| GET | `/executions/{id}` | 状态/输入输出摘要 |
| GET | `/executions/{id}/events` | 历史事件（分页） |
| GET | `/executions/{id}/events/stream` | SSE 实时 |
| POST | `/executions/{id}/cancel` | 等待审批立即取消；运行中持久化协作取消请求，在步骤边界停止 |
| POST | `/executions/{id}/approvals` | 批准、拒绝或超时恢复；同一节点/决定幂等 |
| POST | `/executions/{id}/retry` | 使用原快照、输入和环境创建新 execution；支持 `Idempotency-Key` |
| GET | `/artifacts?executionId={id}` | 产物 |

### Agent / Skill / MCP / Plugin

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET/POST | `/agents`、`/agents/{id}/versions`、`/agents/{id}/test` | Agent |
| GET/POST | `/skills`、`/skills/{id}/versions`、`/skills/{id}/test` | Skill |
| POST | `/skills/import`、GET `/skills/{id}/export` | 导入/导出 |
| GET/POST | `/mcp/servers`、`/mcp/servers/{id}/discover`、`/mcp/servers/{id}/tools/{tool}/call` | MCP Client 管理 |
| GET/POST | `/plugins`、`/plugins/{id}/install`、`/plugins/{id}/upgrade` | 插件 |
| GET/POST | `/capabilities`、`/capabilities/{id}/bindings` | 能力注册 |
| GET/POST | `/connectors`、`/connections`、`/connections/{id}/test` | 连接器/凭证 |
| GET/POST | `/approvals`、`/approvals/{id}/decide` | 审批 |
| GET | `/audit` | 审计（权限） |
| GET/POST | `/triggers`、`/schedules` | 触发器/定时 |

> 具体字段以 OpenAPI 为准；上表是控制面范围，不是全部。

## 4. 请求示例

### 发布

```http
POST /api/v1/workflows/wf_order/validate
Content-Type: application/json

{ "ir": { "schemaVersion": "hpa.dev/v1", "...": "..." } }
```

```http
POST /api/v1/workflows/wf_order/publish
If-Match: "wfv-draft-7"
Content-Type: application/json

{ "version": "1.2.0", "comment": "add approval" }
```

```json
{
  "workflowVersionId": "wfv_01J...",
  "version": "1.2.0",
  "irHash": "sha256:...",
  "publishedAt": "2026-11-01T00:00:00Z"
}
```

### 执行

```http
POST /api/v1/workflows/wf_order/executions
Idempotency-Key: order-sync-2026-11-01
Content-Type: application/json

{ "version": "1.2.0", "inputs": { "date": "2026-11-01" } }
```

```json
{ "executionId": "exec_01J...", "status": "QUEUED" }
```

### SSE

```http
GET /api/v1/executions/exec_01J.../events/stream
Accept: text/event-stream
Last-Event-ID: 42
```

```text
id: 43
event: node.succeeded
data: {"nodeId":"fetch_orders","attempt":1,"at":"..."}
```

- 断线重连用 `Last-Event-ID`（Hisha 事件）；与 MCP SSE 无关。
- 心跳注释；超时关闭；事件保留窗口。

## 5. 审批

```http
POST /api/executions/exec_01J.../approvals
Content-Type: application/json

{ "nodeId": "approve", "decision": "approved", "form": { "decision": "approve", "comment": "ok" } }
```

- 幂等：execution + node + decision；重复返回当前结果，相反决定返回 `APPROVAL_ALREADY_DECIDED`。
- 拒绝/超时按执行策略；审计。
- 高风险需要 step-up 认证（可配置）。

## 6. Webhooks

- 出站：执行/审批/发布事件；签名 `X-Hisha-Signature`（HMAC + timestamp）；重试/退避；去重 ID。
- 入站：Workflow Webhook Trigger；签名/重放保护；租户路由。
- 不把 secret 放 URL；使用 header/签名。

## 7. 内部 Worker API

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| POST | `/internal/v1/workers/register` | 注册（令牌/mTLS） |
| POST | `/internal/v1/workers/heartbeat` | 心跳/容量 |
| POST | `/internal/v1/jobs/lease` | 拉取/租约 |
| POST | `/internal/v1/jobs/{id}/start` | 开始 |
| POST | `/internal/v1/jobs/{id}/events` | 批量事件（幂等） |
| POST | `/internal/v1/jobs/{id}/complete` | 完成 |
| POST | `/internal/v1/jobs/{id}/fail` | 失败 |
| GET | `/internal/v1/executions/{id}/cancel` | 取消检查 |

- mTLS/JWT；租户绑定；只允许注册 Worker；不接受公网。
- 事件批量 + sequence；重复去重；大小限制。

## 8. MCP

- 独立端点 `/mcp`（Streamable HTTP）与 stdio；协议见 [MCP Server](../06-mcp/mcp-server.md)。
- 不复用 REST 认证中间件假设；OAuth 2.1/PAT；工具 schema。
- 不把内部 REST 路径作为工具名。

## 9. SDK/开放

- TypeScript SDK 从 OpenAPI 生成 + 手写运行时（流式/重试/类型）。
- Python/Java SDK 后续；先保证 OpenAPI 稳定。
- OpenAPI 3.1 + JSON Schema 2020-12；错误码/示例/安全方案。
- 版本化：新增字段可选；删除/改语义走 v2。

## 10. 安全/治理

- 认证/授权/租户/限流/审计见安全架构。
- 请求体大小/深度限制；文件上传类型/大小；URL 白名单。
- 不返回 secret；敏感字段脱敏。
- CORS 白名单；CSRF（cookie 会话）；Origin 校验。
- 管理 API 与业务 API 分离；高风险操作 step-up。

## 11. 相关文档

- [数据模型](data-model.md)
- [权限模型](permission-model.md)
- [Control Plane](../02-architecture/control-plane.md)
- [MCP Server](../06-mcp/mcp-server.md)
- [安全架构](../02-architecture/security-architecture.md)
