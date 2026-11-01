# Skill 规范

> Skill 遵循 Agent Skills 开放标准。Hisha 不创建私有 Skill 格式；平台只在标准目录外增加 `hpa.skill.json` 扩展清单与治理元数据。

## 1. 标准目录

```text
invoice-audit/
├── SKILL.md
├── hpa.skill.json     # 可选扩展清单
├── references/
│   └── policy.md
├── scripts/
│   └── audit.py
└── assets/
    └── template.xlsx
```

- `SKILL.md` 必填：YAML frontmatter + instructions。
- `references/`：按需加载的文档。
- `scripts/`：确定性脚本；由宿主执行。
- `assets/`：模板/字体/图片等。
- 标准字段：`name`（≤64）、`description`（≤1024）、可选 `license`、`compatibility`（≤500）、`metadata`、`allowed-tools`。
- 正文建议 ≤500 行；更多内容放 references。

## 2. 扩展清单

```json
{
  "schemaVersion": "hpa.skill/v1",
  "id": "skill_invoice_audit",
  "name": "invoice-audit",
  "version": "1.2.0",
  "license": "Apache-2.0",
  "entry": { "kind": "instructions" },
  "scripts": [
    {
      "id": "audit",
      "path": "scripts/audit.py",
      "runtime": "python3",
      "args": ["--input", "${inputDir}", "--output", "${artifactDir}"],
      "timeoutMs": 120000,
      "network": "none"
    }
  ],
  "permissions": {
    "filesystem": { "read": ["${inputDir}"], "write": ["${artifactDir}"] },
    "network": [],
    "commands": ["python3"],
    "env": ["LANG"]
  },
  "dependencies": [
    { "type": "skill", "id": "skill_policy", "range": "^2.0.0" },
    { "type": "python", "name": "pypdf", "range": ">=5,<6" }
  ],
  "tests": [
    { "name": "smoke", "script": "audit", "input": {}, "expect": { "exitCode": 0 } }
  ],
  "trust": { "level": "community", "signature": null }
}
```

规则：

- 扩展清单不是标准替代；缺失时按标准目录加载，默认最小权限。
- 未知字段保留；导入/导出不丢失。
- 版本 semver；发布后不可变。
- `entry.kind`：`instructions`（Agent 指导）/`script`（确定性入口）/`both`。
- 依赖/权限/脚本必须在发布前解析/校验。

## 3. 执行模式

| 模式 | 触发 | 模型 | 说明 |
| --- | --- | --- | --- |
| Agent-guided | Agent 选择 Skill | 需要（本地/云） | 加载 instructions/references；Agent 按指导调用工具 |
| Deterministic | 用户/Workflow/MCP 调用脚本入口 | 不需要 | 直接运行声明的 script，输入/输出 schema |
| Hybrid | 平台 Agent + 本地脚本 | 云模型 + 本地脚本 | 数据策略明确 |
| Test | Skill Test Console | mock/可选 | 断言/快照 |

> Skill 本身不是可执行程序；Deterministic 模式执行的是 `hpa.skill.json` 声明的脚本，不是 `SKILL.md`。

## 4. 校验

导入/发布前：

1. `SKILL.md` 存在；frontmatter 必填/长度/类型。
2. 目录/文件：路径安全（拒绝 `..`/绝对路径/符号链接逃逸/设备文件）；文件数/总大小限制。
3. 扩展清单 schema；脚本/依赖/权限引用存在。
4. 脚本：语言支持、可解析、无危险 shebang；静态扫描。
5. 依赖：版本范围/冲突/许可。
6. 测试：至少一个 smoke 测试（发布必需）。
7. 许可/来源：未知/专有 → 不可发布市场。

错误结构：`code/path/message/fixHint`；不静默忽略。

## 5. 版本与兼容

- `version` semver；发布不可变。
- 依赖范围；解析锁定；冲突报错。
- 兼容性：`compatibility`（标准字段）+ Hisha `runtimes`/`os`。
- 弃用：标记 + 迁移指引；保留期。
- 导入/导出：标准目录 + 可选 `hpa.skill.json`；外部客户端忽略扩展。

## 6. 权限

- 文件：读/写路径（工作区/输入/产物）；默认只读输入、临时写。
- 网络：域名/端口；默认无。
- 命令：白名单；默认无 shell。
- 环境：白名单；不注入 secret（除非显式凭证绑定）。
- 资源：CPU/内存/时长/输出大小。
- 权限在安装/发布时展示；运行时强制；高风险需审批。

## 7. 测试 Profile

- `tests` 声明：name/script/input/expect（exitCode/stdoutSchema/artifacts）。
- Test Console：mock 模型/工具；本地沙箱；快照；覆盖率（脚本/分支）。
- CI：导入 → 校验 → 测试 → 扫描 → 签名；失败阻止发布。
- 兼容性：导出到 Claude Code/Codex 的加载测试（PoC B）。

## 8. 与外部客户端

- 导出标准目录；`hpa.skill.json` 可选。
- Claude Code：`.claude/skills/<name>/`。
- Codex：`.agents/skills/<name>/` 或 `$CODEX_HOME/skills/`。
- VS Code/Copilot：按当前官方文档；不能假设同一路径。
- 不把 Hisha 私有字段写入 `SKILL.md`；外部客户端忽略扩展清单。

## 9. 相关文档

- [Skill Runtime](skill-runtime.md)
- [本地执行](local-execution.md)
- [Skill 安全](skill-security.md)
- [分发与版本](skill-distribution.md)
- [Agent Skills 标准研究](../01-research/standards/agent-skills.md)
