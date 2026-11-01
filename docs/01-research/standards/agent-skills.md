# Agent Skills 标准研究

> 标准来源：`agentskills.io/specification`（Anthropic `anthropics/skills` 仓库的 `spec/agent-skills-spec.md` 已指向该地址）。<br>
> 结论：Hisha 的 Skill 格式**必须**遵循该标准；平台只增加 `x-hisha` 扩展元数据与外部封装，不创建私有 Skill 格式。

## 1. 标准结构

```
skill-name/
├── SKILL.md          # 必填：YAML frontmatter + 指令
├── scripts/          # 可选：确定性脚本
├── references/       # 可选：按需加载的参考文档
└── assets/           # 可选：模板/字体/图片等
```

`SKILL.md` 的 frontmatter：

| 字段 | 必填 | 约束 | 用途 |
| --- | --- | --- | --- |
| `name` | 是 | ≤64 字符，建议 kebab-case | 技能标识 |
| `description` | 是 | ≤1024 字符 | 触发条件与能力说明；决定是否被选中 |
| `license` | 否 | 文本/指向 LICENSE | 许可声明 |
| `compatibility` | 否 | ≤500 字符 | 宿主/环境兼容性提示 |
| `metadata` | 否 | 键值 | 自定义元数据 |
| `allowed-tools` | 否 | 列表/文本 | 建议的工具白名单（宿主可解释） |

正文建议 ≤500 行；更多内容放入 `references/`，按需加载（渐进披露）。校验工具：`skills-ref`。

## 2. 执行语义

- Skill **不是可执行程序**；它是 instructions + references + scripts + assets 的知识包。
- 宿主（Agent Runtime）加载 metadata，按 description 选择 Skill，读取 SKILL.md；需要时再读取 references/scripts。
- 脚本由宿主在沙箱中执行；网络/文件/命令权限由宿主控制。
- 标准不定义持久状态、版本、签名、依赖或沙箱；这些是平台扩展。

## 3. 外部客户端差异

- Claude Code：`SKILL.md` 放在 `.claude/skills/<name>/`；支持插件市场；`allowed-tools` 可限制工具。
- OpenAI Codex：从源码可见 `.agents/skills/`、`CODEX_HOME/skills`、插件根 `skills/` 加载；解析 frontmatter（`codex-rs/skills/src/parser.rs`、`selection.rs`）。
- GitHub Copilot/VS Code：Agent Skills 支持随版本变化；不能假设目录/字段与 Claude Code 完全一致。
- 因此“导出 Skill”应产出标准目录 + 每个客户端的安装说明/适配脚本，而不是一个万能压缩包。

## 4. Hisha Skill Profile

标准字段保持不变；Hisha 在 `metadata` 或独立的 `hisha.skill.json` 中增加：

```json
{
  "schemaVersion": "hisha.skill/v1",
  "id": "skill_...",
  "version": "1.2.0",
  "entry": { "kind": "instructions" },
  "scripts": [
    { "path": "scripts/run.py", "runtime": "python3", "timeoutMs": 60000, "network": "none" }
  ],
  "permissions": {
    "filesystem": { "read": ["./assets"], "write": ["./artifacts"] },
    "network": [],
    "commands": ["python3"]
  },
  "dependencies": [{ "type": "skill", "id": "skill_...", "range": "^1.0.0" }],
  "tests": [{ "name": "smoke", "input": {}, "expect": {} }],
  "signature": { "algo": "ed25519", "keyId": "...", "value": "..." }
}
```

约束：

- `hisha.skill.json` 是**扩展清单**，不是 Skill 的替代格式；没有它，标准 Skill 仍可导入/导出/运行。
- 未知字段必须保留（导入/导出不丢失）。
- 标准字段校验失败时不可发布；扩展校验失败时给出可修复错误。
- 版本/签名/权限在平台层，不写入 `SKILL.md` 的私有字段（避免污染标准）。

## 5. 导入/导出

### 导入

1. 读取 `SKILL.md`，校验 frontmatter（name/description/长度）。
2. 校验路径安全：拒绝 `..`、绝对路径、符号链接逃逸、设备文件；限制总大小/文件数。
3. 识别脚本与依赖；标记为不可信，除非签名/来源可信。
4. 生成 `hisha.skill.json`（如缺失），默认最小权限。
5. 静态扫描：危险命令、网络调用、secret 读取、prompt injection 模式；输出报告。
6. 进入草稿/待审核状态，不自动发布。

### 导出

1. 输出标准目录（`SKILL.md` + scripts/references/assets）。
2. 可选附带 `hisha.skill.json`（外部客户端可忽略）。
3. 清理平台内部字段/本地路径；保留许可与 NOTICE。
4. 生成各客户端安装说明（Claude Code/Codex/VS Code/Copilot）。
5. 导出包签名与校验和。

## 6. 安全

- Skill 的 instructions 是不可信内容，可能包含 prompt injection；Agent Runtime 必须把 Skill 内容与系统策略分离，禁止 Skill 覆盖权限/审批/预算。
- 脚本在沙箱中执行；默认无网络、只读工作区、临时写目录；高风险操作需审批。
- 供应链：来源信任、签名/校验和、依赖锁定、SBOM。
- 市场：人工/自动审核、评分、举报、下架。

## 7. 与 Hisha 其他概念的关系

| 概念 | 关系 |
| --- | --- |
| Skill vs Tool | Skill 是知识与流程指导；Tool 是可调用操作；Skill 可以指导调用 Tool/MCP/Workflow |
| Skill vs Plugin | Plugin 是分发包装，可包含多个 Skill；Skill 是标准内容单元 |
| Skill vs Workflow | Skill 不保证确定性/可重放；Workflow 是确定性编排；Skill 可以指导 Agent 调用 Workflow |
| Skill vs Agent | Agent 是执行主体；Skill 是 Agent 可加载的能力包 |

## 8. 待验证

- 各客户端对 `allowed-tools`/`metadata` 的实际解释。
- 标准是否会引入版本/签名/依赖字段；若引入，Hisha 应迁移而不是继续私有扩展。
- 脚本运行时的跨平台差异（Python/Node/Shell）。
- 大 Skill 的上下文成本与渐进披露效果。

## 9. 证据

`anthropics/skills` 的 `README.md`、`template/SKILL.md`、`skills/*/SKILL.md`、`skills/*/LICENSE.txt`；`agentskills.io/specification`；OpenAI Codex `codex-rs/skills/src/{parser,selection,loading}.rs`。
