# ADR-0012：开源复用与许可边界

- 状态：已接受（法务复核）
- 日期：2026-11-01
- 决策者：架构/法务（Phase 0）

## 背景

研究确认：n8n 使用 SUL（禁止商业分发）；Dify 修改版 Apache（多租户/LOGO 限制）；Activepieces/Flowise/LiteLLM 有 EE/Enterprise 目录；Anthropic Skills 混合许可（4 个专有 + 1 个无许可）；MCP 处于 MIT→Apache 过渡；MinIO 可能是 AGPL。Hisha 是商业平台，必须避免许可污染。

## 决策

1. **只允许宽松许可**进入核心：MIT、Apache-2.0、BSD-2/3、ISC、PostgreSQL License 等；逐依赖核查。
2. **禁止**：SUL/SSPL/AGPL/未知许可进入产品核心；如需 AGPL 组件（如 MinIO），必须评估/替换/隔离，默认不用。
3. **禁止复制**：n8n 代码/节点/前端；Dify 代码（商业多租户/LOGO 限制）；任何 EE/Enterprise 目录；Anthropic 专有/无许可 Skill；MCP 参考服务器作为生产实现。
4. **允许复用**：Temporal（MIT）、LangGraph（MIT）、LiteLLM MIT 部分、MCP 官方 SDK（MIT）、OTel（Apache-2.0）、Agent Skills 标准、Apache-2.0 示例 Skill、awesome-copilot（MIT，测试语料）。
5. **保留义务**：LICENSE/NOTICE/版权；修改说明；CC-BY 署名；SBOM；市场许可元数据/审核。
6. **门禁**：CI 扫描受限路径/许可证；SBOM（CycloneDX/SPDX）；漏洞扫描；发布归属页；法务复核高风险。
7. **Hisha 自身许可证**：待决；推荐 Open Core（IR/SDK/Local Runtime Apache-2.0；云/企业治理商业许可），见 open-issues。

## 备选

- 采用 n8n/Dify 代码：许可/商业风险高；不可接受。
- 全部自研：慢/重复，不需要。
- 忽略 EE 边界：法律/供应链风险；不可接受。
- AGPL 全栈：与商业托管/嵌入冲突。

## 后果

- 减少可选库（如 MinIO/部分工具）；需要替代方案/商业许可。
- 每次引入依赖需许可/维护/退出方案；开发速度略慢但可发布。
- 市场需要许可元数据/审核/撤回；不能无审核分发。
- 法务复核不是一次性；Phase 5/6 前复核云端/市场/计费。

## 证据

[许可证分析](../01-research/license-analysis.md)、[复用策略](../01-research/reuse-strategy.md)、各项目报告 §12、实际 LICENSE/EE 文件。
