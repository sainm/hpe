# SCIM 2.0 用户、组与角色映射

> 状态：本地实现与自动化验收 **PASS**；真实企业 IdP/SCIM 租户联调 **BLOCKED_EXTERNAL**。本实现不把 mock 或本地 HTTP 测试计入真实企业目录验收。

## 1. 范围

HPA 提供 SCIM 2.0 Users/Groups 资源，用于企业目录向租户成员目录推送用户、停用账号和维护组成员关系。

- 租户绑定 Bearer token；只接受 `owner`、`admin` 或 platform admin。
- Users 映射 `enterprise_members`；SCIM `externalId` 推荐填写 OIDC `sub`，登录时优先按 subject 匹配。
- Groups 保存成员关系和可选固定 HPA 角色映射。
- 支持 POST/GET/PUT/PATCH/DELETE、精确 `eq` filter、分页、PATCH 和弱 ETag。
- Bulk、密码管理、排序、复杂 filter、组嵌套和多 IdP 不在本实现范围。

SCIM 端点不是匿名发现服务。`ServiceProviderConfig`、`ResourceTypes` 和 `Schemas` 同样要求租户 Bearer token。

## 2. 认证与租户

```http
Authorization: Bearer hpa_ent_...
Content-Type: application/scim+json
Accept: application/scim+json
```

认证顺序复用企业 API：

1. 解析 `hpa_ent_` token，校验撤销、过期与 tenant active。
2. 要求 token role 为 `owner`/`admin`，或 `platformAdmin=true`。
3. 所有查询和写入绑定 token 的 `tenant_id`；资源 ID 不能跨租户解析。
4. 生产必须使用 HTTPS。Bearer token 不得放在 URL、日志或 SCIM payload。
5. 只接受显式 `hpa_ent_` token；普通 OIDC JWT、浏览器 Cookie 或 OAuth MCP access token 不能用于 SCIM provisioning。
6. admin token 不能授予或移除 `owner` 角色，也不能修改 effective role 为 owner 的 User 或映射到 owner 的 Group；这些动作只允许 tenant owner 或 platform admin。

platform admin token 的租户仍由 token 的 `tenant_id` 决定。为其他租户自动开通账号时，应创建该租户的 owner/admin 专用 token，不应复用全局管理员 token。

## 3. 端点

| Method | Path | 说明 |
| --- | --- | --- |
| GET | `/scim/v2/ServiceProviderConfig` | Patch/filter/ETag 能力 |
| GET | `/scim/v2/ResourceTypes` | User、Group 资源类型 |
| GET | `/scim/v2/Schemas` | Core User/Group 与角色扩展 |
| GET/POST | `/scim/v2/Users` | 列表、filter、创建 |
| GET/PUT/PATCH/DELETE | `/scim/v2/Users/{id}` | 用户生命周期 |
| GET/POST | `/scim/v2/Groups` | 列表、filter、创建 |
| GET/PUT/PATCH/DELETE | `/scim/v2/Groups/{id}` | 组和成员关系生命周期 |

成功响应 `Content-Type: application/scim+json`。创建返回 `201 + Location`，删除返回 `204`。

### 3.1 Filter 与分页

只支持单个精确比较：

```text
userName eq "user@example.com"
externalId eq "oidc|subject"
active eq true
displayName eq "Auralis Developers"
```

不支持 `co`、`sw`、`and`、`or`、括号或未声明属性；返回 `400 invalidFilter`。`startIndex` 从 1 开始，`count` 范围 0～200，响应使用标准：

```json
{
  "schemas": ["urn:ietf:params:scim:api:messages:2.0:ListResponse"],
  "totalResults": 1,
  "itemsPerPage": 1,
  "startIndex": 1,
  "Resources": []
}
```

### 3.2 并发控制

User/Group 的 `meta.version` 同时作为 `ETag` 响应头。PUT/PATCH/DELETE 可带：

```http
If-Match: W/"<resource-version>"
```

版本不匹配返回 `412 Precondition Failed`；`If-Match: *` 接受当前版本。未提供 `If-Match` 仍允许执行，以兼容未实现 ETag 的目录。

## 4. User 映射

示例：

```json
{
  "schemas": [
    "urn:ietf:params:scim:schemas:core:2.0:User",
    "urn:hpa:params:scim:schemas:extension:role:1.0"
  ],
  "userName": "alice@example.com",
  "externalId": "oidc|alice",
  "displayName": "Alice",
  "name": {
    "givenName": "Alice",
    "familyName": "Example",
    "formatted": "Alice Example"
  },
  "emails": [
    { "value": "alice@example.com", "type": "work", "primary": true }
  ],
  "active": true,
  "urn:hpa:params:scim:schemas:extension:role:1.0": {
    "role": "auditor"
  }
}
```

映射：

- `id` ↔ `enterprise_members.id`
- `userName` ↔ `user_name`
- `externalId` ↔ `external_id`，并作为 OIDC subject 匹配值
- `displayName`、primary email、`active` ↔ 列字段
- 角色扩展 `role` ↔ 直接角色 `base_role`
- 其他 User 属性保存在 `scim_json`，GET 时尽量原样返回
- `groups`、`meta`、`schemas`、`id` 为服务端生成字段

