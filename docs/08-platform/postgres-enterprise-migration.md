# 企业控制面的 PostgreSQL 迁移（Phase 13 / P13-01）

> 状态：**IN_PROGRESS**。本文件记录控制面切换后的权威边界、SQLite 缓存与剩余项。
> 相关契约：[PostgreSQL 控制面基线](postgres-control-plane.md)、[分布式运行时原语](postgres-distributed-runtime.md)。

## 1. 当前形态

企业控制面已经完成“镜像先行、读取切换、权威翻转”：

1. **镜像（历史）**：SQLite 权威，写穿到 PostgreSQL。该阶段代码已移除。
2. **读取切换（完成）**：配额、租户、成员、SCIM、浏览器会话、OAuth AS 在 PG 模式全部读 PostgreSQL。
3. **权威翻转（完成）**：Enterprise API 与 SCIM 的租户/配额/成员/Group 写入、OAuth AS 状态、浏览器会话与平台令牌均只写 PostgreSQL；Agent、Skill 与 MCP 控制面元数据在 PG 模式只写 PostgreSQL；SQLite 对应表降级为可重建缓存，周期任务从 PostgreSQL 重建。

剩余差异是真实企业 IdP/SCIM/Vault/HA 的外部验收；Agent 版本/会话/事件/审批/工具调用、Skill 包/版本/文件/运行/产物与 MCP PAT/审批/审计/服务器元数据已在 PG 模式切换为 PostgreSQL 权威。

## 2. 现状矩阵

| 数据 | SQLite | PostgreSQL | 说明 |
| --- | --- | --- | --- |
| Workflow / Execution / Trigger | 旁路 | **权威** | `HPA_POSTGRES_RUNTIME=1` 时不再写 SQLite |
| Worker 注册 / 心跳 / 停用 | 不写 | **权威** | `postgres_worker_registry` |
| Worker Job / 租约 | 不写 | **权威** | `postgres_worker_daemon` |
| 平台令牌 | 不写 | **权威** | `postgres_token_store` |
| 审计 | 不写 | **权威** | 未知租户回退 NULL tenant |
| 租户 | 缓存 | **权威** | list/get/create/status/delete 走 PostgreSQL；缓存可重建 |
| 配额 | 缓存 | **权威** | 读写都以 PostgreSQL 为准 |
| 成员 | 缓存 | **权威** | Enterprise/SCIM/browser_auth 读写 PostgreSQL |
| SCIM Group / 映射 | 不写 | **权威** | 角色重算在同一 PG 事务 |
| 浏览器会话 | 不写 | **权威** | 读/写/轮换/登出/按租户吊销全部走 PostgreSQL |
| OAuth AS 状态（clients/codes/tokens/consents/signing keys） | 不写 | **权威** | `postgres_oauth_cross_node` |
| Agent 会话 / 版本 / 事件 / 审批 / 工具调用 | 不写 | **权威** | `postgres_agent_cross_node`；会话 owner lease 见 `postgres_agent_lease_cross_node` |
| Skill 包 / 版本 / 文件 / 运行 / 产物 | 不写 | **权威** | `postgres_skill_cross_node`；签名文件仍为本地 |
| MCP PAT / 审批 / 审计 / 外部服务器 | 不写 | **权威** | `postgres_mcp_meta_cross_node`；持久 session/task 另由 `HPA_POSTGRES_MCP` 覆盖 |
| Connector 定义 / 连接 / 审计 | 不写 | **权威** | `postgres_connector_cross_node`；插件包与 Marketplace 元数据仍为本地 |
| Plugin 生命周期 / Marketplace | 不写 | **权威** | `postgres_plugin_cross_node`；包文件/签名私钥仍为节点本地 |

## 3. 权威写入与缓存

### 3.1 启动缓存重建 + 周期刷新

`hpa serve` 在 `HPA_POSTGRES_RUNTIME=1` 时执行一次 `sync_postgres_control()`：从 `hpa_control.tenants/quotas/members` 读取权威数据，重建本机 `enterprise_*` SQLite 缓存并删除 PostgreSQL 已不存在的行。它不再把 SQLite 写回 PostgreSQL。

