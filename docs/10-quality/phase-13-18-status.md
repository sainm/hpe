# Phase 13–18 状态台账

> 目的：把「文档声称」「代码事实」「本机可复现证据」放在同一张表里。触发这份表的是「测试计数照抄旧值」——阶段文档里的 23x passed 是当时的快照，既没写跳过项也没写沙箱限制。表里的状态一律以本机实测为准。
>
> 最近核对于 **2026-11-03 22:15**，代码 `53d34a4`，PostgreSQL 18.4，macOS。

## 0. 一句话结论

- 控制面元数据在 `HPA_POSTGRES_RUNTIME=1` 下**基本全部**以 PostgreSQL 为权威；SQLite 退化为单节点模式或可重建缓存。
- 本机可复现的自动化证据：
  - 默认（沙箱按平台策略启用）：Rust 全量 **225 passed / 14 failed / 1 ignored**，PostgreSQL 隔离 **28 / 29**。
  - 设置 `HPA_SKILL_ALLOW_UNSANDBOXED=1`（受限环境专用）：**235 passed / 4 failed / 1 ignored**，PostgreSQL 隔离 **29 / 29**；4 个失败是 `runner.rs` 里**要求沙箱生效**的用例，在降级模式下按设计失败。
- 仍需外部环境的只有四类：真实 AI 客户端、企业 IdP/CA/Vault、生产 Collector/HA、**跨版本**多节点滚动升级（同版本重启已由 `pnpm test:rolling` 覆盖）。

## 1. 逐阶段台账

| 阶段 | 能力 | 代码位置 | 自动化证据 | 本机状态 |
| --- | --- | --- | --- | --- |
| P13-01 | 租户/配额/成员/SCIM 权威；令牌与审计权威；浏览器会话；OAuth AS | `enterprise.rs`、`scim.rs`、`browser_auth.rs`、`oauth.rs`、`postgres_control.rs` | `postgres_member_authority`、`postgres_scim_cross_node`、`postgres_token_store`、`postgres_session_store`、`postgres_oauth_cross_node`、`postgres_control_mirror`、`postgres_mirror_verify`、`postgres_control_refresh` | 全绿 |
| P13-02 | outbox、调度器 leader 选举、执行租约与恢复、Worker 租约 | `distributed_runtime.rs`、`postgres_execution.rs`、`postgres_worker.rs` | `postgres_distributed`、`postgres_event_bus`、`postgres_scheduler_leader`、`postgres_execution_lease`、`postgres_execution_recovery`、`postgres_atomic_execution_fire`、`postgres_runtime`、`postgres_worker_daemon`、`postgres_worker_registry` | 全绿 |
| P13-03 | 持久 MCP session/subscription/task | `persistent_mcp.rs` | `postgres_mcp`、`mcp_multinode` | 全绿 |
| P13-04 | 可选 Python Agent Worker、LiteLLM 网关 | `services/ai-worker`、`ai_worker_client.rs` | `ai_worker_process`（需 `HPA_AI_WORKER_BIN`）、Python 17 passed / 2 skipped + ruff + mypy | 设置 `HPA_AI_WORKER_BIN=services/ai-worker/.venv/bin/hpa-ai-worker` 后实跑通过（2.70s）；未设置时静默跳过。真实云模型 `BLOCKED_EXTERNAL` |
| P13-05 | Connector 扩展、远程 Marketplace、签名与撤回 | `connector.rs`、`plugin.rs`、`postgres_connector.rs`、`postgres_plugin.rs` | `connectors_extended`、`marketplace_events`、`marketplace_remote`、`postgres_connector_cross_node`、`postgres_plugin_cross_node` | 全绿（keyless Fulcio/Rekor `BLOCKED_EXTERNAL`） |
| P13-06 | 企业认证、SCIM、租户暂停、遗留 SSO | `enterprise.rs`、`browser_auth.rs`、`oidc_verify.rs` | `enterprise`、`enterprise_ops`、`browser_sso`、`postgres_scim_cross_node` | 全绿（真实企业 IdP/CA/Vault `BLOCKED_EXTERNAL`） |
| P13-07 | OTLP traces/metrics/logs、跨进程传播 | `telemetry.rs`、`services/ai-worker` | `otel`（2 用例）、`otel_metrics_logs`、`otel_cross_process`（需 `HPA_AI_WORKER_BIN`）、`check:otel` | `otel`/`otel_metrics_logs` 无需变量实跑通过；`otel_cross_process` 设置变量后 0.86s 实跑通过。生产 Collector/HA `BLOCKED_EXTERNAL` |
| P13-08 | 发布产物、checksums、回滚手册 | `scripts/release-pack.mjs`、`scripts/release-verify.mjs`、`docs/10-quality/release-checklist.md` | `release:pack` + `release:verify`（`hpa doctor` 冒烟） | 打包链路完成；真实客户端验收 `BLOCKED_EXTERNAL` |
| P14 | Skill 包/版本/文件/运行元数据权威 | `postgres_skill.rs` | `postgres_skill_cross_node` | 默认失败（技能测试执行 `scripts/audit.py`，本机禁止 `sandbox-exec`）；设置 `HPA_SKILL_ALLOW_UNSANDBOXED=1` 后**通过**（运行结果记为 `sandbox=process`） |
| P15 | MCP 控制面（PAT/审批/审计/外部服务器） | `postgres_mcp_meta.rs` | `postgres_mcp_meta_cross_node` | 全绿 |
| P16 | Connector 定义/连接/审计 | `postgres_connector.rs` | `postgres_connector_cross_node` | 全绿 |
| P17 | Plugin 生命周期/Marketplace/信任元数据 | `postgres_plugin.rs` | `postgres_plugin_cross_node` | 全绿（包文件与签名私钥仍节点本地） |
| P18 | Agent 会话 owner lease（migration `0020`） | `agent.rs`、`postgres_agent.rs` | `postgres_agent_cross_node`、`postgres_agent_lease_cross_node` | 全绿 |

