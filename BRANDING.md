# Branding Contract

> 永久内部 ID：**hpa**。当前展示名：**Auralis**。历史代号：**Hisha**。
> 目的：展示名可以随时更换；技术标识永远不随品牌变。

## 1. 唯一品牌源

[`branding/brand.json`](branding/brand.json) 是唯一品牌配置：

- `productName` / `shortName`：展示名（当前 Auralis）。
- `internalId`：永久 `hpa`。
- `historicalAliases`：历史代号（Hisha）。
- `identifiers`：包 scope、crate 前缀、env 前缀、数据目录、keychain、OTel、MCP、VS Code、CLI、manifest、schema authority。

代码生成/构建读取此文件；**不在代码中硬编码展示名**。展示字符串通过 UI/i18n/品牌常量注入。

## 2. 永久标识（不得带品牌）

| 用途 | 值 |
| --- | --- |
| npm scope | `@hpa/*` |
| Rust crate | `hpa-*` |
| Python package | `hpa-*` |
| 环境变量 | `HPA_*` |
| 本地数据目录 | `~/.hpa/` |
| Keychain service | `hpa` |
| OTel namespace | `hpa.*` |
| MCP provider id | `hpa-local` |
| MCP resource scheme | `hpa://` |
| MCP tool 名 | `workflow.list` 等，无品牌前缀 |
| VS Code publisher | `hpa` |
| VS Code extension name | `workbench` |
| VS Code extension ID | `hpa.workbench` |
| CLI binary | `hpa` |
| Skill manifest | `hpa.skill.json` |
| Plugin manifest | `hpa.plugin.json` |
| Plugin extension | `.hplugin` |
| Schema authority | `https://hpa.dev`（占位；域名分配后替换，仍不含品牌） |

VS Code extension ID、Marketplace publisher、数据目录、env 前缀、DB schema、MCP provider id 一旦发布/写入数据后不可变；必须在第一天用上表值。

## 3. 可随品牌更换

- UI 标题、Logo、图标、颜色、邮件签名、支持入口。
- 文档标题/正文、官网、营销页、Marketplace `displayName`/描述/截图。
- i18n 中的展示名（key 保持中性，值来自品牌源）。
- 桌面/安装器显示名、帮助文案、通知文案。
- 域名/URL（通过配置，不硬编码；`brand.json.urls`）。

## 4. 禁止

- 在 `package.json` name、crate 名、env 前缀、DB schema/table/column、API path、MCP provider/tool 名、OTel/Prometheus 指标、数据目录、keychain、VS Code extension ID 中使用 `Auralis`/`Hisha`。
- 从展示名生成技术 ID（例如 `auralis.workflow.execute`）；技术 ID 使用领域名（`workflow.execute`）。
- 在业务代码中硬编码展示名；必须通过品牌常量/i18n。
- 用品牌名作为 Workflow/Node/Skill/Capability ID 前缀（使用稳定 ID/哈希）。
- 修改 `hpa` 内部 ID；如确需修改，单独做数据/API/扩展发布迁移，不与展示名更换混在一起。

## 5. 当前文档的遗留标识

- 既有 Phase 0 研究/架构文档中的 `hisha-*`、`hisha.skill.json`、`hisha.dev` 等示例属于**历史占位**，不作为新代码标识。
- 新代码/新配置/迁移必须使用 `hpa`；脚手架创建时以 `brand.json` 为准。
- 历史文档可以保留 Hisha/Auralis 展示名；技术契约以本文件 + ADR-0020 为准。
- 后续批量把历史文档示例迁移到 hpa 时，只改技术标识，不改展示名/历史叙述。

## 6. 改名 runbook

1. 更新 `branding/brand.json` 的 `productName`/`shortName`/`tagline`/URLs/assets。
2. 更新 UI/i18n 展示值、Logo/图标、官网、Marketplace `displayName`/描述。
3. 不修改 `internalId`、包名、crate 名、env、数据目录、DB、API、MCP、extension ID。
4. 运行 `node scripts/check-brand.mjs`。
5. 跑品牌快照/视觉回归/链接检查。
6. 发布说明写明“仅展示名变更，技术 ID 不变”。

## 7. 门禁

`scripts/check-brand.mjs`：

- 检查 `brand.json.internalId === "hpa"`。
- 扫描代码/配置/脚本/部署文件；`docs/`、`branding/`、`README.md`、`BRANDING.md`、`CHANGELOG.md` 允许展示名。
- 发现非白名单路径出现 `Auralis`/`Hisha`（大小写不敏感）即失败。
- 检查文件名；不允许品牌出现在技术路径。
- 以后接入 CI：`node scripts/check-brand.mjs` 作为 required check。

## 8. 相关

- [ADR-0020](docs/adr/0020-permanent-internal-id-hpa.md)
- [技术栈](docs/13-tech-stack/README.md)
- [VS Code/Copilot 模块](docs/14-vscode-copilot/README.md)
