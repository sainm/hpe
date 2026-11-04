# Phase 22：Agent 工具副作用租约守卫

> 状态：**本地验收完成**（2026-11-04）。Phase 18 的租约围栏覆盖会话与事件写入；Phase 22 在计划步骤和工具执行前续租并校验，避免已被接管的旧 holder 继续触发工作流、Connector、MCP 或 Skill 副作用。

## 1. 范围

- `AgentRuntime::ensure_session_lease`：读取本进程持有的 token，调用 `renew_agent_lease` 续期；失败立即移除本地映射并返回 `AGENT_LEASE`。
- 调用点：
  - 确定性/Python 驱动循环的每一轮开始。
  - `execute_tool` 前（含 connector、skill、workflow、MCP 工具）。
  - Python 工具服务处理工具调用前。
  - 审批恢复执行工具前。
  - Python planner 启动与 Python 审批恢复前。
- 本地 SQLite 模式无租约，守卫为 no-op；行为不变。

## 2. 行为契约

- 租约有效时续期并继续；每次续期延长 `lease_expires_at`。
- 租约已过期、被其他 owner 接管或 token 不匹配时，工具不执行，返回 `AGENT_LEASE`/失败工具响应。
- 守卫只保护新的副作用；已开始的单个工具调用不会中途被打断。
- `recover` 与终态围栏语义保持 Phase 18 的规则。
- 工具调用账本仍按原 idempotency key 去重；守卫失败不会写入成功账本。

## 3. PostgreSQL schema

无新增 migration；复用 `agent_sessions.lease_owner/lease_token/lease_expires_at`。

## 4. 验收

```bash
HPA_POSTGRES_TEST_URL=postgresql://user@host:5432/db \
  cargo test -p hpa-local --test postgres_agent_lease_cross_node
```

`postgres_agent_lease_cross_node` 新增覆盖：

- holder 续期成功，其他 owner 续期失败。
- 过期接管后旧 token 续期失败，新 token 续期成功。
- 节点 C 通过 API 运行一个包含 `workflow.list` 工具调用的 Agent：每轮与工具前守卫续期成功，运行完成并释放租约。

门禁：全量 Rust 247 passed / 1 ignored；真实 PostgreSQL 18.4 上 31 个 PG 集成测试文件（32 用例）通过；PostgreSQL 备份演练恢复 21 个 migration、71 个业务关系与 68 张 FORCE RLS 表一致；Web typecheck、12 Vitest 与 build 通过；扩展 65 Vitest、7 个 Extension Host 检查通过；品牌/IR/契约/文档（179 文件 871 链接）/镜像/OTel/供应链门禁通过。

## 5. 非目标

- 不中断已经开始的单个外部工具调用；只阻止新的副作用。
- 不为每个工具调用引入独立租约或分布式锁。
- 不改变 Agent 计划器、预算、审批或工具账本语义。
