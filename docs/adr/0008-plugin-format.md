# ADR-0008：插件格式与签名

- 状态：已接受
- 日期：2026-11-01
- 决策者：架构负责人（Phase 0）

## 背景

插件需要打包 Node/Connector/Tool/Skill/Agent/MCP 配置并安全分发；不能是无审核的 npm，也不能把 Skill 私有化。需要 manifest、签名、依赖、权限、版本、审核/撤回。

## 决策

- 插件格式 `.hplugin`（zip/tar）或 OCI artifact；内容寻址存储。
- Manifest `hpa.plugin.json`（schemaVersion/id/version/license/compatibility/capabilities/dependencies/permissions/integrity/signature）。
- 签名：SHA-256 + Ed25519 或 Sigstore；发布者密钥/轮换/吊销。
- 安装/升级/卸载：校验/依赖/许可/权限/扫描；代码不在 API 进程加载。
- 插件可含 Skills/MCP 配置/Agents/Node/Connector/Scripts/文档；各类仍遵循自身契约。
- 市场审核/评分/撤回；未知来源默认不可信。

## 备选

- npm 包直接安装：生态大但无签名/权限/审核；供应链风险。
- 平台私有二进制：安全但生态封闭。
- 无 manifest 的目录：版本/依赖/权限无法治理。

## 后果

- 需要 SDK/构建/签名/审核工具链；市场运营成本。
- 插件权限/沙箱/依赖解析复杂；需 CI 门禁。
- 签名/撤回/审计必须从 Phase 5 就建立，后补困难。
- 不能把 EE/专有代码打进插件。

## 证据

[插件架构](../07-plugin/plugin-architecture.md)、[Plugin SDK](../07-plugin/plugin-sdk.md)、[Marketplace](../07-plugin/marketplace.md)。
