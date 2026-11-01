# 性能要求

> 以下为 Phase 1-4 的**目标值**，需在 PoC/压测中验证；不是当前实测结果。未达标写“未做”，不伪造数字。

## 1. 工作负载假设

| 场景 | 假设 |
| --- | --- |
| 小型 Workflow | 10-30 节点，1-5 次外部调用，< 30s |
| 中型 Workflow | 50-200 节点，10-50 次调用，< 5min |
| 长流程 | 小时-天级，人工审批/等待（durable） |
| Agent | 5-20 步，3-15 次工具调用，模型延迟主导 |
| Skill 脚本 | 1-60s，CPU/IO 混合 |
| MCP | 10-100 工具/租户，10-50 req/s/实例 |
| 并发 | 单 Worker 10-100 并发执行（按节点类型） |

## 2. SLO（目标）

| 指标 | 目标 | 测量 |
| --- | --- | --- |
| API 读 p95 | ≤ 200ms | 服务端 span |
| API 写 p95 | ≤ 500ms（不含长操作） | 同上 |
| Workflow 启动 p95 | Local ≤ 500ms；Cloud ≤ 1s | start → QUEUED/RUNNING |
| 节点调度开销 p95 | ≤ 50ms（不含外部/脚本） | ready → started |
| 事件投递延迟 p95 | ≤ 1s | event at → UI/consumer |
| 执行完成通知 p95 | ≤ 1s（本地） | finish → SSE |
| Agent 首 token p95 | ≤ 3s（模型依赖；不含冷启动） | 请求 → 首 token |
| 工具调用开销 p95 | ≤ 100ms（不含上游） | tool start → upstream request |
| MCP discover/list p95 | ≤ 300ms | Gateway span |
| MCP 调用开销 p95 | ≤ 100ms（不含上游） | call → upstream |
| UI 交互响应 p95 | ≤ 100ms | 前端性能标记 |
| Designer 打开 200 节点 | ≤ 2s（本地） | 首次可交互 |
| 画布 500 节点 | 平移/缩放 ≥ 45fps（目标 60） | 浏览器 |
| 执行列表 10k 条 | 首屏 ≤ 1s | API+UI |
| 错误率 | < 0.5%（5xx，非用户错误） | 监控 |

## 3. 本地 Runtime

| 指标 | 目标 |
| --- | --- |
| 冷启动（CLI serve） | ≤ 3s（不含模型） |
| 空闲内存 | ≤ 300MB（不含模型/浏览器） |
| 安装包 | ≤ 150MB（不含模型/运行时） |
| SQLite 10k 执行查询 | p95 ≤ 200ms |
| 进程崩溃恢复 | ≤ 5s 继续（检查点） |
| 离线启动 | 不等待网络；≤ 3s |

## 4. 容量/扩展

| 组件 | 目标 |
| --- | --- |
| Worker | 水平扩展；单实例 10-100 并发（按类型） |
| API | 无状态水平扩展；连接池/租约 |
| Postgres | 读副本（Phase 6）；分区（事件/审计） |
| Redis | 队列/租约/限流；HA |
| 对象存储 | 产物吞吐按节点；分片/生命周期 |
| MCP Gateway | 无状态；按租户限流；水平扩展 |
| 队列积压 | 可观测；自动扩容（Phase 6） |

## 5. 数据库/存储

| 指标 | 目标 |
| --- | --- |
| 事件写入 p95 | ≤ 20ms（批量） |
| 检查点写入 p95 | ≤ 50ms（≤1MB） |
| 执行查询（按 ID） | p95 ≤ 50ms |
| 审计写入 | 异步/批量；不阻塞业务 |
| 事件保留 | 默认 30 天；归档 |
| 检查点大小 | ≤1MB；大对象 Artifact |
| 事件大小 | ≤64KB；大日志 Artifact |
| Artifact 默认上限 | 100MB/文件（可配） |

## 6. 资源限制（默认）

| 资源 | 默认 |
| --- | --- |
| 节点超时 | 30s（可配） |
| 执行超时 | 1h（standard）；durable 可更长 |
| Agent 步数 | 12 |
| Agent 工具调用 | 20 |
| 并行分支 | 4 |
| 循环次数 | 500 |
| 嵌套深度 | 10 |
| 图节点数 | 1,000（软限；大图告警） |
| 脚本内存 | 512MB |
| 脚本 CPU | 1 core |
| 脚本输出 | 1MB stdout（大结果 Artifact） |
| MCP 工具参数 | 1MB/深度 32 |
| MCP 结果 | 4MB（可配；大结果资源） |

## 7. 可靠性/恢复

| 指标 | 目标 |
| --- | --- |
| SaaS/Private 可用性 | 99.9%（Phase 6 目标） |
| Local | 无可用性 SLO；崩溃恢复 |
| RPO（SaaS） | ≤5min（Postgres PITR） |
| RTO（SaaS） | ≤1h（目标；演练验证） |
| Worker 断线恢复 | 租约超时后重排；≤2×租约 TTL |
| 事件不丢 | 写入确认后才推进；Outbox |
| 重复副作用 | 幂等键/补偿；不承诺 Exactly Once |

## 8. 成本

| 指标 | 目标/说明 |
| --- | --- |
| 模型成本 | 由 LiteLLM 计量；Agent/Workflow 预算 |
| 基础设施/1k 执行 | 目标 ≤ 基线（Phase 4 实测） |
| 存储成本 | 事件/产物保留策略；冷归档 |
| MCP 调用 | 按租户限流/计量 |
| 本地 | 无云成本；模型可选 |

## 9. 测量方法

- 压测：k6/autocannon；真实 DB/Redis/对象存储；固定硬件/网络。
- 指标：OTel spans + Prometheus；p50/p95/p99；冷/热启动分开。
- 场景：小/中/长流程、Agent、MCP、脚本、并发、故障注入。
- 不 cherry-pick；报告样本量/环境/版本/配置。
- 每次发布跑基线；回归 >20% 阻止。
- 前端：Lighthouse/Performance API；画布 FPS；内存泄漏。

## 10. 相关文档

- [测试策略](test-strategy.md)
- [验收标准](acceptance-criteria.md)
- [部署架构](../02-architecture/deployment-architecture.md)
- [风险登记](../11-roadmap/risk-register.md)
