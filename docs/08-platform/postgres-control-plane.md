# PostgreSQL 控制面基线

> 状态：**P13-01 IN_PROGRESS**。schema、SQLx migration、角色分离、RLS、SQLite 导入、Workflow/Execution/Trigger Repository 已在 PostgreSQL 18 执行通过；`HPA_POSTGRES_RUNTIME=1` 时 workflow/version、execution/event/artifact、trigger_state/trigger_fires、execution lease、remote marker、Worker Job、平台令牌/审计、浏览器会话、OAuth 2.1 AS、企业租户/配额/成员/SCIM Group 与 Agent 版本/会话/事件/审批/工具调用已切换并可跨进程读取；SQLite 控制表作为可重建缓存。Skill 元数据仍使用 SQLite，不能宣称生产控制面已整体切换。

## 1. 边界

Phase 13 的控制面目标是一个 PostgreSQL 数据库，不按租户拆库：

| Schema | 内容 | 隔离方式 |
| --- | --- | --- |
| `hpa_control` | Tenant、令牌、OIDC、OAuth、成员、Worker、任务、配额、审计 | `tenant_id` + FORCE RLS；全局 OIDC 和签名密钥不授予租户角色 |
| `hpa_tenant` | Workflow、Execution、Event、Artifact、Skill、Agent、MCP、Plugin、Connector、Trigger | 复合主键/外键 + `tenant_id` + FORCE RLS |
| `hpa_internal` | 租户上下文函数 | 仅执行权限，不存放业务数据 |

每个 `hpa_tenant` 表同时通过外键引用 `hpa_control.tenants(id)`；子资源还必须使用包含 `tenant_id` 的复合外键，防止伪造租户内父子关系。

当前已覆盖 66 张租户相关表：`hpa_tenant` 40 张，`hpa_control` 中带租户范围的 26 张。`oidc_config`、`oauth_signing_keys` 和全局 `runtime_leases` 不属于租户表，敏感性各不相同，按角色显式授权。

## 2. 角色

所有角色均为 `NOLOGIN`、非超级用户、`NOBYPASSRLS`。部署时为具体 LOGIN 用户授予可 `SET ROLE` 的成员关系，避免长期以平台管理员身份创建连接。

| 角色 | 范围 | 写权限 |
| --- | --- | --- |
| `hpa_migrator` | schema/table owner，迁移专用 | DDL/DML |
| `hpa_tenant_runtime` | 当前 `hpa.tenant_id` | 租户业务数据和租户级控制数据 |
| `hpa_worker_runtime` | 当前 `hpa.tenant_id` | Worker 注册、心跳、任务租约，Workflow/Execution 只读，事件/Artifact 追加 |
| `hpa_platform_admin` | 所有租户 | 控制面管理；策略显式允许跨租户，不依赖 `BYPASSRLS` |
| `hpa_operator` | 所有租户 | 只读安全子集；不能读取令牌哈希、OAuth code、签名私钥 |
| `hpa_readonly` | 当前 `hpa.tenant_id` | 只读安全子集；不能写入或读取签名私钥 |

`hpa_tenant_runtime` 与 `hpa_worker_runtime` 不是 `hpa_platform_admin` 或 `hpa_operator` 的成员。

## 3. 迁移

迁移由 SQLx `Migrator` 执行，提供：

- PostgreSQL advisory lock，防止多个控制面同时迁移。
- 每个 migration 的 SHA-384 checksum；已应用文件变化会返回 `VersionMismatch`。
- 每个 migration 事务化执行；失败不会留下半应用版本。
- `public._sqlx_migrations` 记录版本、描述、checksum、成功状态和安装时间。

首次初始化需要具备 `CREATEROLE`/管理对象权限的 bootstrap 连接，因为 migration 会创建 NOLOGIN 角色并把 schema/table owner 转给 `hpa_migrator`。初始化后，日常迁移连接只应取得 `hpa_migrator` 的 `SET` 成员关系。

```bash
export HPA_POSTGRES_URL='postgresql://bootstrap@db.internal:5432/hpa'
cargo run -p hpa-local -- db migrate
cargo run -p hpa-local -- db status
```

重复运行 `db migrate` 必须输出 `"applied": 0`。命令不打印数据库 URL；也可使用 `DATABASE_URL` 作为后备环境变量。

已发布 migration 不得修改。需要修复时增加新版本；checksum 不一致必须停止发布并由人工恢复正确版本。

## 4. 租户上下文

应用连接池必须先 `SET ROLE hpa_tenant_runtime` 或 `hpa_worker_runtime`，并在同一个事务内设置租户：

