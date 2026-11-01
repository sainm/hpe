# Local Runtime

> 目标：在个人电脑/本地服务器上，无云、无外部数据库、无 JVM 的情况下运行 Workflow、Agent、Skill、MCP 与脚本；可选同步到云端，但不是运行前提。
> 实现：Rust 单二进制 + SQLite + 嵌入 Web UI；详见 [Local Runtime（Rust）](../13-tech-stack/local-runtime-rust.md)。

## 1. 形态

- **CLI + 本地 Web UI**：`hisha` 命令启动本地 API/UI（默认 `127.0.0.1`），浏览器访问；不在 Phase 1 引入 Electron/Tauri。
- **单进程优先**：API/UI/Registry/Runtime 在同一进程；脚本/高风险工具在子进程/容器。
- **可选桌面封装**：后续可用 Tauri 包装本地 Web UI；不改变运行时架构。
- **打包**：Rust 单二进制（`rust-embed` 嵌入 Web UI + SQLite）；`cargo-dist` 生成 macOS/Linux/Windows 安装包；签名更新。

## 2. 目录

```text
~/.hpa/
├── config.yaml          # 本地配置（非密钥）
├── skills/              # 标准 Skill 目录
├── agents/              # Agent 定义
├── workflows/           # Workflow/IR 草稿与版本
├── mcp/                 # 本地 MCP Server 配置与工具
├── runtime/             # SQLite、检查点、租约、队列
├── artifacts/           # 执行产物（内容寻址）
├── logs/                # 本地日志/审计
├── models/              # 本地模型配置（不存密钥）
├── plugins/             # 已安装插件
└── backups/             # 导出/备份
```

- 使用平台数据目录（Windows `%LOCALAPPDATA%`、macOS `~/Library/Application Support`）作为 `~/.hpa` 的等价物；CLI 暴露路径。
- 文件权限：目录 0700；密钥用 OS keychain（macOS Keychain/Windows Credential Manager/Linux Secret Service），不可用则加密文件 + 主密码。

## 3. 组件

| 组件 | 职责 |
| --- | --- |
| Local API/UI | 本地 REST/SSE/Web UI；仅 loopback |
| Local Registry | Workflow/Agent/Skill/MCP/Plugin/Connector 元数据 |
| Workflow Runtime | 轻量执行器（与 WorkerRunner 同核） |
| Agent Runtime | Python AI Worker（LangGraph，可选；gRPC） |
| Skill Runtime | Rust：Loader/Resolver/Context/Execution/Sandbox/Artifacts |
| MCP Client | Rust RMCP：stdio/HTTP 外部 Server |
| MCP Server | Rust RMCP：本地 stdio/HTTP；能力令牌 |
| Script Runtime | Rust 统一 Executor：Python/Node/Shell/WASM/容器 |
| Local Model Provider | Ollama/llama.cpp/OpenAI 兼容；可禁用 |
| Local Policy | 权限/审批/配额；本地策略文件 |
| Local Storage | SQLite + 内容寻址文件 |
| Sync Client | 可选、出站 HTTPS、签名包 |

## 4. 数据

- SQLite（WAL）：workflows/versions、executions/events、agents/sessions、skills/versions、mcp servers、approvals、audit、settings。
- 文件：IR YAML/JSON、Skill 目录、插件、产物。
- 产物内容寻址：`artifacts/<sha256>/...`；事件只存引用。
- 迁移：版本化 SQL；启动时备份。
- 加密：凭证/令牌用 OS keychain；数据库敏感字段可选 SQLCipher（Phase 2 评估）。

## 5. 离线行为

| 功能 | 无云 |
| --- | --- |
| Workflow 编辑/校验/执行 | 可用（本地模型/无模型） |
| Skill 加载/脚本/产物 | 可用 |
| Agent | 本地模型可用；云模型不可用；无模型模式仅确定性入口 |
| MCP Client | 本地 stdio/局域网 HTTP 可用 |
| MCP Server | 本地可用 |
| Plugin 安装 | 本地包/离线包可用；市场不可用 |
| 同步 | 跳过；本地队列待联网 |
| 模型 | 本地 provider；否则提示 |

启动不检查云连接；网络失败不阻塞本地功能。

