# 验收标准

> 每个 Phase 的退出标准必须可执行、可复现。未达标的项写“未做”，不得用文档/占位符冒充完成。
>
> 下表保留原完整路线图退出标准；当前本地实现的实际范围与剩余项见 [实现状态与验收边界](implementation-status.md)。

## Phase 0：Research & Architecture

| 编号 | 标准 | 验证 |
| --- | --- | --- |
| P0-01 | 16 个项目有独立报告，覆盖 18 个字段 | 文件/目录检查 |
| P0-02 | 每个项目锁定版本/发布日期/许可证/tarball SHA-256 | 研究 README 表 |
| P0-03 | 许可证边界（Dify/n8n/EE/专有 Skill/MCP 过渡）有原文依据 | license-analysis |
| P0-04 | 五类复用分类覆盖所有项目 | open-source-analysis |
| P0-05 | 15 个最终问题每题有推荐/依据/替代/风险/实施 | final-questions |
| P0-06 | 9 张关键图存在且 Mermaid 可解析 | diagrams |
| P0-07 | IR/契约/数据模型/API/权限/UI 规格可指导实现 | 文档评审 |
| P0-08 | 三个 PoC 有目标/步骤/验收/风险 | poc-plan |
| P0-09 | 文档内部链接无死链；必需文件齐全 | 校验脚本 |

## Phase 1：Core Workflow Platform

| 编号 | 标准 | 验证 |
| --- | --- | --- |
| P1-01 | Canvas/YAML/JSON 语义往返 100%（fixture） | 属性测试/快照 |
| P1-02 | 规范文本往返稳定（L1） | canonical emit/parse |
| P1-03 | 分支/循环/并行/子流程/重试/超时/错误/审批可执行 | E2E/集成 |
| P1-04 | 五类校验阻止非法发布 | 负向测试 |
| P1-05 | 发布版本不可变；执行绑定版本 | DB/API 检查 |
| P1-06 | 本地无云运行 Workflow（无 AI） | 断网测试 |
| P1-07 | 执行事件/时间线/日志/产物可见 | UI/API |
| P1-08 | 崩溃恢复从检查点继续（标准运行时） | 强制杀进程 |
| P1-09 | 审批暂停/恢复/拒绝/超时 | E2E |
| P1-10 | 三语/窄屏（607px）/键盘可用 | UI 测试 |
| P1-11 | 无 n8n/Dify/EE 代码；许可证门禁通过 | SBOM/路径扫描 |

## Phase 2：Local Runtime & Skill Center

| 编号 | 标准 | 验证 |
| --- | --- | --- |
| P2-01 | 标准 Skill 导入/校验/测试/导出 | E2E |
| P2-02 | 本地无云加载/执行 Skill（模型可选） | 断网测试 |
| P2-03 | 脚本沙箱（文件/网络/命令/资源） | 安全套件 |
| P2-04 | 产物/日志/审计完整 | 检查 |
| P2-05 | 导入/导出保留未知字段/许可/NOTICE | fixture |
| P2-06 | 专有/无许可 Skill 拒绝导出/发布 | 负向测试 |
| P2-07 | 外部客户端加载导出 Skill（至少 Claude Code/Codex） | PoC B |
| P2-08 | 本地 MCP Server/Client 可用 | 集成 |

## Phase 3：MCP Ecosystem

| 编号 | 标准 | 验证 |
| --- | --- | --- |
| P3-01 | 2026-07-28 discover/无状态/resultType/MRTR/subscriptions/tasks | conformance |
| P3-02 | 2025-11-25 兼容 | conformance |
| P3-03 | OAuth 2.1/PAT/stdio 认证 | 集成 |
| P3-04 | 工具/资源/Prompt 注册/权限/租户 | E2E |
| P3-05 | 高风险写操作服务端审批 | E2E |
| P3-06 | 限流/审计/监控 | 检查 |
| P3-07 | 至少 2 个真实外部客户端调用 | PoC B |
| P3-08 | 不 token passthrough；SSRF/越权测试通过 | 安全套件 |

## Phase 4：Agent Platform

| 编号 | 标准 | 验证 |
| --- | --- | --- |
| P4-01 | Agent 配置/版本/发布/运行 | E2E |
| P4-02 | 规划/工具/Skill/MCP/Workflow 调用 | 集成 |
| P4-03 | 预算/步数/工具/超时限制 | 测试 |
| P4-04 | interrupt/resume/审批/checkpoint | E2E |
| P4-05 | 多 Agent supervisor/handoff/预算继承 | 测试 |
| P4-06 | 工具权限/租户/审批不可绕 | 安全套件 |
| P4-07 | 本地/云一致性 | 对比测试 |
| P4-08 | Agent 不绕过 Workflow 审批 | 负向测试 |

## Phase 5：Plugin & Connector Ecosystem