## 2. 本机失败清单

### 2.1 默认模式（沙箱启用）

| 位置 | 数量 | 原因 |
| --- | --- | --- |
| `crates/hpa-skill/tests/runner.rs` | 8 | `sandbox-exec: sandbox_apply: Operation not permitted`（本 shell 不允许嵌套沙箱） |
| `hpa-local` bin 单测（mcp ×2、skills ×1） | 3 | 同源：脚本执行经过 seatbelt |
| `tests/agents.rs` | 1 | 同源 |
| `tests/mcp_http.rs`、`tests/mcp_stdio.rs` | 各 1 | 同源 |
| `tests/postgres_skill_cross_node.rs` | 1 | 同源：技能测试执行 `scripts/audit.py` |

### 2.2 降级模式（`HPA_SKILL_ALLOW_UNSANDBOXED=1`）

| 位置 | 数量 | 原因 |
| --- | --- | --- |
| `crates/hpa-skill/tests/runner.rs` | 4 | `seatbelt_denies_*` 与 `deterministic_script_runs_in_sandbox_*` **要求沙箱生效**，降级后按设计失败 |

其余 10 个用例在该模式下转为通过（含 `postgres_skill_cross_node`、bin 的 mcp/skills、`agents`、`mcp_http`、`mcp_stdio`）。

**解释**：两组数字不能互相替代。默认模式证明「隔离策略按平台生效」；降级模式证明「功能链路在受限环境可跑」。发布证据应以默认模式为准，降级模式只用于补齐功能覆盖。

### 2.3 已知偶发

`tests/plugins.rs` 在**整个 workspace 并行**运行时偶发失败（约 5 次中出现 1 次），`cargo test -p hpa-local --tests` 连跑 2 次与单独运行 12/12 均通过（并发与单线程都过）。

- 已排除：固定端口、`~/.hpa` 共享目录、测试内 `sleep`/短超时（该文件不 sleep、用 UUID 临时目录、端口 0）。
- 未定位：怀疑是 workspace 级别多进程同时启动 `hpa` CLI 时的资源竞争（进程数/文件句柄），本机无法稳定复现。
- 处理：出现时以 `cargo test -p hpa-local --test plugins` 复跑为准；连续两次全量复现后再改代码，避免为不可复现的现象引入猜测性改动。

## 2.1 会「静默跳过」的测试

这些用例在缺少环境变量时打印一行 `skipped:` 并**返回成功**，报告里看起来是绿的。判断方法：看耗时是否 `0.00s`，或直接看输出里有没有 `skipped:`。

| 变量 | 影响的套件 | 未设置时的行为 |
| --- | --- | --- |
| `HPA_POSTGRES_TEST_URL` | 全部 `postgres_*.rs` | 打印 `skipped: HPA_POSTGRES_TEST_URL is not set` 并返回成功 |
| `HPA_AI_WORKER_BIN` | `ai_worker_process`、`otel_cross_process` | 0.00s 返回成功，实际未执行 |
| `HPA_S3_TEST_ENDPOINT` 等 | `connectors_extended` 的 S3 用例 | 跳过 S3 往返 |
| `HPA_MCP_CONFORMANCE` | `mcp_conformance`（脚本侧） | 不注册 conformance 工具 |
| `HPA_OSV_ECOSYSTEM` | 在线漏洞查询 | 退回离线库或无网络结论 |

