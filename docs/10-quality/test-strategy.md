# 测试策略

> 测试目标不是覆盖率数字，而是证明：IR 语义一致、执行可恢复、权限不可绕、离线可用、客户端兼容、许可证合规。

## 1. 测试层级

| 层级 | 范围 | 工具（候选） | 门禁 |
| --- | --- | --- | --- |
| Unit | IR/编译器/表达式/节点/权限/工具 | cargo-nextest（Rust）、Vitest（TS）、pytest（Python） | 必须 |
| Property | IR 往返/表达式/状态机 | proptest / fast-check / hypothesis | 必须 |
| Contract | Runtime/Skill/MCP/Plugin/Connector 契约 | Proto/schema fixtures（Rust/TS/Python） | 必须 |
| Integration | API/DB/队列/Worker/沙箱 | Testcontainers/Compose + gRPC | 必须 |
| E2E | UI + API + Worker + Local binary | Playwright + Rust binary | 必须（关键路径） |
| Security | 沙箱/越权/注入/SSRF | 专用套件 + fuzz + cargo-deny/audit | 必须 |
| Chaos | 崩溃/断网/租约/重复 | 故障注入 | Phase 2+ |
| Compatibility | 外部客户端/协议版本 | 真实客户端/夹具 | PoC B |
| Performance | 延迟/吞吐/资源 | criterion/k6/autocannon | Phase 1+ |
| License/SBOM | 依赖许可证/漏洞 | cargo-deny/audit、pip-audit、npm audit、syft/Trivy | 每次发布 |

## 2. Workflow 测试

| 类别 | 用例 |
| --- | --- |
| IR Validation | schema 正/负、端口、悬空边、循环、容器、变量、secret |
| YAML/JSON | 语义往返、规范文本、注释/CST、Unicode/CRLF、重复键/自定义 tag |
| Canvas | 导出/导入、布局忽略、节点/边同步、冲突 |
| Branch | condition/switch、default、未命中 |
| Loop | 空/单/多、while、maxIterations、并发、嵌套 |
| Parallel | all/any/n_of、failFast、取消、输出合并 |
| Retry | 可重试/不可重试、退避、attempt 事件 |
| Timeout | 节点/容器/执行、取消 |
| Resume | 崩溃/检查点/重复副作用 |
| Error | fail/skip/fallback/compensate/工作流错误图 |
| Subflow | 合同、版本、递归、循环引用 |
| Human | 批准/拒绝/超时/重复/恢复 |
| Version | 发布冻结、旧版本执行、迁移 |

### 属性测试

- 随机合法 IR → YAML → IR / JSON → IR：`semanticEqual`。
- 随机表达式 AST → 解析 → 求值 → 序列化：等价。
- 随机状态机事件序列 → 无非法状态转换。
- 随机执行计划 → 崩溃点注入 → 恢复后事件/结果一致（允许重复，幂等去重）。

## 3. Agent 测试

- 规划/工具选择/参数（mock 模型）。
- 预算/步数/工具调用/超时/成本限制。
- 审批批准/拒绝/超时/重复。
- checkpoint/恢复/版本迁移。
- 工具错误/重规划/终止。
- Prompt injection（工具结果/Skill 内容）。
- 多 Agent 预算/深度/权限交集。
- 本地/云一致性。
- 回归：模型/提示/工具变更。

## 4. Skill 测试

- 结构/frontmatter/manifest/路径/符号链接。
- 脚本解析/退出码/stdout/stderr/输出 schema。
- 沙箱：文件/网络/命令/资源/secret。
- 确定性/离线/本地/云。
- Agent-guided（mock 模型/工具）。
- 导入/导出/未知字段/许可。
- 外部客户端加载（Claude/Codex/VS Code）。
- 供应链：签名/哈希/依赖/漏洞。

## 5. MCP 测试

- 2026-07-28：discover/无状态/`_meta`/resultType/MRTR/subscriptions/tasks/缓存/错误码。
- 旧版：initialize/会话/GET/断线重放。
- 认证：OAuth 发现/CIMD/DCR/resource/iss/PKCE/PAT/stdio。
- 工具 schema/大结果/分页/错误。
- 租户隔离/权限/审批/限流/审计。
- SSRF/路径/原型污染/协议走私。
- 真实客户端：Claude Code、VS Code/Copilot、Codex。

## 6. 部署/运维测试

| 模式 | 用例 |
| --- | --- |
| Local | 无网络启动/运行/导入/导出/更新/回滚 |
| Private | Compose/Helm 安装/升级/备份/恢复/HA |
| SaaS | 多租户/RLS/配额/限流/计费（Phase 6） |
| Hybrid | 注册/心跳/断线/重连/租约/凭证隔离 |
| Offline | 云不可用；本地功能；同步队列 |
| Worker | 崩溃/离线/版本不兼容/重复事件 |
| 迁移 | DB/IR/节点版本/回滚 |

## 7. 安全测试

- 越权（API/Workflow Node/Tool/MCP/Skill）。
- 跨租户（ID/缓存/搜索/资源/向量）。
- 沙箱逃逸（路径/符号链接/命令/容器）。
- Prompt injection/工具操纵。
- SSRF/DNS rebinding/重定向/协议走私。
- Secret 泄漏（日志/事件/checkpoint/导出）。
- 供应链（签名/依赖/许可）。
- 限流/资源耗尽/DoS（受控）。

## 8. E2E 关键路径

1. 登录 → 创建 Workflow → 编辑 → 校验 → 运行 → 查看结果。
2. Canvas/YAML/JSON 往返 → 运行。
3. AI 生成 → 校验 → 修复 → 应用草稿。
4. Skill 导入 → 测试 → 本地执行 → 导出。
5. Agent 配置 → 测试 → 调用 Skill/MCP/Workflow → 审批。
6. MCP 注册 → 发现 → 调用 → 审批 → 审计。
7. 外部客户端（PoC B）→ 工具调用 → 本地脚本。
8. 执行失败 → 重试/恢复/补偿。
9. 窄屏（607px）关键页面。
10. 三语切换/无硬编码文案。

## 9. 测试数据

- 固定 fixtures：分支/循环/并行/审批/子流程/错误/大对象/Unicode。
- 生成器：合法/非法 IR；随机事件；随机权限请求。
- 脱敏：生产数据不得进入测试；使用合成数据。
- 快照：IR/YAML/UI 关键页面；更新需评审。

## 10. CI 门禁

- Unit/Property/Contract/Integration/E2E（关键）。
- Schema/静态校验。
- 安全套件（沙箱/越权/SSRF）。
- 许可证/SBOM/漏洞。
- 文档链接/必需文件/18 字段/15 题计数（本仓库）。
- 性能基线（Phase 1+）。
- 失败阻止合并/发布；不允许“跳过”生产门禁。

## 11. 反假绿

- 不只跑单文件/单路径；覆盖率按模块统计。
- 空库/空 fixture 不算通过。
- 真实数据库/队列/沙箱，不用全 mock 证明集成。
- E2E 检查实际 UI/网络，不只看 HTTP 200。
- 外部客户端真实调用；不只用 mock。
- 崩溃恢复必须强制杀进程；不模拟。
- 许可证扫描覆盖传递依赖。

## 12. 相关文档

- [验收标准](acceptance-criteria.md)
- [性能要求](performance-requirements.md)
- [PoC 方案](../11-roadmap/poc-plan.md)
- [安全架构](../02-architecture/security-architecture.md)