| 编号 | 标准 | 验证 |
| --- | --- | --- |
| P5-01 | 插件 manifest/签名/依赖/安装/升级/回滚/卸载 | E2E |
| P5-02 | SDK 可构建/测试/打包/签名 | 示例插件 |
| P5-03 | 权限/沙箱/不在 API 进程加载代码 | 安全测试 |
| P5-04 | Connector 认证/测试/限流/重试/SSRF | 集成 |
| P5-05 | 市场审核/评分/撤回/许可 | E2E |
| P5-06 | 插件贡献能力/多入口独立权限 | 检查 |
| P5-07 | 许可/SBOM/漏洞门禁 | 发布检查 |

## Phase 6：Enterprise & SaaS

| 编号 | 标准 | 验证 |
| --- | --- | --- |
| P6-01 | 多租户/RLS/跨租户 0 | 安全测试 |
| P6-02 | OIDC/SSO/RBAC/审计 | 集成 |
| P6-03 | Private/Cloud Worker/Hybrid 注册/断线/恢复 | 部署测试 |
| P6-04 | 配额/限流/计量/计费（如启用） | 检查 |
| P6-05 | 备份/恢复/保留/删除 | 演练 |
| P6-06 | 合规控制点/审计导出 | 评审 |
| P6-07 | 性能/SLO 达标 | 压测 |

## 通用“不可接受”

- 用 mock 冒充真实集成；空 fixture 通过。
- 只跑单路径/单文件；覆盖率/场景不全。
- UI 白屏但文本审计通过；Monaco 内容不在快照。
- 崩溃恢复未强制杀进程；重复副作用未测。
- 许可证只查根许可证；EE/专有内容混入。
- 文档计数/链接/字段与实现不一致。
- 未验证的 AI 生成流程可发布/执行。

## Phase 7～12 增量本地验收

这些 Phase 是 Phase 6 后增加的本地增量，不替代上面的完整 SaaS/云端退出标准。