启动之后同一函数由后台任务周期性重跑（默认 30 秒，`HPA_CONTROL_SYNC_INTERVAL_MS` 可调，最小 50ms）。刷新失败只记 `warn`，不影响请求路径；PostgreSQL 始终是唯一事实源。

### 3.2 权威写入

| 操作 | PG 模式行为 |
| --- | --- |
| `POST/PUT/DELETE /api/enterprise/tenants` | 只写 PostgreSQL；租户目录与缓存行由本地目录/周期重建维护 |
| `PUT /api/enterprise/quotas/{tenant}` | 先写 `hpa_control.quotas`，失败即请求失败；SQLite 缓存可稍后重建 |
| `POST/PUT/DELETE /api/enterprise/members` | 只写 PostgreSQL 成员表（含 SCIM 字段） |
| SCIM User/Group | 只写 PostgreSQL；Group 成员与 effective role 在同一事务 |
| OAuth AS | clients/requests/codes/tokens/consents/signing keys 只写 `hpa_control` |
| MCP 元数据 | PAT/审批/审计/外部服务器只写 `hpa_tenant.mcp_*`；审批决定与一次性消费在单条 UPDATE 内完成 |
| Connector 元数据 | 定义/连接/审计只写 `hpa_tenant.connector_*`；插件启停/卸载通过同一存储改定义状态 |
| Plugin 元数据 | 包/安装/能力/事件、上架/评分、发布者/来源/远程包只写 `hpa_tenant.plugin_*`/`marketplace_*`；插件事件同事务写 `marketplace_event_stream` |
| 管理员令牌 / 审计 / 浏览器会话 | 只写 PostgreSQL |

PostgreSQL 写入失败会直接返回错误；不会出现“本地成功、共享状态没变”。

### 3.3 读取

| 读取 | 行为 |
| --- | --- |
| 租户 list/get/status | PG 模式读 `hpa_control.tenants`；空节点立即可见其他节点创建的租户 |
| 配额 | 读 `hpa_control.quotas` |
| 成员 / SCIM Users/Groups | 读 `hpa_control.members/scim_groups/scim_group_members` |
| MCP PAT / 审批 / 审计 / 服务器 | 读 `hpa_tenant.mcp_tokens/mcp_approvals/mcp_audit/mcp_servers`；PAT 认证按 token hash 查询并触碰 `last_used_at` |
| Connector 定义 / 连接 / 审计 | 读 `hpa_tenant.connector_definitions/connections/connector_audit`；执行引擎按定义 manifest 在本节点运行 |
| Plugin / Marketplace | 读 `hpa_tenant.plugin_*`/`marketplace_*`；插件包文件与安装目录仍按本节点路径读取 |
| 浏览器登录成员覆盖 | `BrowserAuth` 读 PostgreSQL 成员 |
| 租户 active 检查 | 读 PostgreSQL 状态；仅升级窗口在 PG 无行时回退 SQLite 并自愈 |
| `active_tenants()` | PG 有租户行时以 `hpa_control.tenants` 为准，否则回退 SQLite |

`postgres_control_mirror` 已覆盖跨节点读取：空 data dir 的节点能为它从未见过的租户注册 Worker（租户解析来自 PostgreSQL），未知租户仍 404。

## 4. 完成状态

1. ~~租户读取切 PG~~（完成）。
2. ~~配额权威翻转~~（完成）。
3. ~~成员与 SCIM~~（完成）：Enterprise 成员 API、SCIM User/Group、Group 角色映射与 `browser_auth::find_member` 在 PG 模式全部读写 PostgreSQL；SQLite 成员/组不再由 API 写入。
4. ~~浏览器会话与 OAuth AS~~（完成）：session 与 OAuth clients/codes/tokens/consents/signing keys 全部走 PostgreSQL。
5. ~~停止双写~~（完成）：API 不再写 SQLite 权威行；`sync_postgres_control` 只做 PG→SQLite 缓存重建，不再反向覆盖 PostgreSQL。

剩余：真实企业 SCIM/IdP、HA/read replica 与外部 Vault/CA 仍未验收。

### 3.4 会话（已完成）

