# Skill 集成

> 目标：把 Auralis Skill 以标准 Agent Skills 目录安装到 VS Code / GitHub Copilot 识别的原生位置；Skill 只指导 Copilot，不执行脚本。

## 1. 安装目标

| 目标 | 路径 | 适用 | 说明 |
| --- | --- | --- | --- |
| 项目（默认） | `<root>/.github/skills/<name>/` | 团队/仓库 | Copilot/VS Code 官方原生路径 |
| 项目（Claude） | `<root>/.claude/skills/<name>/` | 跨 Claude Code | 需用户显式选择，避免重复 |
| 项目（通用） | `<root>/.agents/skills/<name>/` | 跨客户端 | 需用户显式选择 |
| 个人（Copilot） | `~/.copilot/skills/<name>/` | 个人 | 不写入仓库 |
| 个人（Claude/通用） | `~/.claude/skills/`、`~/.agents/skills/` | 跨客户端 | 显式选择 |
| 扩展内置 | VSIX `skills/<name>/SKILL.md` + `chatSkills` | 官方只读技能 | 随扩展版本；不写用户目录 |

规则：

- 目录名必须与 `SKILL.md` frontmatter `name` 完全一致；否则 VS Code/Copilot 不加载。
- 默认只安装到 `.github/skills`；不同时写多个目录。
- 个人安装不写 workspace；项目安装不改用户 home。
- 不把 Auralis 扩展清单写入 SKILL.md；元数据放 sidecar。

## 2. 搜索/安装

### 搜索

- 来源：本地 Runtime Registry、Auralis Control Plane、Marketplace。
- 展示：name/description/version/trust/license/permissions/scripts/来源/评分。
- 过滤：已安装/可更新/信任/许可/兼容客户端。
- 不把不可信/无许可 Skill 自动安装。

### 安装

1. 选择目标（项目/个人/具体目录）。
2. 预览：SKILL.md、文件树、脚本/权限、许可、信任、版本。
3. 调用 Auralis Skill Service 校验（Rust；不重复实现校验器）：
   - frontmatter name/description/长度；目录名匹配。
   - 路径安全（拒绝 `..`/绝对路径/符号链接逃逸/设备文件）。
   - 文件数/大小；脚本可解析；引用存在。
   - 许可/来源/签名；风险提示。
4. 检查目标冲突：已存在同名不同内容 → 拒绝/要求更新/另存；不静默覆盖。
5. 写入临时目录 → 校验 → 原子移动到目标；保留旧版本（更新时）。
6. 写入 sidecar（workspace `.hpa/skills.json` 或 globalState）记录版本/hash/来源/目标。
7. 刷新树/文件监听；提示 Copilot 重新加载（命令/Chat 重新打开）。

安装失败：清理临时目录；不留下半安装状态；回滚旧版本。

## 3. 更新/移除

### 更新

- 检查 Auralis 版本/hash；显示 changelog/权限差异。
- 若用户编辑过目标文件：标记 dirty；提供 diff/保留本地/覆盖/另存，不自动覆盖。
- 原子替换；旧版本备份；更新 sidecar。
- major 版本需确认；权限扩大需确认。
- Copilot 可能缓存技能；提示重新加载/新会话。

### 移除

- 只删除由 Auralis 安装的文件（sidecar 记录 + 内容 hash）。
- 用户新增/修改文件：保留并提示；不递归删除未知内容。
- 依赖检查：其他 Skill/Workflow 引用 → 警告/阻止。
- 删除 sidecar 条目；不删除 Auralis Runtime/Registry 源。

## 4. 版本管理

- 版本 = Auralis SkillVersion semver；sidecar 记录 `installedVersion`、`contentHash`、`source`、`target`、`installedAt`。
- 不把版本写进 SKILL.md 私有字段；标准 frontmatter 不变。
- 可回滚：保留上一版本目录（`.hpa/skills-backup/<name>/<version>/`）或重新下载。
- 本地修改检测：hash 不一致 → dirty；更新需用户确认。
- 扩展内置 `chatSkills` 版本随扩展；不能单独更新；升级扩展即更新。

## 5. 扩展 `chatSkills`

```json
{
  "contributes": {
    "chatSkills": [
      { "path": "./skills/hpa-workflow/SKILL.md" }
    ]
  }
}
```

- 目录结构：`skills/<name>/SKILL.md`；`name` 必须匹配目录。
- 只读、随 VSIX 分发；适合“如何用 Auralis”的官方指导，不承载用户私有 Skill。
- 不包含 secret；不写用户仓库。
- 版本与扩展版本绑定；VSIX 签名/市场审核。
- 与项目安装 Skill 同名时：VS Code 可能按优先级选择；避免同名冲突（扩展内置使用 `hpa-*` 前缀）。

## 6. 与 Copilot 的关系

- Skill 是 instructions/references/scripts/assets；Copilot 负责加载/选择/遵循。
- Skill 可指导 Copilot 调用 Auralis MCP Tools；工具权限/审批由 Auralis 服务端执行。
- 扩展不执行 Skill 脚本；脚本由 Auralis Runtime 在沙箱执行（如 Skill 提供确定性入口）。
- 不保证 Copilot 每次都会调用 Skill；description 要明确触发条件。
- Custom Instructions 可以补充“优先使用 Auralis 工具”，但不能替代 Skill/审批。

## 7. 安全

- 工作区未信任：禁止安装/更新/移除；只读浏览。
- 路径：拒绝 symlink/路径穿越/绝对路径；写入限制在工作区/个人技能目录。
- 内容：把 Skill 指令视为不可信；提示注入不能改变 Auralis 权限/审批。
- 脚本：不在扩展执行；Auralis Runtime 沙箱/权限。
- 供应链：来源/签名/许可/扫描；不可信 Skill 警告。
- 多根工作区：明确目标根；不跨根写入。
- 不覆盖用户文件；不删除未知内容；失败回滚。

## 8. 验收

- 安装到 `.github/skills` 后 VS Code/Copilot 能发现 Skill。
- 目录名/frontmatter 不匹配时拒绝并给出修复。
- 更新/移除/回滚；本地修改保护。
- 个人/项目目标切换；多根工作区正确。
- 扩展 `chatSkills` 可加载；同名冲突有提示。
- 未信任/无许可/路径攻击被阻止。
- 不执行脚本；执行日志在 Auralis Runtime。

## 9. 相关

- [Copilot 集成](copilot-integration.md)
- [MCP 集成](mcp-integration.md)
- [Security](security.md)
- [Skill 规范](../05-skill/skill-specification.md)
- [官方证据](evidence.md)
