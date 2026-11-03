# PostgreSQL 测试的文件级隔离

> 适用：`crates/hpa-local/tests/postgres_*.rs`（29 个测试文件）。脚本：`scripts/run-postgres-tests.sh`。

## 1. 问题

这些测试共享同一个 `HPA_POSTGRES_TEST_URL` 时，会互相污染：

- 多数文件以 `local` 租户起步，而 `local` 的配额是**全库唯一**的一行（`hpa_control.quotas`）。某个文件把 `max_executions_per_day` 调小（为了验证 429），后续文件的 Agent 运行就会得到 `429 TENANT_QUOTA`。
- `agent_sessions`、`executions` 是累积表，跨文件累加后触发「当日配额」判断。
- 迁移是幂等的，但 `_sqlx_migrations` 的 checksum 测试需要一个**刚迁移过**的库。

结果：`cargo test --workspace` 一次性跑 29 个文件时会出现 7～8 个与被测功能无关的失败；逐个单独跑却全绿。这类红是噪声，会掩盖真缺陷。

## 2. 做法

```bash
HPA_POSTGRES_ADMIN_URL=postgresql://user@host:5432/postgres \
  scripts/run-postgres-tests.sh          # 每个文件一个临时库，跑完即删
```

脚本行为：

1. 从 `HPA_POSTGRES_ADMIN_URL` 解析出 `scheme://authority`，维护库固定连 `/postgres`；
2. 对每个 `postgres_*.rs` 执行 `CREATE DATABASE hpa_t_<run>_<file>`；
3. 用该库作为 `HPA_POSTGRES_TEST_URL` 跑 `cargo test -p hpa-local --test <file>`；
4. 打印每文件结果与 `test result` 摘要，默认删除临时库（`--keep` 保留）；
5. 任一文件失败即非零退出。

要求：账号具备 `CREATEDB`；URL 必须是 `postgresql://user[:pass]@host[:port]/database` 形式（不带 socket 路径）。

## 3. 当前结果（2026-11-04，PostgreSQL 18.4）

- **29/29 文件（30/30 用例）通过**。
- 首次运行暴露了 `postgres_member_authority` 的测试竞态：节点配置 100ms 强制缓存刷新，却在长时间操作后断言 SQLite 缓存必须为 0。该套件已改为验证 PostgreSQL 权威，缓存刷新继续由独立 `postgres_control_refresh` 覆盖；复跑全绿。
- 当前 shell 可以执行 seatbelt，`postgres_skill_cross_node` 不再需要 `HPA_SKILL_ALLOW_UNSANDBOXED=1`。禁止嵌套沙箱的 CI 仍应按失败原因区分平台限制与功能缺陷。

## 4. CI 约定

- CI 里不要把所有 PG 文件指向同一个库；要么用本脚本，要么在矩阵中为每个文件注入独立库名。
- `cargo test --workspace` 的本地全量运行仅用于发现编译/非 PG 回归；PG 结论以本脚本的输出为准。
- 新增 `postgres_*.rs` 时不需要改脚本，文件名匹配即可。