## 6. 同步（可选）

- 方向：本地 → 云（发布/备份）、云 → 本地（拉取团队能力）。
- 认证：OIDC device flow/PAT；出站 HTTPS；mTLS 可选。
- 冲突：
  - 注册表/已发布版本：服务器权威；本地冲突创建分支/草稿。
  - 草稿：本地优先；同步时提示合并/覆盖。
  - 执行事件：只追加；本地执行可上传审计摘要（按策略）。
- 密钥：不同步明文；云端凭证引用在本地解析为本地凭证或拒绝。
- 包签名：插件/Skill 同步包需签名/校验和；拒绝未知来源。
- 离线队列：加密本地队列；联网重试；幂等。

## 7. 本地 MCP

- **Local MCP Server**：stdio 优先；HTTP 绑定 loopback；能力令牌（短期、按工具/租户）；默认只读工具，写操作需本地审批。
- **Local MCP Client**：可连接本地/远程 MCP Server；stdio 启动子进程；HTTP OAuth/PAT；工具 schema 校验。
- **Registry**：本地工具注册、权限、审计；不与云端 Registry 强制同步。
- 与外部客户端：本地 MCP Server 可被 Claude/Copilot/Codex 通过 stdio 或 HTTP 连接；按客户端配置生成说明。

## 8. 安全

- 本地 UI 仅 loopback；DNS rebinding 防护（Origin/Host 校验）；CSRF token。
- 脚本默认：无网络、只读工作区、临时写目录、CPU/内存/时长限制；高风险需容器。
- Agent/Skill 的 instructions 视为不可信；不得提升权限。
- 本地 MCP Server 的 token 绑定工作区/工具；可撤销。
- 自动更新：签名 + 校验和 + 回滚；不静默安装插件。
- 日志脱敏；审计本地保留（可配置）。

## 9. 跨平台

| 平台 | 沙箱 | 备注 |
| --- | --- | --- |
| Linux | Landlock/namespaces/cgroup（可用时） | 最佳隔离；容器可选 |
| macOS | 容器（Docker/Podman）或受限进程 | `sandbox-exec` 已弃用/不可靠；高风险需容器 |
| Windows | Job Objects/AppContainer/容器 | 受限进程 + 明确提示 |

本地运行不强制依赖容器；但“不可信 Skill”默认要求容器或显式风险确认。

## 10. 与云端的关系

- 本地是**完整运行时**，不是云端的瘦客户端。
- 云端可选提供：团队 Registry、市场、审计汇总、备份、远程 Worker、托管模型。
- 同一 IR/版本/策略；执行结果事件模型一致。
- Hybrid：本地 Worker 可作为云端 Execution Plane 的 Worker（出站连接）；本地 UI 仍可独立运行。

## 11. 启动与健康

```bash
hisha init                 # 初始化目录/配置
hisha serve --port 4780    # 本地 API/UI（loopback）
hisha workflow validate f.yaml
hisha workflow run f.yaml --input input.json
hisha skill test ./skills/invoice-audit
hisha mcp serve --stdio
hisha doctor               # 依赖/沙箱/模型/端口检查
```

- `doctor` 检查 Rust/Python/容器/模型/端口/权限；输出可操作建议。
- 健康：`/health/live`、`/health/ready`；UI 显示本地 Runtime 状态。
- 日志：JSON lines；滚动；本地审计独立。

## 12. 非目标

- 不在本地实现多租户 SaaS 计费。
- 不要求本地运行大模型；无模型模式必须支持确定性 Workflow/Skill 脚本。
- 不把本地 Runtime 做成云端代理；离线必须可用。
- 不在 Phase 1 支持所有 OS 沙箱的强隔离；高风险场景用容器。

## 13. 相关文档

- [系统架构](system-architecture.md)
- [Execution Plane](execution-plane.md)
- [Skill 本地执行](../05-skill/local-execution.md)
- [MCP Server](../06-mcp/mcp-server.md)
- [Local Runtime Manager](../09-ui/local-runtime-manager.md)
- [Local Runtime（Rust）](../13-tech-stack/local-runtime-rust.md)
- [ADR-0007](../adr/0007-local-runtime-stack.md)