发布证据必须来自**设置了这些变量之后的运行**；这张表里所有“全绿”都按此口径记录。

### 2.3.1 一次误判记录：SCIM 到底写没写 PostgreSQL

我曾根据 `grep -c 'hpa_control\.' crates/hpa-local/src/scim.rs` 返回 0，判断「SCIM 只写 SQLite、文档虚报」。**这个判断是错的**：

- SCIM 的路由与校验在 `scim.rs`，但 SQL 在 `postgres_control.rs` 的 `scim_*` 方法里（`scim_create_member`、`scim_groups` 等），所以单文件 grep 命中 0。
- 实测（2026-11-03，PostgreSQL 模式）：`POST /scim/v2/Users` → 201，**同一秒**查 `hpa_control.members` 已有 1 行；`POST /scim/v2/Groups`（带成员与角色映射）→ 201，`scim_groups`=1、`scim_group_members`=1，随后读该用户 `effectiveRole` 立即为 `auditor`。没有任何镜像延迟。

教训：验证「某能力是否走 PG」不能只 grep 单个文件，要看该能力的 store 层（`postgres_*.rs` / `postgres_control.rs` 里的同名方法）。这条也写进第 4 节的纪律里。

### 2.4 关于各阶段文档里的「门禁」数字

`phase-14` … `phase-18` 每篇结尾都有一段「全量 Rust 23x passed / 1 ignored」的门禁记录。那是**该阶段完成当时的快照**，且只统计了通过数、没写同一环境下被跳过的用例和沙箱限制。以本表为准：

- 默认模式：225 passed / 14 failed / 1 ignored（14 个为沙箱限制）
- 降级模式：235 passed / 4 failed / 1 ignored
- PostgreSQL：默认 28/29，降级 29/29

另外「66 张 FORCE RLS 表」指的是 **40 张 `hpa_tenant` + 26 张 `hpa_control`**，不是 66 张租户表；`hpa doctor` 现在按 schema 分开报告。

## 2.5 滚动重启演练

`pnpm test:rolling`（需要空库的 `HPA_POSTGRES_URL`）跑两轮：起两个节点 → 发起带审批的执行 → 等待 `waiting_approval` → `SIGKILL` 持有它的节点 → 用**空 data dir** 起替代节点 → 在替代节点批准 → 执行 `succeeded`，且幸存节点看到同一状态、执行列表里只有一条记录。17 项断言全过。

**边界**：这是同版本滚动重启。跨版本（旧二进制对新 schema）没有自动化，发布清单里仍是人工项；迁移是前向 only，回滚需要备份恢复。

## 3. 如何复现本表

```bash
# 1. Rust 全量（无 PG 时 PostgreSQL 套件会打印 skipped 并返回成功）
cargo test --workspace --no-fail-fast

# 2. PostgreSQL：每个测试文件独立库
HPA_POSTGRES_ADMIN_URL=postgresql://user@host:5432/postgres pnpm test:postgres
#    无法嵌套沙箱的环境（本机 / 受限 CI）加 HPA_SKILL_ALLOW_UNSANDBOXED=1 让技能用例跑起来，
#    此时 29/29 全通过；真机上不要设置它，隔离性由 runner.rs 覆盖

# 3. 双节点验收（共享一个库）
HPA_POSTGRES_URL=postgresql://user@host:5432/empty_db pnpm test:multi-node

# 4. 发布产物
pnpm release:pack && pnpm release:verify -- dist-release/hpa-*.tar.gz

# 5. 仓库门禁
pnpm check
```

## 4. 文档纪律（写给自己和下一位维护者）

1. **声称某模块已切 PostgreSQL 之前**，grep **整个 crate**（`grep -rn 'hpa_control\.\|scim_' crates/hpa-local/src`），并确认它调用的 store 方法落在 `postgres_*.rs`。只看单个 handler 文件会误判（见 2.3.1）。
2. **测试计数只写实测值**，并写清环境（沙箱/真实 PG/CI）。
3. **区分“本地模式仍可用”与“PG 模式已验证”**：两者都要有对应测试，缺哪个就在本表标出来。
4. **`BLOCKED_EXTERNAL` 要写清缺什么**（凭证？主机？IdP 租户？），不要用“待后续”含糊过去。
