# Local Runtime（Rust）

> 目标：一个签名的 Rust 单二进制，在无云、无 NestJS、无外部数据库的情况下运行 Workflow、Skill、MCP 与本地插件；Web UI 作为静态资源嵌入。Python/Node 仅在需要时作为可选脚本/Agent 运行时。

## 1. 形态

```text
hisha（单二进制）
├── CLI（clap）
├── Local API（Axum, 127.0.0.1）
├── Web UI（rust-embed 静态资源）
├── Core Runtime（hpa-runtime）
├── IR/Compiler/Validator
├── Skill Runtime
├── MCP Client/Server（RMCP）
├── Plugin Host（Wasmtime / process / external）
├── Process/Sandbox/Policy
├── SQLite（SQLx, WAL）
├── Artifacts/Logs/Backups
└── Optional: Python AI Worker / Node script runtime / local model
```

- 默认 `hisha serve` 启动本地 API/UI；浏览器访问 `http://127.0.0.1:4780`。
- 无 Electron/Tauri 依赖；后续可用 Tauri 包装，不改变运行时。
- 单用户/本地工作区；不实现 SaaS 多租户/计费/市场托管。

## 2. 启动流程

1. 解析配置/环境变量；确定数据目录（`directories` crate）。
2. 打开 SQLite（WAL）；运行迁移；备份（升级前）。
3. 初始化 keychain；加载本地凭证引用。
4. 加载本地 Registry（Workflow/Skill/Agent/MCP/Plugin）。
5. 启动 Axum（loopback）；嵌入 UI；生成/读取本地会话 token。
6. 启动可选 Python AI Worker（检测 3.13/依赖）；失败则禁用 Agent。
7. 启动 MCP 本地 Server（stdio/HTTP）如启用。
8. 健康检查/doctor；输出可操作状态。

## 3. 目录

```text
~/.hpa/ 或平台数据目录
├── config.toml
├── runtime.db                 # SQLite
├── runtime.db-wal
├── skills/<name>/<version>/
├── workflows/<id>/<version>/
├── agents/<id>/<version>/
├── mcp/<server>.json
├── plugins/<id>/<version>/
├── artifacts/<sha256>/
├── logs/
├── backups/
└── updates/
```

- 权限 0700；凭证不落文件，使用 OS keychain（`keyring`）。
- 数据目录跨平台：Windows `%LOCALAPPDATA%`、macOS `~/Library/Application Support`、Linux `~/.local/share`。
- 备份：`hisha backup` 加密包；恢复需主密钥/密码。

## 4. 本地控制面（子集）

| 能力 | 本地实现 | 云端 Control Plane |
| --- | --- | --- |
| Workflow/版本/发布 | Rust + SQLite | NestJS + Postgres |
| 执行/事件/审批 | Rust | Rust + NestJS 查询 |
| Skill/Agent/MCP/Plugin | Rust Registry | NestJS |
| 用户/租户 | 本地用户 + 会话 token | OIDC/多租户/RBAC |
| 计费/配额/市场 | 不做 | Phase 6 |
| 审计 | 本地 JSONL/SQLite | 追加式/外部 SIEM |

- 本地 API 是 OpenAPI 子集；与云端契约一致，功能按矩阵裁剪。
- 本地不跑 NestJS；避免 Node 运行时依赖。
- 契约测试：同一 OpenAPI/IR fixture 在本地/云端结果一致。

## 5. 本地 API

- Base：`http://127.0.0.1:4780/api/v1`；仅 loopback；`Origin`/`Host` 校验；本地 token/CSRF。
- 端点：workflows/versions/validate/run/events、executions、approvals、skills、agents（可选）、mcp、plugins、settings、doctor。
- SSE：执行事件；断线重连 `Last-Event-ID`。
- UI：静态资源；CSP；无 CDN；离线可用。
- 不暴露公网；如需局域网访问需显式配置 + TLS/认证（后续）。

## 6. 运行与沙箱

