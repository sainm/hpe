# Phase 18：Agent 会话 owner lease

> 状态：**本地验收完成**（2026-11-03）。Phase 18 为 PostgreSQL 模式下的 Agent 主动执行加入会话级 owner lease，避免多个控制面在启动恢复时把仍在运行的会话标记为 interrupted。

## 1. 范围

- 新 migration `0020_agent_session_leases.sql` 给 `hpa_tenant.agent_sessions` 增加 `lease_owner`、`lease_token`、`lease_expires_at` 与过期索引；不新增表。
- `PostgresAgentStore` 增加 acquire/renew/release、fenced session update、fenced event append 与 lease 查询原语。
- `AgentRuntime` 在 `start`、`drive`、`resume_approval` 与 `recover` 时获取租约；`persist` 与 `emit` 在持有租约时使用 holder + token + 未过期的条件写。
- `recover` 只接管无租约或租约已过期的 running 会话；仍被其他节点持有的会话保持 running。
- 会话进入终态或等待审批后释放租约；进程崩溃后由 TTL 过期允许其他节点接管。

## 2. 行为契约

- 获取租约递增 `lease_token`；同一 owner 可重入续约，其他 owner 必须等待过期。
- fenced update/event 要求 `lease_owner`、`lease_token` 匹配且 `lease_expires_at > now()`；任一不满足即返回 `AGENT_LEASE`，不写入。
- 终态会话（succeeded/failed/budget_exhausted/timeout/cancelled）拒绝后续 fenced 写入，取消与执行完成不会互相覆盖。
- 每次 fenced 写入会延长租约；`HPA_AGENT_LEASE_TTL_MS` 控制 TTL（默认 30000，最小 200）。
- `HPA_NODE_ID` 在多节点部署中必须稳定且唯一；未设置时进程启动生成随机节点 id。
- `recover` 的接管写入使用新 token；旧 holder 的后续写会被拒绝。
- 本地 SQLite 模式不启用租约，行为不变。

## 3. PostgreSQL schema

```text
hpa_tenant.agent_sessions:
  lease_owner TEXT
  lease_token BIGINT NOT NULL DEFAULT 0
  lease_expires_at TIMESTAMPTZ
  INDEX agent_sessions_lease (tenant_id, lease_expires_at) WHERE lease_expires_at IS NOT NULL
```

表仍由 FORCE RLS 隔离；租约操作与其他 Agent 写入使用同一租户事务。

## 4. 验收

```bash
HPA_POSTGRES_TEST_URL=postgresql://user@host:5432/db \
  cargo test -p hpa-local --test postgres_agent_lease_cross_node
```

`postgres_agent_lease_cross_node` 覆盖：

- 节点 A 持有 running 会话租约后启动节点 B；B 的 `recover` 不中断该会话，lease owner/token 保持不变。
- 同 token 的 A 写入成功，B 的伪造写入失败；B 在租约有效期内无法获取。
- 短 TTL 过期后 B 接管，token 递增；A 的旧 token 写入失败，B 的新 token 写入成功。
- 释放后启动节点 C；C 接管过期/空闲会话并把状态改为 interrupted、写入 `agent.interrupted` 事件；A 的 live 会话仍保持 running。
- 节点 C 通过 HTTP API 创建/发布/运行一个无工具 Agent，运行完成后其会话租约已释放。
- 终态会话拒绝迟到的 fenced 写入。
- 两节点 SQLite Agent 表保持 0 行。

门禁：全量 Rust 239 passed / 1 ignored；真实 PostgreSQL 18.4 上 29 个 PG 集成测试文件（30 用例）通过；PostgreSQL 备份演练恢复 20 个 migration、69 个业务关系与 40 张 `hpa_tenant` + 26 张 `hpa_control` FORCE RLS 表一致；Web typecheck、11 Vitest 与 build 通过；扩展 63 Vitest、7 个 Extension Host 检查通过；品牌/IR/契约/文档（172 文件 856 链接）/镜像/OTel/供应链门禁通过。

## 5. 非目标

- 不实现控制面 leader 选举或全局调度；租约只保证同一会话单一写者。
- 不改变 Agent 计划器、工具、审批、预算或 checkpoint 语义。
- 不迁移 Agent 会话文件/本地 checkpoint；租约只覆盖 PostgreSQL 元数据。
- 不改变本地 SQLite 模式或 `hpa agent ...` CLI。
