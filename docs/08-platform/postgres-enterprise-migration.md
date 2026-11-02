# 企业控制面的 PostgreSQL 迁移（Phase 13 / P13-01）

> 状态：**IN_PROGRESS**。本文件记录“整库切换”的现状、镜像设计与剩余步骤。
> 相关契约：[PostgreSQL 控制面基线](postgres-control-plane.md)、[分布式运行时原语](postgres-distributed-runtime.md)。

## 1. 为什么不是一次切换

企业控制面在 SQLite 时代由三张互相引用的表构成：`enterprise_tenants` → `enterprise_quotas` / `enterprise_members`，成员又被 SCIM 的 Group 映射与浏览器会话引用。把它们同时切走意味着一次改动 80 余处查询、并让 SCIM/SSO 两条链路同时失去回滚点。

因此采用**镜像先行、读取逐条切换、最后翻转权威**的三步走：

1. **镜像（已完成）**：SQLite 仍是权威，但每一次租户/配额/成员变更都同步写入 PostgreSQL。
2. **读取切换（部分完成）**：配额读取已经在 PostgreSQL 模式下走 PostgreSQL。
3. **权威翻转（未开始）**：SQLite 停止写入，PostgreSQL 成为唯一事实源，再删除镜像代码。

## 2. 现状矩阵

| 数据 | SQLite | PostgreSQL | 说明 |
| --- | --- | --- | --- |
| Workflow / Execution / Trigger | 旁路 | **权威** | `HPA_POSTGRES_RUNTIME=1` 时不再写 SQLite |
| Worker 注册 / 心跳 / 停用 | 不写 | **权威** | `postgres_worker_registry` |
| Worker Job / 租约 | 不写 | **权威** | `postgres_worker_daemon` |
| 平台令牌 | 不写 | **权威** | `postgres_token_store` |
| 审计 | 不写 | **权威** | 未知租户回退 NULL tenant |
| 租户 | **权威** | 镜像 + **读取** | 存在性/状态解析已在 PG 模式走 PostgreSQL，未命中回退并自愈 |
| 配额 | **权威** | 镜像 + **读取** | 读路径已在 PG 模式下走 PostgreSQL |
| 成员 | **权威** | 镜像（写穿） | 创建/更新/删除同步到 PG |
| SCIM Group / 映射 | **权威** | 未镜像 | 依赖成员表，翻转时一起处理 |
| 浏览器会话 / OAuth AS 状态 | **权威** | 未镜像 | 含加密 token 字段，需单独迁移 |

## 3. 镜像实现

### 3.1 启动镜像

`hpa serve` 在 `HPA_POSTGRES_RUNTIME=1` 时执行一次 `sync_postgres_control()`：把 SQLite 的租户、配额、成员全量 upsert 进 `hpa_control.tenants/quotas/members`。它是幂等的，失败会让启动失败（避免“以为在跑 PostgreSQL，实际只有半套数据”）。

### 3.2 写穿

| 操作 | 镜像动作 |
| --- | --- |
| `POST /api/enterprise/tenants` | 写 PG 租户 + 默认配额（`upsert_tenant` / `upsert_quota`） |
| `PUT /api/enterprise/quotas/{tenant}` | `upsert_quota` |
| `POST /api/enterprise/members` | `upsert_member` |
| `PUT /api/enterprise/members/{id}` | `upsert_member` |
| `DELETE /api/enterprise/members/{id}` | `delete_member` |

镜像失败只记 `warn`，不阻断 SQLite 写入；这是“SQLite 仍为权威”阶段的刻意取舍——翻转权威后同样的错误必须变成硬失败。

### 3.3 读取切换

| 读取 | 行为 |
| --- | --- |
| `quota()` | PG 模式先读 `hpa_control.quotas`，未命中回退 SQLite |
| `tenant_is_active()` | PG 模式先读 `hpa_control.tenants`；PG 无行时回退 SQLite 并**自愈**（顺手 `ensure_postgres_tenant`） |
| `active_tenants()` | PG 有租户行时以 `hpa_control.tenants` 为准，否则回退 SQLite（调度器与 `/metrics` 用） |
| `ensure_postgres_tenant()` | PG 已有该租户即跳过，避免“没见过的租户”在镜像阶段返回 404 |

回退路径在权威翻转时必须删除。`postgres_control_mirror` 已覆盖跨节点读取：空 data dir 的节点能为它从未见过的租户注册 Worker（租户解析来自 PostgreSQL），未知租户仍 404。

## 4. 剩余步骤

按依赖顺序，每一步都应独立提交并带 PG 集成测试：

1. ~~**租户读取切 PG**~~（已完成）：`ensure_tenant` / `active_tenants` 的存在性与状态判断改读 `hpa_control.tenants`；写入仍走 SQLite + 镜像。
2. **配额权威翻转**：删除回退路径，`PUT /quotas` 先写 PG，镜像反向写 SQLite 或直接停写。
3. **成员与 SCIM**：`scim.rs` 的 12 处成员查询、Group 映射、`browser_auth::find_member` 一起切；SCIM E2E 必须在 PG 模式下重跑（`pnpm test:scim-e2e`）。
4. **会话与 OAuth AS 状态**：`hpa_control.sessions` / `oauth_*` 已建表，需要把加密 token 字段、refresh 轮换与 CSRF 校验一并迁移；验收标准是“节点 A 登录、节点 B 直接用同一 Cookie 通过”。
5. **停止双写**：删除 SQLite 控制面写入与 `sync_postgres_control`，`enterprise_*` 表只作为历史备份保留。

## 5. 验证

```bash
HPA_POSTGRES_TEST_URL=postgresql://user@host:5432/db \
  cargo test -p hpa-local --test postgres_control_mirror   # 本文件所述镜像与配额读取
HPA_POSTGRES_TEST_URL=... cargo test -p hpa-local --test postgres_worker_registry --test postgres_token_store
node scripts/scim-e2e.mjs                                  # 需要运行中的服务与管理员令牌
```

`postgres_control_mirror` 覆盖：启动镜像出本地租户与配额、API 建租户写 PG、配额更新与成员创建写穿、以及**删掉整个 data dir 后用空目录启动的节点仍能从 PostgreSQL 读到同一配额**。

## 6. 风险

- 双写窗口内 SQLite 与 PG 可能不一致，且目前只有启动镜像做兜底对账；翻转前应加一个“差异检查”命令（比对三张表的行数与关键字段）。
- 成员镜像只覆盖 `enterprise.rs` 的四条写路径；SCIM 直接写 `enterprise_members`，其变更要到下次启动才会出现在 PG 里——这是当前的已知偏差。
- `hpa_control.members` 有 `(tenant_id, subject)` 唯一约束与 `user_name` / `external_id` 两个部分唯一索引；翻转时若历史 SQLite 数据存在重复，会在这里首次暴露。