```sql
BEGIN;
SELECT set_config('hpa.tenant_id', $1, true);
-- tenant-scoped queries
COMMIT;
```

- 必须使用 transaction-local `set_config(..., true)`；不能在池化连接上留下 session 级租户上下文。
- 未设置或设置为空字符串时，`hpa_internal.current_tenant()` 返回 `NULL`，租户表默认拒绝。
- 事务回滚或连接归还连接池时必须清除角色和 GUC。
- SQL RLS 是末级防线，不是应用授权、RBAC 或审批的替代品。
- 平台管理员由 policy 显式放行；普通租户角色不能通过 `SET ROLE` 获得该角色。

## 5. SQLite 导入

`db import-sqlite` 从只读模式打开源目录，并按租户事务导入：

```bash
export HPA_POSTGRES_URL='postgresql://bootstrap@db.internal:5432/hpa'
cargo run -p hpa-local -- db migrate
cargo run -p hpa-local -- db import-sqlite --source /srv/hpa-sqlite
```

源目录布局与本地运行时一致：

```text
/srv/hpa-sqlite/
  control.db
  hpa.db
  tenants/<tenant-id>/hpa.db
```

- 导入连接必须能 `SET ROLE hpa_platform_admin`；导入仍经过显式 RLS policy，不使用 `BYPASSRLS`。
- 每个业务库的 `tenant_id` 来自目录/控制库，不信任业务表内可能被伪造的 `tenant_id`。
- 目标已存在主键时 `ON CONFLICT DO NOTHING`，重复执行幂等；其他唯一约束冲突会失败，不会被吞掉。
- JSON 必须可解析，时间戳必须能转换，BLOB/整数/布尔按目标 PostgreSQL 类型严格转换。
- OIDC login flow、浏览器 session、OAuth request/code 属于短生命周期凭证，不迁移；升级后必须重新登录并在需要时重新授权。
- 导入不会覆盖目标已有主键。正式切换前应冻结源库，完成行数/哈希核对，再切换 API；当前仓库尚未实现自动切换和双写对账。

## 6. PostgreSQL execution runtime

启用执行事实的 PostgreSQL 路由：

```bash
export HPA_POSTGRES_URL='postgresql://runtime@db.internal:5432/hpa'
export HPA_POSTGRES_RUNTIME=1
hpa serve --port 8787
```

启动时校验 `hpa_tenant.executions` schema，并从 PostgreSQL 重建本机 SQLite 控制缓存（租户/配额/成员）；API 不再把 SQLite 写回 PostgreSQL。随后以下操作进入 PostgreSQL：

- API 创建 execution、RuntimeManager 读取/保存 state 与 events。
- Workflow draft upsert、publish、版本冻结、列表和详情读写。
- API execution 列表/详情/event stream 和 approval waiting 扫描。
- 执行输出 artifact 的写入、列表和读取。
- Trigger publish state、due schedule、schedule claim、启停、execution note 和 trigger fire 幂等记录。
- `Stats`、每日执行配额和并发 execution 配额计数。
- Execution lease、renew、takeover 和 fenced state commit，防止两个控制面同时驱动同一 running execution。
- MCP 与 Scheduler 通过同一租户 `Db` 创建 execution 时使用 PostgreSQL。

创建 execution 时在同一 PostgreSQL 事务中幂等物化 workflow/version snapshot。已存在的同一 `workflow_id@version` 若 IR 内容不同则返回 `WorkflowSnapshotConflict`，不覆盖冻结版本；因此 execution 外键不能依赖未导入的 workflow。

API 不接受请求体切换 tenant：`tenantId` 若存在必须等于认证租户，否则返回 400 `TENANT_MISMATCH`；Repository 再次校验 state 内嵌 tenant，防止绕过 API 的伪造。

执行模式中的 `remote_executions` marker 同步写入 PostgreSQL；Hybrid Worker 写入 execution 后，本地 recovery 不会接管该远程执行。Workflow draft/current row 与 frozen version 均写 PostgreSQL。当前限制：Skill 元数据与部分 Agent 工具实现仍依赖 SQLite；跨节点 Agent 使用 Skill 前需在目标节点导入 Skill。此模式不证明 P13-01 完成。

Execution owner 使用 `HPA_NODE_ID`、`HPA_EXECUTION_LEASE_TTL_SECONDS`（默认 30 秒）和 `HPA_EXECUTION_RECOVERY_TICK_MS`（默认 5000）。每个 step 前续租，state/events 更新必须匹配 holder + fencing token 且 lease 未过期。节点故障后其他控制面扫描 running execution，lease 过期后递增 token 接管；旧 owner 的后续 commit/release 失败。

## 7. 验证

