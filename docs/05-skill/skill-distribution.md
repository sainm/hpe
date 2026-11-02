# Skill 分发与版本

> 分发单位是标准 Skill 目录（可选 `hpa.skill.json`），不是平台私有包。市场/注册表只增加治理元数据，不改变标准格式。
>
> 当前本地实现已支持目录导入、内容哈希冻结、本地 Ed25519 签名和三种客户端目录导出；官方/社区信任层级、Sigstore、远程 Registry、撤回列表和密钥轮换仍是后续目标。

## 1. 版本

- semver：`MAJOR.MINOR.PATCH`。
- 发布不可变；内容哈希绑定版本。
- 依赖范围：`^`/`~`/精确；解析锁定。
- 兼容性：`compatibility`（标准）+ Hisha `runtimes`/`os`。
- 预发布：`-beta.N`；市场默认不展示。

## 2. Registry

| 字段 | 说明 |
| --- | --- |
| id | 稳定 ID（`skill_...`） |
| name | 标准 name（唯一/租户内） |
| version | semver |
| contentHash | SHA-256 |
| license | SPDX/文本 |
| trust | official/verified/community/untrusted |
| permissions | 文件/网络/命令/资源 |
| dependencies | skill/python/node/connector |
| tests | 通过记录 |
| signature | Ed25519/Sigstore |
| status | draft/published/deprecated/revoked |
| visibility | private/workspace/tenant/public |
| owner | 用户/团队 |

- 本地 Registry（SQLite）与云端 Registry 同构；同步可选。
- 版本解析：精确/范围；冲突报错；不静默降级。
- 删除：保留审计/引用；软删除/下架。

## 3. 导入

1. 来源：目录/zip/URL/市场/插件。
2. 校验：结构/frontmatter/manifest/路径/脚本/依赖/许可。
3. 扫描：危险模式/secret/网络/文件/混淆。
4. 签名/信任：校验/标记。
5. 生成草稿；不自动启用/执行。
6. 测试：smoke/集成；通过后发布。

## 4. 导出

1. 选择版本（已发布/草稿）。
2. 输出标准目录 + 可选 `hpa.skill.json`。
3. 清理内部字段/本地路径；保留许可/NOTICE。
4. 生成客户端安装说明（Claude Code/Codex/VS Code/Copilot）。
5. 签名/校验和；可选离线包。
6. 拒绝导出专有/无许可内容。

## 5. 市场

- 列表：名称/描述/版本/发布者/信任/许可/权限/评分/下载量。
- 详情：SKILL.md 预览、权限、脚本、依赖、版本历史、变更日志。
- 安装：权限确认；依赖解析；版本选择。
- 更新：变更日志/权限差异；不自动升级 major。
- 下架/撤回：通知；已安装版本禁用/隔离。
- 评分/举报/审核；排行榜（可选）。

## 6. 签名与完整性

- 发布：内容哈希 + 签名（Ed25519/Sigstore）。
- 安装：校验签名/哈希/发布者；失败拒绝。
- 撤回：撤销列表；本地/云端检查。
- 密钥：轮换/吊销；离线信任根。

## 7. 依赖

- Skill → Skill：版本范围；循环依赖拒绝。
- Skill → 脚本包：Python/Node 锁文件；离线包。
- Skill → Connector/MCP：能力引用；权限独立。
- 解析：拓扑排序；冲突/缺失错误；不自动安装未知来源。
- SBOM：依赖/许可/漏洞。

## 8. 兼容性

- 标准版本：Agent Skills spec；Hisha 扩展版本。
- 客户端：Claude Code/Codex/VS Code/Copilot 路径/字段差异；导出适配。
- Runtime：Python/Node/Shell 版本；OS；容器。
- 测试矩阵：导入/加载/执行/导出/外部客户端。

## 9. 许可

- 每个 Skill 记录 license；市场展示。
- 禁止专有/无许可内容公开分发。
- Apache/MIT 保留 NOTICE/版权；CC-BY 署名。
- 依赖许可扫描；不合规阻止发布。

## 10. 运营

- 审核 SLA/责任；举报处理；下架流程。
- 指标：安装/使用/失败/举报/撤回。
- 版本采用率；弃用通知。
- 安全公告。

## 11. 相关文档

- [Skill 规范](skill-specification.md)
- [Skill 安全](skill-security.md)
- [Marketplace](../07-plugin/marketplace.md)
- [插件架构](../07-plugin/plugin-architecture.md)