没有 `externalId` 时，`userName` 作为 subject。若企业 IdP 的 OIDC subject 不是邮箱，必须配置 `externalId=sub`，否则网页 SSO 无法可靠关联 SCIM 用户。

## 5. Group 与角色冲突

Group 使用同一角色扩展：

```json
{
  "schemas": [
    "urn:ietf:params:scim:schemas:core:2.0:Group",
    "urn:hpa:params:scim:schemas:extension:role:1.0"
  ],
  "displayName": "Auditors",
  "externalId": "group-auditors",
  "urn:hpa:params:scim:schemas:extension:role:1.0": {
    "role": "auditor"
  },
  "members": [
    { "value": "entmem_..." }
  ]
}
```

角色解析规则：

1. 用户保存一个直接角色 `base_role`，默认 `viewer`。
2. 用户属于一个或多个未映射角色的 Group 时，直接角色生效。
3. 用户属于一个映射角色的 Group 时，该组角色覆盖直接角色。
4. 用户同时属于映射到不同角色的 Group 时，新增/更新以 `409 invalidValue` 失败，整个事务回滚。
5. 删除映射 Group 或移除成员后，重新计算有效角色；没有映射组时恢复 `base_role`。
6. 成员角色受 Group 管理时，Enterprise 成员 API 直接修改 `role` 返回 `409 MEMBER_ROLE_MANAGED_BY_GROUP`。

该策略避免“多组角色取最高权限”带来的隐式提权，也避免复合角色无法映射到现有固定 RBAC。

## 6. PATCH

支持 `urn:ietf:params:scim:api:messages:2.0:PatchOp`，操作 `add`、`replace`、`remove` 大小写不敏感。

User 常用 path：

- `userName`
- `externalId`
- `displayName`
- `active`
- `emails`
- `urn:hpa:params:scim:schemas:extension:role:1.0`
- 其他对象属性，如 `title`、`name.givenName`

Group 常用 path：

- `displayName`
- `externalId`
- 角色扩展
- `members`
- `members[value eq "<member-id>"]` 的 `remove`

Role、`groups`、`meta`、`schemas`、`id` 的只读和可写边界按资源类型校验；未知 `op`、非法 path 或错误类型返回 `400 invalidValue`/`invalidSyntax`。

## 7. 存储

SQLite `control.db`：

```text
enterprise_members
  id, tenant_id, subject, user_name, external_id,
  display_name, email, role, base_role,
  scim_json, status, created_at, updated_at

enterprise_scim_groups
  id, tenant_id, display_name, external_id, role,
  scim_json, created_at, updated_at

enterprise_scim_group_members
  tenant_id, group_id, member_id, created_at
```

PostgreSQL migration `0010_scim_directory.sql`～`0013_scim_unique_email.sql` 提供同义表、FORCE RLS、同租户复合外键、时间戳对齐、角色回填与邮箱唯一约束，以及运行时/平台/只读角色授权。

升级到邮箱唯一约束前，应审计已有成员：

```sql
SELECT tenant_id, lower(email), count(*)
FROM hpa_control.members
WHERE email IS NOT NULL
GROUP BY tenant_id, lower(email)
HAVING count(*) > 1;
```

存在重复时必须先由业务确认主成员并人工修正；migration 不得自动合并身份或角色。

## 8. 安全与错误语义

错误使用：

```json
{
  "schemas": ["urn:ietf:params:scim:api:messages:2.0:Error"],
  "scimType": "uniqueness",
  "detail": "SCIM resource conflicts with an existing value",
  "status": "409"
}
```

- userName 在同租户大小写不敏感唯一。
- externalId 在同租户唯一；email 不得关联多个成员。
- Group displayName 在同租户大小写不敏感唯一。
- 任何 Group 变更与角色重新计算处于同一数据库事务。
- `scim_json` 不包含 token、密码或认证秘密。
- 审计记录 `scim.user.create/update/delete` 与 `scim.group.create/update/delete`，不记录 Bearer token。

## 9. 验收

Rust HTTP 集成测试：

```bash
cargo test -p hpa-local --test enterprise \
  scim_users_groups_filters_etags_and_role_mapping_are_fail_closed -- --nocapture
```

覆盖：认证边界、租户隔离、create/filter/PATCH/delete、If-Match 412、重复 userName、跨租户 ID、角色冲突回滚、删除组恢复直接角色。

可重复 E2E：

```bash
HPA_BASE_URL=http://127.0.0.1:8787 \
HPA_TOKEN_FILE=/secure/scim-tenant-owner.token \
pnpm test:scim-e2e
```

该脚本创建唯一测试 User/Group，验证 filter、Group 角色覆盖、PATCH 非索引属性、Group 删除后角色恢复，并清理测试资源。

## 10. 外部待验

以下项目需要企业提供 SCIM 测试租户、权限和网络出口，当前保持 `BLOCKED_EXTERNAL`：

- Entra ID、Okta、Keycloak 或客户 IdP 的真实 SCIM 推送。
- IdP 的 PATCH 方言、分页上限、TLS/mTLS/reverse proxy header 与限流策略。
- 真实 SCIM token 轮换和 401/403 重试行为。
- 企业目录删除/停用策略与 HPA 成员/RBAC 的联合验收。

本地测试能证明协议、状态机、事务和负向边界，不证明上述企业环境已验收。
