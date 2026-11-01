# ADR-0020：hpa 作为永久内部 ID

- 状态：已接受
- 日期：2026-11-01
- 决策者：产品/架构

## 背景

产品展示名可能更换（当前 Auralis，历史 Hisha）。如果包名、环境变量、数据目录、数据库、扩展 ID、MCP 标识从展示名派生，改名会引发大范围迁移。需要一次固定内部标识。

## 决策

- **永久内部 ID：`hpa`**；当前展示名：**Auralis**；历史别名：**Hisha**。
- 技术标识使用 `hpa`：
  - npm `@hpa/*`、Rust `hpa-*`、Python `hpa-*`。
  - env `HPA_*`、数据目录 `~/.hpa`、keychain `hpa`、OTel `hpa.*`。
  - MCP provider `hpa-local`、resource scheme `hpa://`；MCP tool 名无品牌前缀。
  - VS Code publisher `hpa`、extension name `workbench`、ID `hpa.workbench`；CLI `hpa`。
  - Skill manifest `hpa.skill.json`、Plugin manifest `hpa.plugin.json`。
  - Schema authority `https://hpa.dev`（占位；域名分配后替换）。
- 展示名只出现在 UI/i18n/文档/市场 displayName/资源；由 `branding/brand.json` 单源驱动。
- 禁止从展示名生成技术 ID；禁止在技术路径/内容中出现 Auralis/Hisha。
- `scripts/check-brand.mjs` 作为 CI 门禁；`docs/`、`branding/`、根 README/BRANDING/CHANGELOG 允许展示名。
- 既有 Phase 0 文档中的 `hisha-*`/`hisha.skill.json`/`hisha.dev` 是历史占位，不作为新代码标识；脚手架使用 hpa。

## 备选

1. `hisha` 永久内部 ID：与历史文档一致，但品牌更换仍受旧名约束。
2. `auralis` 永久内部 ID：当前展示名耦合，改名成本高。
3. 每个品牌一套技术 ID：数据/扩展/协议全部迁移，不可接受。
4. 不用内部 ID、用完整产品名：同样不可接受。

## 后果

- 代码中几乎看不到展示名；开发体验略微“去品牌化”，换名只改展示资源。
- VS Code publisher/extension ID 稳定；Marketplace displayName 可换。
- 数据目录/DB/env/协议不随展示名变化；用户数据无需迁移。
- 若未来必须改内部 ID，单独做迁移 ADR；不与展示名更换混合。
- 历史文档的旧标识需在脚手架/迁移时清理；品牌门禁只覆盖代码/配置，不重复改写历史叙述。

## 证据

[BRANDING.md](../../BRANDING.md)、[brand.json](../../branding/brand.json)、[check-brand.mjs](../../scripts/check-brand.mjs)。
