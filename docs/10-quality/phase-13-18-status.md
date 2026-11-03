# Phase 13–18 状态台账

> 目的：把「文档声称」「代码事实」「本机可复现证据」放在同一张表里。之前出现过文档写“SCIM 已切 PostgreSQL”但 `scim.rs` 里零处 `hpa_control` 引用、以及测试计数照抄旧值的情况；这张表用来防止再次发生。
>
> 最近核对于 **2026-11-03 22:15**，代码 `53d34a4`，PostgreSQL 18.4，macOS。

## 0. 一句话结论

- 控制面元数据在 `HPA_POSTGRES_RUNTIME=1` 下**基本全部**以 PostgreSQL 为权威；SQLite 退化为单节点模式或可重建缓存。
- 本机可复现的自动化证据：Rust 全量 **225 passed / 14 failed / 1 ignored**（14 个全部是沙箱限制），PostgreSQL 文件级隔离 **28 / 29** 通过。
- 仍需外部环境的只有四类：真实 AI 客户端、企业 IdP/CA/Vault、生产 Collector/HA、多节点滚动升级。

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

## 2. 本机失败清单（全部为环境限制）

| 位置 | 数量 | 原因 |
| --- | --- | --- |
| `crates/hpa-skill/tests/runner.rs` | 8 | `sandbox-exec: sandbox_apply: Operation not permitted` |
| `hpa-local` bin 单测（mcp ×2、skills ×1） | 3 | 同源：脚本执行经过 seatbelt |
| `tests/agents.rs` | 1 | 同源 |
| `tests/mcp_http.rs`、`tests/mcp_stdio.rs` | 各 1 | 同源 |
| `tests/postgres_skill_cross_node.rs` | 0（设置 `HPA_SKILL_ALLOW_UNSANDBOXED=1` 后通过） | 同源：技能测试执行 python；该开关只用于受限环境，生产禁用 |

判据：这些用例在允许 `sandbox-exec` 的 macOS 主机上应通过；在 Linux CI 上会走不同沙箱实现，需单独确认（当前 CI 只跑 `cargo test --workspace`，未覆盖 `hpa-skill` 的 seatbelt 用例）。

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

1. **声称某模块已切 PostgreSQL 之前**，先 `grep -c 'hpa_control\.' <file>`；0 就是没切。
2. **测试计数只写实测值**，并写清环境（沙箱/真实 PG/CI）。
3. **区分“本地模式仍可用”与“PG 模式已验证”**：两者都要有对应测试，缺哪个就在本表标出来。
4. **`BLOCKED_EXTERNAL` 要写清缺什么**（凭证？主机？IdP 租户？），不要用“待后续”含糊过去。
