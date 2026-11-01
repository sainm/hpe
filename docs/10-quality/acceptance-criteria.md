# 验收标准

> 每个 Phase 的退出标准必须可执行、可复现。未达标的项写“未做”，不得用文档/占位符冒充完成。

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

## 相关文档

- [测试策略](test-strategy.md)
- [性能要求](performance-requirements.md)
- [路线图](../11-roadmap/implementation-roadmap.md)
- [需求](../00-product/product-requirements.md)