- Workflow Runtime 与 Worker 同核（`hpa-runtime`）；LocalRunner 使用 SQLite 检查点/事件。
- Skill Runtime：加载/解析/脚本；Python/Node/Shell 通过 `hpa-script` 子进程；WASM 通过 Wasmtime。
- MCP：RMCP stdio/HTTP；本地 Server 能力令牌；外部 Server 连接。
- 插件：manifest/签名/安装；WASM 优先；高风险进程用容器。
- 沙箱：Linux Landlock/cgroups/seccomp；macOS/Windows 容器或受限进程；UI 显示风险。
- 端口：本地 API 4780；MCP HTTP 可选 4781；均 loopback。

## 7. 模型与 AI

- 本地模型：Ollama/llama.cpp/OpenAI 兼容端点；用户配置；不自动下载。
- 云模型：LiteLLM/Provider；keychain；预算/限流。
- 无模型：确定性 Workflow/Skill 正常；Agent 禁用。
- Python AI Worker：可选 sidecar/进程；`uv` 环境；gRPC；未安装则禁用。
- 隐私：本地模型不出网；云模型提示数据边界；Skill 脚本网络策略独立。

## 8. 同步（可选）

- 出站 HTTPS；OIDC device flow/PAT；mTLS 可选。
- 同步：Registry/已发布版本/市场；草稿默认不同步；执行事件按策略。
- 冲突：本地草稿优先；已发布版本服务器权威；提示合并/另存。
- 包签名/校验和；未知来源不自动启用。
- 密钥不同步；本地凭证引用无法解析时拒绝/提示。
- 离线队列：加密；联网重试；幂等。

## 9. 更新/打包

- `cargo-dist` 0.32.0 生成归档/安装器；Windows/macOS/Linux native runner。
- macOS：universal binary、签名/公证；Windows：签名安装器；Linux：tar.gz/deb/rpm。
- UI 静态资源 `rust-embed`；二进制校验/签名；更新清单签名；回滚保留上一版本。
- 不自更新未签名包；用户确认；离线安装包。

## 10. CLI

```bash
hisha init
hisha serve --port 4780
hisha doctor
hisha workflow validate ./order-sync.yaml
hisha workflow run ./order-sync.yaml --input input.json
hisha skill import ./skills/invoice-audit
hisha skill test invoice-audit --case smoke
hisha mcp serve --stdio
hisha plugin install ./plugin.hplugin
hisha sync status
hisha backup --out backup.hisha
```

- 所有命令支持 `--json`（自动化/CI）。
- 错误码与 API 一致；退出码稳定。
- 不自动安装 Python/Node/模型；`doctor` 给出安装建议。

## 11. 安全

- 本地 API 只绑定 loopback；token；CSRF；CSP；Origin/Host 校验。
- keychain/加密；日志脱敏；审计 JSONL/SQLite。
- 脚本默认无网络/只读输入/临时写；高风险容器。
- 本地 MCP 能力令牌短期/可撤销；工具有权限/审批。
- 更新签名/校验；插件签名/审核；不允许静默升级。

## 12. 验收

- 无网络机器：启动 → 创建/编辑 Workflow → 校验 → 运行 → 产物/日志。
- 无云/无 NestJS：Skill 导入/测试/执行/导出。
- 本地 MCP：外部客户端通过 stdio/HTTP 调用本地工具。
- Python 缺失：Agent 禁用，Workflow/Skill 仍可用。
- 崩溃恢复：杀进程后从检查点继续（标准运行时）。
- 三平台安装/启动/更新/回滚。
- 607px Web UI 可用；i18n ja/en/zh。

## 13. 限制

- 本地不提供多租户/SaaS/市场托管/计费。
- macOS/Windows 强沙箱需容器；无容器时只能受限进程。
- Python/Node/Java 脚本需用户/镜像提供运行时。
- 单用户/本地工作区；团队协作需同步/云端。
- Local 与 Cloud 的控制面功能子集不同，需契约/功能矩阵测试。

## 14. 相关文档

- [语言策略](language-strategy.md)
- [Rust 技术栈](rust-stack.md)
- [Script/Plugin 执行](script-plugin-execution.md)
- [跨平台](cross-platform.md)
- [Local Runtime（架构）](../02-architecture/local-runtime.md)
- [Local Runtime Manager](../09-ui/local-runtime-manager.md)
