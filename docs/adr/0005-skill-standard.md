# ADR-0005：Skill 采用 Agent Skills 标准

- 状态：已接受
- 日期：2026-11-01
- 决策者：架构负责人（Phase 0）

## 背景

Skill 是 Hisha 的核心分发单元，需要被平台内部 Agent、Local Runtime、外部 AI 客户端共同使用。私有格式会锁死生态；标准（SKILL.md + references/scripts/assets）已被 Claude/Codex 等采用。

## 决策

- 以 `agentskills.io/specification` 为唯一 Skill 格式：`SKILL.md` frontmatter（name/description 必填，可选 license/compatibility/metadata/allowed-tools）+ 目录约定。
- 平台扩展放在 `hpa.skill.json`（脚本/权限/依赖/测试/签名），不写入 `SKILL.md` 私有字段；未知字段保留。
- Skill 不是可执行程序；脚本由 Skill Runtime 在沙箱执行。
- 导入/导出标准目录；按客户端路径生成安装说明；拒绝专有/无许可内容。
- 只复用 Apache-2.0 示例；不使用 docx/pdf/pptx/xlsx/doc-coauthoring。

## 备选

- 私有插件/Skill 格式：可控但无法分发到外部客户端。
- 直接把脚本作为 Skill：丢失 instructions/references/渐进披露。
- 只支持 Agent 内 Skill：无法本地/Workflow/MCP 复用。

## 后果

- 标准字段/行为受外部规范约束；需跟踪 agentskills.io 演进。
- 权限/沙箱/依赖/签名是平台扩展，不能假设外部客户端理解。
- 导出兼容性测试（Claude Code/Codex/VS Code）成为门禁。
- 许可/归属/导出拒绝必须有工具支持。

## 证据

[Agent Skills 标准研究](../01-research/standards/agent-skills.md)、[Skill 规范](../05-skill/skill-specification.md)、[Anthropic Skills 报告](../01-research/projects/anthropic-skills.md)。
