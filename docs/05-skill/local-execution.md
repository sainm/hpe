# Skill 本地执行

> 本地 Skill 执行必须在没有云端 Control Plane、没有网络的情况下工作。模型可选；脚本必须真实执行并留下结果/审计。

## 1. 前置条件

- 安装 Hisha Local Runtime（见 [Local Runtime](../02-architecture/local-runtime.md)）。
- 本地目录可写；Python/Node 运行时按 Skill 需要安装（或使用容器）。
- 模型可选：本地模型（Ollama/llama.cpp/OpenAI 兼容）、云模型（联网时）、无模型。

## 2. 三种运行方式

| 方式 | Skill/脚本 | 模型 | 网络 | 数据 |
| --- | --- | --- | --- | --- |
| Local | 本地 | 本地/无 | 无/局域网 | 本地 |
| Hybrid | 本地 | 云 | 受控 | 按策略发送必要上下文 |
| Cloud | 平台 | 云/本地 | 平台 | 平台存储 |

本文件聚焦 Local；Hybrid/Cloud 的差异在数据边界与执行位置。

## 3. 本地流程

1. `hpa skill import ./invoice-audit` → 校验/扫描/写入不可变版本快照。
2. `hpa skill test skill_invoice_audit --case smoke` → 沙箱执行脚本/断言。
3. `hpa skill run skill_invoice_audit --input '{"invoiceId":"inv_1","items":[]}'` → 确定性脚本入口。
4. `hpa skill sign skill_invoice_audit` → 用本地 Ed25519 密钥签名内容哈希；Skill Center/API 也可编辑文本并生成新版本。
5. `hpa skill publish skill_invoice_audit` → 许可/测试/签名完整性门禁后发布；`hpa skill export skill_invoice_audit --target claude` 导出。
6. 产物写入 `~/.hpa/artifacts/<sha256>`；审计 `~/.hpa/logs/audit.jsonl`。
7. 断网重跑，结果一致（确定性入口）。Agent 加载 Skill（需模型）属于 Phase 4。

## 4. 目录与注册

```text
~/.hpa/
├── hpa.db                 # skills / skill_versions / skill_files / skill_runs
├── artifacts/<sha256>     # 内容寻址产物
├── logs/audit.jsonl       # 执行审计
├── keys/plugin-ed25519.key # 本地 Skill/Plugin Ed25519 私钥（0600）
├── skills/drafts/         # Skill Center 编辑生成的临时源目录
├── skills/signatures/     # 按 skill/version 保存的签名记录
└── tmp/                   # 执行临时工作区（运行后清理）
```

- 本地 Registry（SQLite）记录 Skill/版本/内容哈希/许可/测试/运行；版本快照文件以 BLOB 保存。
- 版本不可变；Skill Center 编辑必须提升版本并由后端写入新草稿目录。
- 导出为标准目录后可放入 `~/.claude/skills/<name>/`、`$CODEX_HOME/skills/<name>/` 或项目内 `.agents/skills/`。

## 5. 无模型模式

- 只支持 `entry.kind = script` 或明确的确定性工具链。
- 不调用模型；不做自然语言理解；输入/输出 schema 明确。
- 适合定时任务、批处理、本地文件转换、数据校验。
- 无法完成需要推理/选择的任务；UI 明确提示。

## 6. 本地模型

- Provider：Ollama、llama.cpp server、OpenAI 兼容本地端点。
- 配置：endpoint/model/context/token；健康检查。
- 模型文件/下载由用户管理；Hisha 不自动下载大模型。
- 隐私：本地推理不出网；仍受 Skill 脚本网络策略约束。

## 7. 脚本运行时

| 语言 | 检测 | 默认 |
| --- | --- | --- |
| Python | `python3 --version` | 子进程；venv/依赖由 Skill 声明 |
| Node.js | `node --version` | 子进程；npm 依赖锁定 |
| Shell | `bash`/`sh` | 仅命令白名单；高风险 |
| 容器 | Docker/Podman | 高风险/不可信 Skill |

依赖安装：

- 优先使用 Skill 自带锁文件/离线包；
- 不在运行时静默 `pip install`/`npm install`（可配置，但需网络/审批）；
- 缺失依赖 → 明确错误/安装指引。

## 8. 沙箱与权限

- 工作区：每次执行临时目录；输入只读；`artifacts/` 可写。
- 文件：只允许声明路径；拒绝 `..`/绝对路径/符号链接逃逸。
- 网络：默认关闭；需要时按域名/端口白名单 + 审批。
- 命令：白名单；禁止 shell 拼接；子进程数限制。
- 资源：CPU/内存/时长/输出大小；超限终止。
- Secret：只注入绑定凭证；不写日志；退出清理。
- 高风险：容器；本地 OS 限制较弱时 UI 明确风险。

## 9. 本地 MCP 与 Skill

- Skill 可作为本地 MCP Server 的工具后端（确定性脚本）。
- MCP Server 进程通过 stdio/HTTP 调用 Skill Runtime；使用能力令牌。
- 外部客户端（Claude/Copilot/Codex）连接本地 MCP Server → 调用工具 → 本地脚本 → 结果。
- 全程本地/离线；审计写入 `~/.hpa/logs/audit.jsonl`。

## 10. 同步（可选）

- 本地发布 → 云端 Registry（需登录/权限）；包签名/校验和。
- 云端拉取团队 Skill → 本地草稿；不自动启用不可信 Skill。
- 冲突：本地草稿优先；已发布版本服务器权威。
- 不同步 secret；不同步其他租户数据。

## 11. 离线行为

| 动作 | 离线 |
| --- | --- |
| 导入/校验/测试 | 可用 |
| 确定性脚本执行 | 可用 |
| Agent + 本地模型 | 可用（模型已安装） |
| Agent + 云模型 | 不可用；提示 |
| 市场/云同步 | 不可用；排队 |
| 本地 MCP | 可用 |

## 12. 验收（Phase 2）

- 无网络机器上：导入 Skill → 校验 → 执行脚本 → 生成 artifact → 查看日志（已实现并测试）。
- 沙箱拒绝越权文件/网络；审计记录（已实现并测试）。
- 导出标准目录与 Claude/Codex 标准路径（已实现）；Codex 通过 `codex debug prompt-input` 实际发现导出的 Skill，Claude Code 真实会话加载未自动执行。
- 同一输入重复执行结果一致（确定性入口，已测试）。
- Agent + 本地模型加载 Skill 属于 Phase 4，不在本次实现范围。

## 13. 已知限制

- macOS/Windows 的 OS 级沙箱弱；高风险需容器。
- 本地模型能力/上下文有限；复杂 Agent 任务可能失败。
- 依赖安装/模型下载不由平台静默完成。
- 本地无多租户；以工作区/项目隔离为主。

## 14. 相关文档

- [Skill Runtime](skill-runtime.md)
- [Skill 安全](skill-security.md)
- [Local Runtime](../02-architecture/local-runtime.md)
- [MCP Server](../06-mcp/mcp-server.md)
- [PoC B](../11-roadmap/poc-plan.md)
