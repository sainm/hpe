# 权限模型

> 默认拒绝；每个入口独立评估；AI/客户端不能绕过。权限 = RBAC（角色/范围）+ ABAC（资源属性/风险/环境）+ 审批义务。

## 1. 主体

| 主体 | 说明 |
| --- | --- |
| User | 人类用户（OIDC/本地） |
| ServiceAccount | 服务/CLI/Worker |
| PAT | 用户/服务账号的短期令牌 |
| Worker | mTLS/JWT 身份 + 租户绑定 |
| ExternalClient | 通过 MCP/OAuth 的外部客户端（映射到用户/服务账号） |
| System | 平台内部（迁移/审计），最小使用 |

## 2. 作用域

`Tenant` → `Workspace` → `Project` → `Resource`。

- 权限在作用域上授予；子作用域继承（可收紧，不可扩大）。
- 跨作用域共享需显式 grant；跨租户默认拒绝。
- 资源归属：owner/project/workspace/tenant；公开市场资源只读。

## 3. 角色（默认）

| 角色 | 典型权限 |
| --- | --- |
| Owner | 租户管理、计费、删除；不能绕过审计 |
| Admin | 工作区/项目/成员/连接器/模型；策略 |
| Developer | Workflow/Agent/Skill 创建/编辑/测试；发布需权限 |
| Publisher | 发布/版本/市场提交 |
| Operator | 执行/查看/取消/重试；不能改定义 |
| Approver | 审批；不能发布/执行（可配置） |
| Auditor | 只读审计/执行/配置；不可修改 |
| Viewer | 只读 |

角色可自定义；权限点细粒度。角色不是安全边界本身，策略才是。

## 4. 权限点

| 资源 | 动作 |
| --- | --- |
| Workflow | read/create/update/delete/validate/publish/execute/export/import |
| Execution | read/cancel/retry/approve/delete-artifact |
| Agent | read/create/update/delete/test/publish/run |
| Skill | read/create/update/delete/test/publish/export/import/run |
| MCP | read/register/update/delete/discover/invoke/approve |
| Connector/Credential | read/create/update/delete/test/use |
| Plugin | read/install/upgrade/uninstall/publish |
| Capability | read/register/update/delete/bind |
| Approval | read/decide/delegate |
| Audit | read/export |
| Tenant/Workspace | manage/member/role/quota/settings |

## 5. 策略评估

```ts
interface PolicyRequest {
  principal: Principal;
  action: string;
  resource: ResourceRef;
  context: {
    tenantId: string;
    workspaceId?: string;
    projectId?: string;
    risk?: "low" | "medium" | "high" | "critical";
    entrypoint: "api" | "workflow_node" | "agent_tool" | "mcp_tool" | "skill_script" | "plugin";
    environment?: string;
    ip?: string;
    time?: string;
    attributes?: Record<string, unknown>;
  };
}

interface PolicyDecision {
  effect: "allow" | "deny" | "require_approval";
  reasons: string[];
  obligations?: { approvalId?: string; ttlMs?: number; maskedFields?: string[] };
}
```

评估顺序：

1. 认证/租户匹配（不匹配 → deny）。
2. 显式 deny（优先级最高）。
3. RBAC 允许？
4. ABAC 条件（风险/环境/归属/标签/时间/IP）。
5. 高风险 → require_approval（附审批策略）。
6. 默认 deny。

## 6. 入口独立

同一能力不同入口独立评估：

| 入口 | 示例 | 额外检查 |
| --- | --- | --- |
| API | `workflow.execute` | scope/角色/租户 |
| Workflow Node | HTTP/Connector | 节点权限 + 连接凭证 + 发布授权 |
| Agent Tool | Connector/Workflow/MCP | 工具风险/预算/审批 |
| MCP Tool | 外部客户端 | OAuth scope/租户/风险/审批 |
| Skill Script | 文件/网络/命令 | 沙箱/权限清单/审批 |
| Plugin | 安装/能力 | 签名/权限/管理员批准 |

Node 有权限不代表 Tool/MCP 入口有权限；反之亦然。

## 7. 审批

- 触发：`risk >= high`、写/删除/发布/执行脚本/数据导出/权限变更（可配置）。
- 审批人：角色/用户/组；按资源归属/项目/租户。
- SLA/升级/超时（默认拒绝）。
- 审批义务绑定 toolCallId/executionId；不可复用/代签。
- 高风险 step-up 认证。
- 审批与审计独立；拒绝原因回传调用方。

## 8. 租户隔离

- token → tenant；请求路径/资源必须匹配。
- 所有查询 `tenant_id`；RLS 兜底。
- 跨租户 ID 直接 404/403（不泄漏存在性）。
- 平台管理员跨租户操作需独立角色 + 审计 + 理由。
- 缓存/搜索/向量索引按租户分区。

## 9. 共享/公开

- 工作区内共享：显式 grant（read/use/clone）。
- 公开市场：发布只读能力/模板；不共享凭证/执行数据。
- 克隆：复制定义，不复制凭证/历史；权限重新评估。
- 公开 MCP 工具：只读/受限；写操作仍需授权。

## 10. 配额与权限

- 配额是资源限制，不是权限；即使 allow 也可能因配额拒绝。
- 预算（成本/Token）在 Agent/Workflow/租户级；超限拒绝/降级。
- 并发/限流按租户/入口。

## 11. 审计

- 记录：principal/action/resource/decision/reasons/obligations/tenant/entrypoint/trace。
- 权限变更/审批/拒绝/跨租户尝试。
- 审计不可由普通管理员删除；导出需权限。
- 策略版本化；执行快照绑定策略版本。

## 12. 示例

```json
{
  "principal": { "type": "user", "id": "usr_1", "tenantId": "ten_1" },
  "action": "workflow.execute",
  "resource": { "type": "workflow_version", "id": "wfv_1", "tenantId": "ten_1", "projectId": "prj_1" },
  "context": { "entrypoint": "mcp_tool", "risk": "high", "environment": "prod" }
}
```

决策：`require_approval`，reason `risk=high and entrypoint=mcp_tool`；创建 Approval；绑定 execution/toolCall。

## 13. 测试

- 角色/范围/继承/显式 deny。
- 跨租户/跨工作区/跨项目。
- Node vs Tool vs MCP 入口差异。
- 高风险审批/拒绝/超时/重复。
- 共享/克隆/公开资源。
- 配额/预算。
- 审计完整性；策略版本。

## 14. 相关文档

- [安全架构](../02-architecture/security-architecture.md)
- [能力注册中心](capability-registry.md)
- [MCP 安全](../06-mcp/mcp-security.md)
- [Skill 安全](../05-skill/skill-security.md)
- [数据模型](data-model.md)