`BrowserAuth` 在 PG 模式下把会话读写全部指向 `hpa_control.sessions`：

| 操作 | PG 行为 |
| --- | --- |
| 登录写入 | `create_session` |
| 每次请求 | 按 token_hash 查询 + `last_seen_at` 触碰 |
| 静默续期 | `rotate_session_tokens`：`WHERE token_hash = ? AND refresh_token_version = ?` 保留原围栏，命中 0 行即视为重放并吊销 |
| 登出 | `revoke_session_by_hash` |
| 刷新令牌过期/重放 | `revoke_session_by_id` |
| 租户暂停 | `revoke_tenant_sessions` |

`postgres_session_store` 证明：节点 A 创建的会话，节点 B（空 data dir）用同一 Cookie 通过认证并拿到 `platformAdmin`；节点 B 登出后该行在 PostgreSQL 标记 `revoked_at`，两节点都拒绝；任一节点的 SQLite `enterprise_sessions` 保持 0 行。

### 3.5 配额写入（已完成）

`PUT /api/enterprise/quotas/{tenant}` 在 PG 模式下：

1. 先 `upsert_quota` 到 `hpa_control.quotas`，失败直接返回 500——不再“本地成功、共享状态没变”。
2. 再把同一份值 upsert 到 `enterprise_quotas` 作为单节点缓存，失败只记日志（该节点可能从未见过这个租户）。

`postgres_control_mirror` 覆盖：空 data dir 的节点为未见过的租户写配额返回 200，另一节点与 PostgreSQL 都读到新值。

## 4.1 双节点验收

`node scripts/multi-node-e2e.mjs`（需要指向空库的 `HPA_POSTGRES_URL`）会启动两个共享同一 PostgreSQL 的控制面节点，逐项断言：

| 断言 | 说明 |
| --- | --- |
| `shared-admin-token` | `enterprise init` 写入的令牌在两个节点都可用 |
| `shared-quota` | 节点 A 设置配额，节点 B 读到同一值 |
| `execution-visible-on-b` | 节点 A 发起的工作流在节点 B 查询为 `succeeded` |
| `worker-executes-across-nodes` | Worker 注册在 A，daemon 连 B 领取并完成 |
| `shared-revocation` | A 吊销令牌后 B 立即 401 |
| `late-node-joins` | 后启动的第三个节点直接可用 |

### 4.2 缓存重建

`HPA_CONTROL_SYNC_INTERVAL_MS`（默认 `30000`）控制 PG→SQLite 缓存重建间隔。测试 `postgres_control_refresh` 用 100ms 间隔启动节点，通过 `/scim/v2/Users` 创建成员（直接写 PostgreSQL），断言该成员无需重启即出现在 `hpa_control.members`，并在缓存中收敛。

## 4.2 双节点与滚动重启

| 脚本 | 覆盖 |
| --- | --- |
| `pnpm test:multi-node` | 两节点共享库：管理员令牌、租户、配额、工作流执行、Worker 跨节点执行、令牌吊销、后加入节点 |
| `pnpm test:rolling` | 两轮「杀掉持有待审批执行的节点 → 用空 data dir 起替代节点 → 在替代节点批准并跑完」；断言执行不丢、列表里只有一条、幸存节点状态不偏斜 |

`test:rolling` 是**同版本滚动重启**的演练：它证明 PostgreSQL 中的执行状态、审批与租约能在节点被杀后由新节点接管。**跨版本兼容（旧二进制读新 schema）不在其中**，仍需人工按发布清单演练。

## 4.3 漂移检查

```bash
HPA_POSTGRES_URL=postgresql://user@host:5432/db \
  hpa --data-dir <data-dir> db verify-mirror
```

比较 PostgreSQL 权威与 SQLite 缓存，输出 `counts` 与 `drift.items`：

| 漂移类型 | 含义 |
| --- | --- |
| `tenant.missingInPostgres` / `tenant.statusMismatch` | PostgreSQL 无租户，或状态不一致 |
| `quota.missingInPostgres` / `quota.mismatch` | PostgreSQL 配额缺失或字段不一致 |
| `member.missingInPostgres` / `member.attrMismatch` | PostgreSQL 成员缺失，或 `role`/`status` 不一致 |
| `*.extraInPostgres` | PostgreSQL 有权威行、SQLite 缓存尚未重建 |

