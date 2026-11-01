# Anthropic Skills 研究报告

> 仓库：`anthropics/skills` · `main` 快照（无 release，2026-11-01）· tarball SHA-256 `6fbb56693915c88d...`<br>
> 研究方式：源码（446 文件）+ 19 个 Skill 的 SKILL.md/LICENSE + 规范指针；未运行。<br>
> 结论：**Direct Reuse of the Agent Skills standard；sample skills 仅 Apache-2.0 部分可用**。

## 1. 项目定位

Anthropic 的 Agent Skills 参考实现与示例集合：每个 Skill 是一个文件夹，含 `SKILL.md`（YAML frontmatter + 指令）、可选 `references/`、`scripts/`、`assets/`。标准文本已迁移到 `agentskills.io/specification`；本仓库提供示例、模板与 Claude 插件市场配置。

## 2. 核心功能

- 19 个示例 Skill：文档处理、设计、MCP 构建、Skill 创建、Web 测试、品牌、内部沟通等。
- `skill-creator`：创建/评估/优化 Skill 的方法与工具。
- `mcp-builder`：编写高质量 MCP Server 的指导。
- `template/SKILL.md`：最小模板。
- `.claude-plugin/marketplace.json`：作为 Claude Code 插件市场分发。
- 渐进披露：先加载 metadata/description，需要时再读 reference/script。

## 3. 技术架构

- 纯文件结构；没有运行时/服务器。
- `SKILL.md` frontmatter：`name`（≤64 字符）、`description`（≤1024 字符）必填；可选 `license`、`compatibility`（≤500）、`metadata`、`allowed-tools`。
- 目录约定：`scripts/`、`references/`、`assets/`；`SKILL.md` 建议 ≤500 行。
- 校验工具 `skills-ref`（标准生态）；本仓库提供模板。
- 插件市场通过 `.claude-plugin/marketplace.json` 组织多个 Skill。

## 4. 技术栈

Markdown + YAML frontmatter + 脚本（Python/JS 等）+ 资源文件；无框架依赖。宿主负责加载、工具权限与执行。

## 5. 数据模型

Skill 元数据（name/description/license/compatibility/metadata/allowed-tools）；文件树；脚本参数/资源引用；可选的插件清单。没有平台数据库模型。

## 6. 执行机制

- Skill 本身**不是可执行程序**；宿主（Claude/Claude Code/Hisha Agent Runtime）根据 description 选择 Skill，加载 SKILL.md 与按需引用文件，再按指令调用工具/执行脚本。
- 脚本执行、沙箱、网络/文件权限由宿主负责；Skill 不声明强制的运行环境（除 `allowed-tools`/compatibility 提示）。
- 没有持久化/状态机；Skill 是无状态的知识/流程包。

## 7. 扩展机制

- 新增 Skill 文件夹 + SKILL.md；用 `references/` 分文件控制上下文；用 `scripts/` 提供确定性操作；用 `assets/` 放模板/字体等。
- 插件市场清单组织多个 Skill；Claude API 支持上传 Skill。
- `skill-creator` 提供评估循环（测试 prompt、量化/定性评估）。

## 8. 插件开发方式

编写 SKILL.md（触发描述、步骤、示例、边界）+ 可选脚本/参考；用 `skills-ref` 校验；通过插件市场或 API 分发。Hisha 应完全兼容此结构，只在 `metadata`/扩展字段中加入 `x-hisha` 元数据。

## 9. API 设计

本仓库无 API；Claude API 的 Skills 端点、Claude Code 的文件系统/插件市场是外部接口。Hisha 需要定义自己的 Skill Registry/Import/Export API，但包格式保持标准目录结构。

## 10. 部署架构

文件系统/插件市场/API 上传；无服务端。Hisha Local Runtime 可直接扫描 `~/.hisha/skills/`；云端存对象存储并版本化。

## 11. 前端交互设计

无 UI；宿主呈现 Skill 列表/调用。Hisha 的 Skill Center 需要自建编辑器（表单 + SKILL.md 原文 + 文件树 + 测试控制台）。

## 12. 许可证

**没有仓库根 LICENSE**，每个 Skill 单独许可：

- 14 个 Apache-2.0：academy-guide、algorithmic-art、brand-guidelines、canvas-design、claude-api、discernment-nudge、frontend-design、internal-comms、mcp-builder、skill-creator、slack-gif-creator、theme-factory、web-artifacts-builder、webapp-testing。
- 4 个专有/source-available：docx、pdf、pptx、xlsx（`© 2025 Anthropic, PBC. All rights reserved.`，使用受 Anthropic 协议约束）。
- 1 个无许可证文件：doc-coauthoring（默认保留所有权利，不可再分发）。

因此不能把整个仓库当作 Apache-2.0；只能复用标准结构与 Apache-2.0 示例，并保留 NOTICE/版权。标准文本在 agentskills.io，按其许可使用。

## 13. 可直接复用

- Agent Skills 目录结构、frontmatter 字段、渐进披露、校验规则。
- Apache-2.0 的示例 Skill 作为导入/导出测试夹具与写法参考（保留版权）。
- `skill-creator` 的评估思路（生成测试 prompt、量化/定性评估）。
- `mcp-builder` 的 MCP 工具设计指导（作为开发文档参考）。

## 14. 需要二次开发

- Hisha 扩展元数据（`x-hisha`：权限、依赖、入口、测试用例、签名）。
- 导入器/导出器的校验、路径穿越/符号链接防护、脚本清单。
- 测试控制台（mock 模型/工具、断言、快照）。
- 市场分发/版本/签名；与标准兼容的降级（未知字段保留）。

## 15. 仅值得借鉴

- “description 决定触发”的写法与边界说明。
- references/scripts/assets 分层与渐进披露。
- 模板/校验器/评估闭环。
- 插件市场清单的组织方式。

## 16. 不适合采用

- 专有 docx/pdf/pptx/xlsx 模板（不可再分发）。
- 无许可证的 doc-coauthoring。
- 把 Skill 当作可执行程序/插件 ABI。
- 让 Skill 直接拥有不受限的 shell/网络权限。

## 17. 与 Hisha 的兼容性

高。Hisha Skill Center 应以该标准为唯一格式；平台内部只增加 `x-hisha` 扩展元数据，不改变标准字段与目录。导出为标准目录即可被 Claude/Codex 等客户端使用（各客户端加载路径不同，见外部客户端研究）。

## 18. 引入后的维护成本

低：标准稳定、文件格式简单；主要成本在导入校验、安全沙箱、测试与市场治理。示例 Skill 仅作为测试/文档，不进入产品资产。

## 证据路径

`README.md`、`spec/agent-skills-spec.md`（指向 agentskills.io）、`template/SKILL.md`、`skills/*/SKILL.md`、`skills/*/LICENSE.txt`、`THIRD_PARTY_NOTICES.md`、`.claude-plugin/marketplace.json`、`skills/skill-creator/*`、`skills/mcp-builder/*`。
