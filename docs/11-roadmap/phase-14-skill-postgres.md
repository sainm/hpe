# Phase 14：Skill 元数据 PostgreSQL 权威

> 状态：**本地验收完成**（2026-11-03）。Phase 14 把 Skill 包与版本元数据从租户 SQLite 迁到 PostgreSQL；本地 SQLite 模式保持不变。

## 1. 范围

- `HPA_POSTGRES_RUNTIME=1` 时，以下数据只读写 `hpa_tenant` PostgreSQL schema（租户 RLS）：
  - `skills`：Skill 摘要、latest version、license、status。
  - `skill_versions`：manifest、SKILL.md、校验报告、content hash、来源路径、发布状态。
  - `skill_files`：包内文件路径、SHA-256、大小与字节内容。
  - `skill_runs`：测试/运行输入、输出、stdout/stderr、exit code、sandbox、耗时。
  - `skill_run_artifacts`：运行产物元数据与本地存储路径。
- 本地 SQLite 模式、Skill API、Skill Center、CLI 与 MCP 工具目录不改变语义；`HPA_POSTGRES_RUNTIME=1` 时 API 从 PostgreSQL 读取。
- Skill 配额在 PostgreSQL 模式按 `hpa_tenant.skills` 计数。

## 2. 行为契约

- 导入按 `(skill_id, version)` 冻结：相同 content hash 幂等返回 `inserted=false`；同版本不同内容返回 `SKILL_VERSION_FROZEN`。
- 先写 `skills` 行再写 `skill_versions`/`skill_files`，满足 PostgreSQL 外键；文件内容以 BYTEA 保存。
- 发布更新 `skill_versions.status/published_at` 与 `skills.status/updated_at`；跨节点读取立即看到发布状态。
- 测试与运行产生的 run/artifact 写入 PostgreSQL；其他节点可通过 `/api/skills/{id}/runs`、`/api/skill-runs/{id}` 与 artifacts 读取。
- 签名文件仍保存在节点本地文件系统；Phase 14 只迁移元数据。跨节点发布前需在目标节点具备签名验证所需文件。
- `skill_run_artifacts` 在 PostgreSQL 中保存元数据与本地 `stored_path`；跨节点可读取 artifact 元数据，但 artifact 内容仍依赖节点本地文件或共享对象存储。
- `hpa skill/agent ...` CLI 仍直接操作本地 SQLite；PostgreSQL 模式的生产路径是 `hpa serve` 的 HTTP API。

## 3. PostgreSQL schema

复用 `migrations/postgres/0002_tenant_core.sql`：

```text
hpa_tenant.skills
hpa_tenant.skill_versions
hpa_tenant.skill_files
hpa_tenant.skill_runs
hpa_tenant.skill_run_artifacts
```

所有表由 FORCE RLS 隔离；写事务使用 `SET LOCAL ROLE hpa_tenant_runtime` 与 `set_config('hpa.tenant_id', ..., true)`。

## 4. 验收

```bash
HPA_POSTGRES_TEST_URL=postgresql://user@host:5432/db \
  cargo test -p hpa-local --test postgres_skill_cross_node
```

`postgres_skill_cross_node` 覆盖：

- 节点 A 导入 fixture Skill，节点 B 立即可见摘要与版本。
- 节点 B 读取 `/source` 中的 `scripts/audit.py` 与包内文件。
- 节点 A 签名/发布后，节点 B 读取到 `status=published`。
- 节点 B 运行 Skill 测试与脚本；节点 A 读取 run 与 `summary.json` artifact。
- 两节点 SQLite `skills/skill_versions/skill_files/skill_runs/skill_run_artifacts` 均为 0 行。
- 配额上限 1 时再次导入返回 429，确认 Skill 计数来自 PostgreSQL。

门禁：全量 Rust 235 passed / 1 ignored；真实 PostgreSQL 18.4 上 25 个 PG 集成测试文件（26 用例）通过；Web 11 Vitest、扩展 63 Vitest + 7 host checks、品牌/IR/契约/文档/镜像/OTel/供应链门禁通过。

## 5. 非目标

- 不迁移 Skill 签名私钥/签名文件到 PostgreSQL；不改变签名或信任模型。
- 不实现跨节点 Skill 包文件系统同步、远程 Marketplace、Skill 执行沙箱所有权或 Skill 工具调用的第二执行引擎。
- 不改变本地 SQLite 模式与既有 CLI 路径。