| 编号 | 本地标准 | 验证 |
| --- | --- | --- |
| P7-L01 | 扩展构建、单元测试并在真实 Runtime 在线时执行全部 Host 检查 | VSIX/Extension Host |
| P7-L02 | Runtime、MCP、Skill 安装和未信任工作区边界可回归 | Host/负向检查 |
| P8-L01 | start/events/complete 在发网前持久化，进程重启后排空 | outbox 重启/HTTP 集成 |
| P8-L02 | 过期/旧 lease 上报拒绝并清理，不能污染新 lease | 409 集成 |
| P8-L03 | 远程审批 SLA 到点按 `onTimeout` 恢复并重排 | 真实 daemon E2E |
| P13-SCHED-01 | 两个 Scheduler 实例只有一个 leader，standby 不执行 schedule | 双进程 PostgreSQL |
| P13-SCHED-02 | leader 被强杀后 standby 在 TTL 内接管；旧 leader 重启不能形成双 leader | 强杀/接管测试 |
| P9-L01 | OIDC Code+PKCE、state/nonce、session/CSRF、登出与三种签名算法 | mock IdP + HTTP/浏览器 |
| P10-L01 | OAuth Code+PKCE、DCR、同意、refresh 轮换、吊销、内省、JWKS | API + Playwright |
| P11-L01 | schedule、webhook、event 均只能运行已发布版本 | 契约/HTTP |
| P11-L02 | webhook/event 事件 ID 幂等，停用与错误签名拒绝 | 负向集成 |
| P13-EVENT-01 | topic 多订阅者部分成功后，同一 Event ID 只补投缺失订阅者 | Scheduler 负向集成 |
| P13-TRIGGER-01 | `HPA_POSTGRES_RUNTIME=1` 时 publish state、due/claim、启停、trigger fire 与 execution 写 PostgreSQL | 真实 PG + API |
| P13-TRIGGER-02 | topic 发布重复 Event ID 返回 409，PostgreSQL trigger fire 不新增 | 真实 `hpa serve` E2E |
| P13-EVENT-02 | Topic Event 经 PostgreSQL outbox queued→processed；崩溃可重投，API 不等待执行 | 真实 PG + 异步 consumer |
| P13-EVENT-03 | 相同 Event ID/payload 返回 duplicate，不同 payload 冲突；最终只有一条 fire/execution | 负向 E2E |
| P13-EVENT-04 | PostgreSQL execution、start event、trigger fire 原子提交；重复 fire 不留下孤立 execution | 真实 PG 事务测试 |
| P12-L01 | Prometheus、readiness、执行时间线在 1440/607 可读取 | API + Playwright |
| P12-L02 | 企业指标鉴权与探针公开边界 | 企业 HTTP |
| P13-SCIM-01 | 仅租户 `hpa_ent_` owner/admin 或 platform admin 可读写 Users/Groups，跨租户 ID 不可解析 | HTTP 负向矩阵 |
| P13-SCIM-02 | Users/Groups CRUD、精确 filter、分页、PATCH 与 ETag 语义完整 | HTTP + E2E |
| P13-SCIM-03 | 不同角色映射组冲突整事务失败；删除组恢复直接角色；admin 不能改动 owner 记录 | 负向集成 |
| P13-SCIM-04 | 旧 SQLite 成员 schema 无损升级，PostgreSQL RLS/复合外键/导入可重复 | migration + 真实 PG |
| P13-SCIM-05 | 至少一个真实企业 IdP 完成 SCIM 推送和 token 轮换 | `BLOCKED_EXTERNAL` |
| P13-MCP-01 | session/subscription/notification/task 在 PostgreSQL 中租户、token、TTL 隔离 | 真实 PG 负向测试 |
| P13-MCP-02 | `tasks/get/update` 跨节点读取持久任务，取消命令按 owner node/lease 消费 | 双进程集成 |
| P13-MCP-03 | 强停 task owner 后命令不丢，重启恢复并跨节点触发 tasks SSE | 双进程强停测试 |
| P13-MCP-04 | 至少两个真实 MCP 客户端完成工具调用矩阵 | `BLOCKED_EXTERNAL` |
| P13-MCP-05 | 持久通知 backlog 有界；慢消费者断开；窗口内按 `Last-Event-ID` 重放 | 真实 PG + HTTP SSE |
| P13-MCP-06 | 官方 2025-11-25 核心 server 场景通过；测试夹具仅在显式测试模式启用 | `pnpm test:mcp-conformance` 9/9 |
| P13-MCP-07 | 官方 2026-07-28 server conformance | `BLOCKED_EXTERNAL`（工具链尚无场景） |
| P13-EXEC-01 | `HPA_POSTGRES_RUNTIME=1` 时 API/Runtime 的 execution、events、artifact 写入 PostgreSQL | 真实 PG + `hpa serve` |
| P13-EXEC-02 | 强停并重启后 execution/events 仍可由 API 读取；workflow snapshot、state tenant、event execution 冲突失败关闭 | 重启 E2E + 负向测试 |
| P13-EXEC-04 | PostgreSQL 模式同步 remote execution marker，local recovery 不接管远程执行 | 真实 PG 负向测试 |
| P13-EXEC-05 | 每日执行与并发配额按 PostgreSQL execution 计数，不读取旧 SQLite 表 | Repository 测试 |
| P13-EXEC-06 | execution lease 过期后另一节点接管，旧 holder 的 state commit/release 被拒绝 | 真实 PG fencing 测试 |
| P13-EXEC-07 | 空 data_dir 节点从 PG 恢复 running execution、执行并释放 lease | 进程级 E2E |
| P13-WORKFLOW-01 | `HPA_POSTGRES_RUNTIME=1` 时 draft/publish/list/get/version 全部走 PostgreSQL | 真实 PG Store + API |
| P13-WORKFLOW-02 | 相同 workflow/version 内容变化失败关闭；新版本可发布；跨租户不可见 | 负向集成 |
| P13-EXEC-03 | Workflow、Trigger、Enterprise、Worker、Agent 等全部生产 Repository 切换 PostgreSQL | `IN_PROGRESS` |
| P13-MARKET-01 | 远程 source/publisher/key/package 可在 Web 管理并安装 | API + Playwright |
| P13-MARKET-02 | 1440/607 无溢出，source/publisher axe serious/critical=0 | Playwright + axe |
| P13-MARKET-03 | 在线 OSV/离线漏洞库、key-based Sigstore bundle + OCI digest provenance、Web/VS Code Remote UI、持久 SSE 撤回推送；Fulcio/Rekor keyless、Registry attestation 拉取与跨控制面 federation | 部分完成 |
| P13-OTLP-01 | Rust/Python traces、metrics、logs 到达真实 HTTP receiver，跨进程 W3C traceparent 串联 | Rust/Python OTLP 测试 |
| P13-OTLP-02 | Collector 故障隔离、导出队列有界、ready/API 不被阻塞 | Collector 停机测试 |
| P13-OTLP-03 | error/latency/probabilistic tail sampling 有界；三信号删除已知敏感属性；配置有自动门禁 | `check:otel` + Collector 配置 |
| P13-OTLP-04 | 真实 HA Collector、traceID 一致路由、压测与容量评审 | `BLOCKED_EXTERNAL` |
| Q-L01 | 干净环境有锁文件、可执行浏览器依赖、CI 与供应链门禁 | `pnpm check`/CI |
| Q-L02 | 强制杀进程后运行中 Workflow 从检查点恢复 | 独立进程恢复测试 |
| Q-L03 | 关键页面 serious/critical axe 为 0，首个 Tab 可离开 body | Playwright + axe |
| Q-L04 | 包下载使用国内镜像；容器镜像使用 DaoCloud 且固定 digest；权威漏洞源作为显式例外 | `pnpm check:mirrors` |

## 相关文档

- [测试策略](test-strategy.md)
- [性能要求](performance-requirements.md)
- [路线图](../11-roadmap/implementation-roadmap.md)
- [需求](../00-product/product-requirements.md)