存在漂移时命令以非零退出码结束，便于放进发布门禁。`postgres_mirror_verify` 覆盖：缓存收敛后为零、直接改 SQLite 后报 `member.attrMismatch`、PG→SQLite 缓存重建后回到零。

**用途**：这是第 5 步「停止双写」的前置条件——没有零漂移就不该关掉 SQLite 写入。

## 4.4 多节点共享前提（部署契约）

切换到 PostgreSQL 后，**节点本地仍保留三类文件**。部署时必须按住户/插件量规划：

| 本地路径 | 内容 | 多节点要求 |
| --- | --- | --- |
| `data_dir/plugins/installed/<id>/` | 插件包文件、解包后的handler与资源 | **必须共享**（NFS/对象存储挂载/同一 PV），否则「A 节点安装、B 节点加载失败」 |
| `data_dir/tenants/<id>/` | 租户 SQLite 缓存与附件 | 可不共享：PG 模式下是可重建缓存，缺失会按需重建 |
| `data_dir/keys/` 等 | 本地密钥与配置 | 按需：OAuth 签名密钥与会话密钥在 PG/环境变量中，本地文件仅作引导 |

`hpa doctor` 会把这件事显式化：在 PostgreSQL 模式下它比较 `hpa_tenant.plugins` 的安装数与本地 `plugins/installed` 目录数，缺包时输出

```
"plugins": { "installedInPostgres": 3, "packagesPresentLocally": 1, "packagesMissing": 2 },
"warnings": ["2 plugin(s) are installed in PostgreSQL but their packages are missing under
             /data/hpa/plugins/installed; mount the plugins directory on a shared volume ..."]
```

`status` 不会因此变成 `degraded`（缺包只影响用到它的节点），但工单里应该把它当作部署错误处理。

## 5. 验证

```bash
HPA_POSTGRES_TEST_URL=postgresql://user@host:5432/db \
  cargo test -p hpa-local --test postgres_control_mirror
HPA_POSTGRES_TEST_URL=... cargo test -p hpa-local --test postgres_oauth_cross_node
HPA_POSTGRES_TEST_URL=... cargo test -p hpa-local --test postgres_scim_cross_node
HPA_POSTGRES_TEST_URL=... cargo test -p hpa-local --test postgres_member_authority
HPA_POSTGRES_TEST_URL=... cargo test -p hpa-local --test postgres_mirror_verify
HPA_POSTGRES_TEST_URL=... cargo test -p hpa-local --test postgres_agent_cross_node
HPA_POSTGRES_TEST_URL=... cargo test -p hpa-local --test postgres_agent_lease_cross_node
HPA_POSTGRES_TEST_URL=... cargo test -p hpa-local --test postgres_skill_cross_node
HPA_POSTGRES_TEST_URL=... cargo test -p hpa-local --test postgres_mcp_meta_cross_node
HPA_POSTGRES_TEST_URL=... cargo test -p hpa-local --test postgres_connector_cross_node
HPA_POSTGRES_TEST_URL=... cargo test -p hpa-local --test postgres_plugin_cross_node
node scripts/scim-e2e.mjs                                  # 需要运行中的服务与管理员令牌
```

