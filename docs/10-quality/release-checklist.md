# 发布清单（Phase 13 / P13-08）

> 这里定义“什么算一版可发布的 Auralis”。凡是需要外部凭证的验收项，保持 `BLOCKED_EXTERNAL`，不得用本地 mock 代替。
> 打包与校验脚本：`scripts/release-pack.mjs`、`scripts/release-verify.mjs`。

## 1. 前置条件

| 组件 | 版本 | 校验方式 |
| --- | --- | --- |
| Rust | 1.99.1（MSRV 1.97） | `rustc --version`、`rust-toolchain.toml` |
| Node | ≥ 26.13（本地实测 22.x 亦可运行脚本） | `node --version` |
| pnpm | 12.16.1（`corepack pnpm`） | `corepack pnpm --version` |
| Python | 3.13 | `uv run --frozen python --version`（可选 AI Worker） |
| PostgreSQL | 18.x | `psql -tAc 'select version()'` |
| Playwright | 由根 `package.json` 锁定 | `node -e "require('playwright')"` |

## 2. 门禁矩阵

发布前必须全绿（命令在仓库根执行）：

```bash
# 静态与契约
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
corepack pnpm run check          # brand / ir / contracts / docs / mirrors / otel / supply-chain / audit

# 单元与集成
cargo test --workspace --no-fail-fast
(cd apps/web && corepack pnpm test && corepack pnpm build)
(cd apps/vscode-extension && corepack pnpm test && corepack pnpm build)
(cd services/ai-worker && uv run --frozen pytest -q && uv run --frozen ruff check . && uv run --frozen mypy src)

# PostgreSQL（需要 HPA_POSTGRES_TEST_URL 指向一个可写的空库）
HPA_POSTGRES_TEST_URL=postgresql://user@host:5432/db cargo test -p hpa-local --test postgres_rls \
  --test postgres_import --test postgres_distributed --test postgres_event_bus \
  --test postgres_execution_store --test postgres_workflow_store --test postgres_trigger_store \
  --test postgres_runtime --test postgres_execution_lease --test postgres_execution_recovery \
  --test postgres_atomic_execution_fire --test postgres_mcp --test postgres_scheduler_leader \
  --test postgres_worker_registry --test postgres_token_store --test postgres_worker_daemon

# 端到端
node scripts/run-local-e2e.mjs
HPA_POSTGRES_URL=postgresql://user@host:5432/empty_db node scripts/multi-node-e2e.mjs
```

说明：

- 未设置 `HPA_POSTGRES_TEST_URL` 时，PostgreSQL 套件会打印 `skipped` 并返回成功——**这种情况不算发布证据**。
- 在没有 `sandbox-exec` 的环境（容器内嵌套沙箱）中，12 个依赖 seatbelt 的用例会失败并报 `sandbox_apply: Operation not permitted`；发布验收必须在真实主机上跑，不允许据此判断功能回归。

## 3. 打包

```bash
corepack pnpm run release:pack                       # release 构建 + 打包
corepack pnpm run release:pack -- --profile debug --skip-build   # 快速验证流水线
```

产物写入 `dist-release/`（已在 `.gitignore` 中）：

```
hpa-<version>-<sha>-<platform>/
  bin/hpa                 # 服务端二进制（serve / worker / db / doctor …）
  web/                    # 已构建的前端静态资源
  migrations/postgres/    # SQLx 迁移，按版本号顺序
  deploy/                 # systemd / worker env / OTel compose 示例
  sbom/                   # CycloneDX：Rust 与 Python
  docs/                   # 发布说明、清单、PostgreSQL/观测性契约
  SHA256SUMS              # 每个文件的 sha256
hpa-<version>-<sha>-<platform>.tar.gz
hpa-....tar.gz.sha256     # 压缩包校验和
hpa-....manifest.json     # 版本、git sha、工具链、文件计数、压缩包摘要
```

`manifest.json` 是发布记录的事实来源：它记录构建时的 git sha、profile、平台、rustc/node/python 版本，以及打包进去的迁移/前端/SBOM 数量。

## 4. 校验

```bash
corepack pnpm run release:verify -- dist-release/hpa-<...>.tar.gz
```

校验脚本会：

1. 对照 `.tar.gz.sha256` 验证压缩包；
2. 解包后逐文件核对 `SHA256SUMS`；
3. 读取 `manifest.json`（在压缩包旁边或包内）；
4. 用临时 `--data-dir` 运行 `hpa doctor` 并断言 `status = ok`；
5. 确认 `migrations/postgres/*.sql` 与 `web/index.html` 存在。

任何一步失败都会以非零退出码结束。

## 5. 签名

- 密钥签名（key-based cosign）与 OCI provenance 已接入市场包校验链路；发布包本身可用 `cosign sign-blob` 对 `.tar.gz` 签名，公钥随发布记录一起分发。
- `scripts/install-cosign-domestic.sh` 安装镜像版 cosign（固定 digest）。
- **keyless（Fulcio/Rekor）仍为 `BLOCKED_EXTERNAL`**：需要可访问的 Sigstore 信任根与 OIDC 身份，当前环境无法完成。

## 6. 回滚

| 对象 | 步骤 |
| --- | --- |
| 二进制 | 停止服务 → 用上一版 tarball 的 `bin/hpa` 覆盖 → 启动 → `hpa doctor` 与 `/api/ready` 复核 |
| 前端 | 随 tarball 的 `web/` 覆盖；`hpa serve` 内嵌静态资源，重启即生效 |
| PostgreSQL schema | 迁移**前向只有**：`hpa db status` 查看已应用版本；回滚需从迁移前备份恢复（`pg_dump` 或 `hpa enterprise` 租户备份），不要手改 `_sqlx_migrations` |
| SQLite 租户库 | 用租户备份 `restore`（会要求重启），或直接还原 `tenants/<id>/hpa.db` 文件 |
| 企业控制库 | `control.db` 在备份范围内需单独 `VACUUM INTO`；PostgreSQL 模式下令牌/审计/Worker 已在库中，按数据库恢复 |

回滚后必须复核：`/api/ready` 四项检查、`/metrics` 中 `hpa_scheduler_last_tick_timestamp_seconds` 新鲜、以及一次 `workflow.execute` 冒烟。

## 7. 已知边界（发布说明必须原文带上）

- 外部 AI 客户端（Claude Code / Codex / Copilot）的真实模型会话调用**未验收**，状态 `BLOCKED_EXTERNAL`。
- 企业 IdP / CA / Vault / 生产 Collector / 多节点滚动升级**未验收**，同样 `BLOCKED_EXTERNAL`。
- 企业控制面的租户、成员、配额、SCIM、浏览器会话仍以 SQLite 为权威；PostgreSQL 模式下已切走的是 Workflow/Execution/Trigger、Worker 注册与任务、平台令牌与审计。
- 调度器为每节点各自 tick + 数据库乐观锁，不是 leader-only 派发；时间同步依赖宿主机 NTP。
- MCP `2026-07-28` 的官方 conformance 场景尚未提供；已验收的是 `2025-11-25` 核心矩阵 9/9。

## 8. 发布记录模板

每次发布在 `docs/10-quality/` 下追加一条记录，至少包含：

- 版本号、git sha、构建时间、构建机器与工具链；
- 门禁矩阵的实际通过数（Rust/Web/扩展/Python/PostgreSQL/E2E）；
- 压缩包 sha256、SBOM 文件名与哈希；
- 与上一版的行为差异和迁移要求；
- 未验收项与对应负责人。