真实 PostgreSQL 18 集成测试：

```bash
createdb hpa_p13_test
export HPA_POSTGRES_TEST_URL='postgresql://localhost:5432/hpa_p13_test'
cargo test -p hpa-local --test postgres_rls -- --nocapture
cargo test -p hpa-local --test postgres_import -- --nocapture
cargo test -p hpa-local --test postgres_execution_store -- --nocapture
cargo test -p hpa-local --test postgres_execution_lease -- --nocapture
cargo test -p hpa-local --test postgres_execution_recovery -- --nocapture
cargo test -p hpa-local --test postgres_runtime -- --nocapture
```

测试覆盖：

- 66 张租户表均 `relrowsecurity=true`、`relforcerowsecurity=true` 且至少有一个 policy。
- Alpha 只能读写 Alpha；Beta 行、跨租户 ID 更新和删除均为 0。
- 跨租户 INSERT 与未设置租户上下文均 fail-closed。
- 同一事务切换 `hpa.tenant_id` 只能看到对应租户，验证连接复用不复用旧上下文。
- `hpa_readonly` 不能写入或读取 OAuth 签名密钥。
- runtime 角色不是 platform admin/operator 的成员，且不能绕过 RLS。
- 修改已应用 migration 内容会返回 checksum mismatch。
- SQLite fixture 导入控制面/业务面数据，强制覆盖业务 `tenant_id`，校验 JSON/Blob/时间戳，并验证第二次导入新增数为 0。
- Execution store 完成 insert/get/save、event 顺序、输出 artifact、状态计数和跨租户不可见；相同 workflow/version 的不同 snapshot、state 内伪造 tenant 或 event 指向其他 execution 均失败关闭。
- Workflow store 完成 draft upsert、publish、current/list/version、workflow count 和冻结版本冲突；跨租户不可见。
- Trigger store 完成 publish state、due/claim、启停、execution note、fire 历史、Event ID 唯一约束、republish 保留 last state 和跨租户不可见。
- `hpa serve` + `HPA_POSTGRES_RUNTIME=1` 完成 API 创建/发布/执行；PostgreSQL 中存在 execution、events、artifact，强停并重启后仍可从 API 读取 execution 和 events。
- 同一 `hpa serve` 发布 event trigger 后，`/api/events/{topic}` 在 PostgreSQL 写 trigger_state、trigger_fires 和 execution；重复 Event ID 返回 409 且 fire 行数保持 1。
- Empty-data-dir 控制面可从 PostgreSQL 读取 running execution、workflow snapshot 和 event history，取得 lease 后完成执行，并在终态释放 lease。

CI 的 `postgres` job 在 `postgres:18` service 上重复执行 migration、第二次应用和以上测试。

备份恢复 rehearsal：

```bash
export HPA_POSTGRES_TEST_URL='postgresql://localhost:5432/hpa_p13_test'
node scripts/check-postgres-backup.mjs
```

脚本拒绝在未带 `test`/`phase13`/`backup` 标记的数据库上演练，使用 `pg_dump` custom format 创建临时备份，恢复到同集群临时库，逐表比较行数、migration checksum 和 66 表 RLS 状态，最后删除临时库。

## 8. 备份与回滚

Phase 13 基线不提供自动 down migration。企业数据删除不可逆，自动回滚 DDL 会扩大故障面。发布前和每次 migration 前必须：

1. 使用 `pg_dump --format=custom --file=hpa-before-<version>.dump "$HPA_POSTGRES_URL"`。
2. 在独立数据库执行 restore rehearsal，并运行 RLS/核心查询 smoke。
3. migration 失败时立即停止新实例，保留失败事务和 `_sqlx_migrations` 证据。
4. 已提交但业务不兼容时，恢复备份到新数据库并通过连接切换回滚；不得直接编辑 checksum 或删除迁移记录。

只有从未接收业务数据的临时测试库允许直接删除重建。

## 9. 尚未完成

- Workflow/Execution/Event/Artifact/Trigger Repository、Worker Job、企业控制面、浏览器会话、OAuth AS 与 Agent 版本/会话/事件/审批/工具调用已有显式 PostgreSQL 路由并验证跨节点读取；Skill 等其余元数据仍使用 SQLite。
- SQLite→PostgreSQL 数据导入已有幂等实现；PostgreSQL→SQLite 控制缓存由周期任务重建；Skill 整库迁移与真实 HA 演练尚未实现。
- 对象存储、只读副本、在线 PITR、连接重试和多控制面部署属于后续 P13-02/P13-06。
- 本文件中的 PASS 只适用于 schema/RLS/migration 基线，不适用于完整 P13-01 或 Phase 13。