- `postgres_oauth_cross_node`：A 注册/授权/同意，B 兑换、JWKS、refresh 轮换、复用吊销、revoke/内省；两节点 SQLite OAuth 表 0 行。
- `postgres_scim_cross_node`：A 创建 User/Group，B 读取角色映射与成员；SQLite SCIM 表 0 行。
- `postgres_member_authority`：A 建租户/成员，B 立即可见；目录角色覆盖 session；disabled 成员与 suspended 租户使 B 的 session 失效；SQLite 无权威行。
- `postgres_agent_cross_node`：A 建/发布 Agent，B 读取版本；B 恢复 interrupted 会话为 succeeded、取消 running 会话；审批与工具调用跨节点可见，重复 begin 幂等；两节点 SQLite Agent 表 0 行。
- `postgres_agent_lease_cross_node`：A 持有 running 会话租约，B 启动恢复不中断；伪造 token 写入失败；过期后 B 接管并递增 token，A 旧 token 失败；C 启动接管空闲会话并写 `agent.interrupted`；终态拒绝迟到写入。
- `postgres_skill_cross_node`：A 导入 Skill，B 读取版本与包文件；A 发布后 B 看到 published；B 运行 Skill 后 A 读取 run 与 `summary.json` artifact；SQLite Skill 表 0 行；Skill 配额按 PG 计数。
- `postgres_mcp_meta_cross_node`：A 创建 MCP PAT，B 列表可见并用它通过 `/mcp` 认证，A 读到 B 写入的审计；B 吊销后 A 立即 401；审批决定/一次性消费/过期、服务器 upsert/list/delete 与 stats 跨节点一致；SQLite MCP 表 0 行。
- `postgres_connector_cross_node`：A/B 共享内置 Connector 定义；A 建样例连接，B 读取、更新、测试，A 读到更新的连接与 B 写入的审计；A 删除后 B 列表消失；定义幂等/冻结/所有权与 SQLite 0 行均有断言。
- `postgres_plugin_cross_node`：A 安装样例插件，B 读取插件/包/能力/贡献的 Connector；B 停用、A 启用；A 发布 Marketplace，B 审核/评分/撤回；发布者与来源/key 跨节点可见，撤销跨节点生效；远程包与插件 MCP server 存储级验证；A 卸载后 B 读到 uninstalled；SQLite 插件表 0 行。
- `postgres_control_mirror`：跨节点配额与租户读取、空 data dir 节点为未见租户注册 Worker。

## 6. 风险

- SQLite 缓存有最多一个刷新间隔的滞后；PG 模式的 API 读取已全部走 PostgreSQL，只有历史/诊断路径读缓存。
- `hpa_control.members` 有 `(tenant_id, subject)`、email、user_name、external_id 唯一约束；历史重复数据在首次写入/导入时会失败关闭，需先清理。
- SCIM Group 的 SQLite 表在 PG 模式不再写入；依赖 SQLite 直读的自定义脚本应改用 SCIM/PostgreSQL。
- 删除租户只标记 `deleted` 并吊销 PG token/session；历史 SQLite 行由缓存重建清理，租户业务库目录由删除流程移除。
- Agent 版本/会话/事件/审批/工具调用在 PG 模式走 `hpa_tenant.agent_*`，跨节点发布、恢复、取消、审批与工具幂等由 `postgres_agent_cross_node` 验证；Agent 配额也按 PostgreSQL 行计数。会话 owner lease（`lease_owner/lease_token/lease_expires_at`）保证同一 running 会话单一写者，`recover` 跳过 live 租约、只接管过期会话；`HPA_NODE_ID` 必须稳定且唯一，TTL 由 `HPA_AGENT_LEASE_TTL_MS` 控制（默认 30s，最小 200ms）。Skill 包/版本/文件/运行/产物元数据由 Phase 14 切到 `hpa_tenant.skill_*`，签名文件与 artifact 内容仍在节点本地；`hpa agent/skill ...` CLI 仍只操作本地 SQLite，PG 模式应使用 HTTP API。
- MCP PAT/审批/审计/服务器元数据由 Phase 15 切到 `hpa_tenant.mcp_*`；`credential_env` 只保存变量名，服务器进程与凭证值仍属节点本地。持久 session/subscription/task 仍由 `HPA_POSTGRES_MCP=1` 的 `hpa_control` 表覆盖，两者不要混用为同一迁移项。`hpa mcp token ...` CLI 仍只操作本地 SQLite，PG 模式应使用 HTTP API。
- Connector 定义/连接/审计由 Phase 16 切到 `hpa_tenant.connector_*`；插件包、签名、发布者/来源/远程 Marketplace 索引与插件自身表由 Phase 17 切到 `hpa_tenant.plugin_*`/`marketplace_*`。插件包文件、签名私钥与安装目录仍在节点本地；跨节点启用/回滚需要目标节点具备文件或共享存储。凭证环境变量只在连接行保存名称，值与执行仍属节点本地。
