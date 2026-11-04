# Phase 27：双控制面 + 双 Worker 滚动升级演练

> 状态：**本地验收完成**（2026-11-04）。两个控制面共享 PostgreSQL、两个 Worker 经故障切换代理消费 Worker Job；控制面逐个替换、Worker 逐个强杀并重启后，6 个 Job 全部成功，重派 Job 的 `attempts` 增加但 `executionId` 保持不变。

## 1. 范围

- 新增 `scripts/worker-rolling-upgrade-e2e.mjs` 与 `test:worker-rolling`。
- 两个控制面节点共享同一 PostgreSQL，使用独立数据目录和稳定 `HPA_NODE_ID`。
- 一个 loopback 故障切换代理承载 Worker 的 `--control-url`，在控制面替换时切换目标。
- 两个 Worker 注册为不同 workerId，`max-concurrency=1`，通过代理领取 Job。
- 一个本地慢 HTTP 服务作为 Workflow 的 `http.request` 目标，使 Worker 在 Job 执行期间可被强杀。
- 两个控制面逐个滚动：切换代理、SIGKILL 旧节点、启动同版本替代节点；期间 job-5 在途并最终成功。
- 两个 Worker 逐个滚动：分别在 job-1、job-4 执行中 SIGKILL，等待 30 秒租约过期后由另一 Worker 重派完成；随后用新数据目录重启被杀 Worker，继续领取新 Job。
- 断言：Job 不丢失、重派 `attempts >= 2`、`executionId` 不变、所有 Job 终态 `succeeded`、控制面滚动后两个 Worker 仍 active。

## 2. 行为契约

- 同版本滚动；跨版本升级仍需要停机窗口，不在本阶段范围。
- Worker token、Job、租约与 Execution 状态都在 PostgreSQL；控制面替换不依赖本地 SQLite。
- Worker 通过反向代理访问控制面，符合私有部署中控制面置于 HTTPS 反向代理之后的形态。
- Worker 被强杀后，旧 lease 由控制面在下一次 lease 时原子清理并重排；旧进程无法再提交。该 fencing 由既有 `postgres_worker_registry` / `postgres_worker_daemon` / execution lease 测试覆盖。
- Job 是至少一次语义：慢 HTTP 目标可能收到重复调用；内部 execution 记录保持同一个 `executionId`，外部副作用仍需业务幂等键。
- 租约 TTL 默认 30 秒；演练不缩短 TTL，按真实过期窗口验证重派。
- 演练要求空 PostgreSQL 数据库；脚本自行执行 migration、enterprise init 并创建临时数据目录。

## 3. 验收

```bash
cargo build -p hpa-local
HPA_POSTGRES_URL=postgresql://user@host:5432/empty_db pnpm test:worker-rolling
```

- 新演练 **21/21 checks PASS**：租户/令牌/工作流、两个 Worker 各自领取、Worker-1 强杀后重派、Worker-2 强杀后重派、单 execution ID、控制面 A/B 替换、滚动期间 job-5 成功、滚动后两个 Worker active、6 个 Job 全部成功。
- 既有演练回归：`test:multi-node` 10/10 checks、`test:rolling` 17/17 checks。
- 慢 HTTP 目标共 8 次调用（6 个 Job + 2 次重派），证明外部调用至少一次而内部执行未分裂。
- 全量 Rust 252 passed / 0 failed / 1 ignored；PostgreSQL 31 文件（32 用例）、Web 12、扩展 65 + 7 host checks、文档 184 文件 892 链接与供应链门禁保持通过。

## 4. 非目标

- 不验证跨版本 schema/binary 混跑；版本不匹配仍按文档 fail-closed。
- 不改变 Worker 状态机、lease TTL 或 outbox 设计；本阶段只补演练与证据。
- 不覆盖真实外部负载均衡/VIP、mTLS、对象存储和自动扩缩容；这些仍需生产环境验收。
