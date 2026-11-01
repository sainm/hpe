# Local Runtime 桥接

> 复用既有 Rust Local Runtime；VS Code Extension 不执行任意本地脚本。扩展只调用 Local API/SSE，执行/沙箱/进程/权限/产物全部由 Runtime 负责。

## 1. 连接方式

| 通道 | 用途 | 说明 |
| --- | --- | --- |
| HTTP Local API | Workflow/Skill/Execution/Artifact/Approval | `http://127.0.0.1:8787/api` |
| SSE | 执行/审批/状态事件 | `text/event-stream`；断线重连 |
| MCP Streamable HTTP | Copilot 工具调用 | `http://127.0.0.1:8787/mcp` |
| stdio（可选） | 本地 MCP/CLI | 仅官方 Runtime bridge；不由扩展启动任意脚本 |

- 不使用 FFI/共享内存/直接数据库访问。
- 不引入第二套执行引擎；扩展不调用 `child_process` 执行 Workflow/脚本。
- 本地 API 只绑定 loopback；扩展校验 Origin/Host/端口。

## 2. 发现/健康

```text
GET /health/live
GET /health/ready
GET /api/v1/runtime/info
```

> `/api/v1/runtime/info` 与本地 token 获取是**集成本模块新增的只读/认证端点**；若现有 Local Runtime 未提供，则用 `/health/ready` + 版本头/现有 CLI 获取。它们不改变核心 Runtime 架构/执行模型。

- 信息：runtime version、protocol version、capabilities、platform、data dir、mcp endpoint。
- 扩展启动时异步探测；失败不阻塞 VS Code。
- 版本不兼容：显示最低/当前版本；禁用执行；不自动升级。
- 端口冲突/未启动：状态栏 Offline；提供 Start/诊断/文档。

## 3. 启动/停止

- **不自动下载/静默启动** Runtime。
- 用户命令 `hpa.runtime.start`：
  - 若已运行 → 连接。
  - 若配置了 binary path → 在 VS Code Terminal 中显式启动（用户可见），或提示用户运行 `hpa serve`。
  - 未配置 → 打开安装/文档。
- `hpa.runtime.stop`：仅当扩展启动且用户确认；不杀非扩展启动的进程。
- 更新：提示用户在 Auralis Local Runtime Manager/CLI 更新；扩展不自动替换二进制。
- Output Channel 显示 stdout/stderr 摘要；不打印 token。

## 4. 认证

- 本地：loopback + 短期本地 token。
  - 获取：Local API `POST /api/v1/auth/local-token`（仅 loopback/Origin 校验）或配置的本地 token 文件（0600）。
  - 存储：`SecretStorage`（不写 workspace/settings）。
  - 刷新：到期前刷新；失败提示重新登录/重启 Runtime。
- 远程：PAT/OIDC；SecretStorage；HTTPS；MCP Gateway token。
- 禁止 token 写入 `.vscode/mcp.json`/日志/URL；不使用 token passthrough。

## 5. API 使用

| 功能 | API | 说明 |
| --- | --- | --- |
| Workflow 列表/详情 | `GET /workflows`、`GET /workflows/{id}` | 只读摘要；复杂编辑跳 Web UI |
| 执行 | `POST /workflows/{id}/executions` | Idempotency-Key；返回 executionId |
| 执行状态 | `GET /executions/{id}` | 轮询/SSE |
| 事件 | `GET /executions/{id}/events/stream` | SSE；Last-Event-ID |
| 取消 | `POST /executions/{id}/cancel` | 协作取消 |
| 审批 | `GET /approvals`、`POST /approvals/{id}/decide` | 或在 Web UI 审批；高风险 step-up |
| Skill 安装/校验 | `POST /skills/validate`、`POST /skills/install`（如提供） | 扩展也可写标准目录；校验用 Runtime |
| Artifact | `GET /executions/{id}/artifacts`、`GET /artifacts/{id}` | 大文件下载/引用 |
| MCP 状态 | `GET /mcp`/`GET /runtime/info` | provider 注册/健康 |

- 超时：请求级；执行级由 Runtime 管理；扩展不无限等待。
- 重试：GET/幂等操作可退避；执行/写操作使用 idempotencyKey；不重试未知副作用。
- 取消：AbortController；执行取消单独 API。

## 6. 事件/监控

- SSE 事件：execution.started/node.*/approval.*/artifact.created/finished。
- 扩展合并到 Tree/Webview；大日志/事件分页/截断。
- 断线：指数退避重连；从 Last-Event-ID 继续；状态栏 Degraded。
- 执行完成/失败/审批：通知；可打开 Execution Monitor/Approval Center。
- 产物：内容寻址；扩展用 `vscode.workspace.fs` 打开临时文件或 Webview 预览；不直接读 Runtime 数据目录。

## 7. 本地模型/离线

- Runtime 可在无云/无模型下运行确定性 Workflow/Skill。
- Agent 需要 Python AI Worker；未安装时禁用并提示。
- 扩展不检查/下载模型；只显示 Runtime 能力。
- 离线：本地功能可用；远程 Control Plane/Marketplace 禁用；MCP 本地可用。

## 8. 远程开发/多根

- Remote SSH/Container：扩展运行在工作区端；Local Runtime 必须在该端可访问（`127.0.0.1` 是远端 loopback）。
- 若用户希望使用本机 Runtime，显式配置 remote endpoint/端口转发；不要假设自动可达。
- 多根：Runtime 可能按工作区/项目启动；扩展选择 primary root；执行/Skill 安装目标显式。
- 不在未信任工作区执行 Workflow。

## 9. 错误/恢复

| 错误 | 扩展行为 |
| --- | --- |
| Runtime 未启动 | Offline；Start/文档 |
| 认证失败 | Sign in/刷新 token；不重试风暴 |
| 版本不兼容 | 禁用执行；提示升级 |
| 执行超时 | 显示执行状态/取消；不重复提交 |
| 审批拒绝 | 显示拒绝原因；可重新规划/取消 |
| Runtime 崩溃 | 状态 Offline；重连；执行状态以 Runtime 为准 |
| 端口被占用 | 诊断；提示更改配置/停止冲突进程 |
| 文件过大 | 下载/外部打开；不内联 |

## 10. 安全

- 扩展不执行脚本/不写 Runtime 数据库/不读私钥。
- Local API 只 loopback；token 短期；Origin/Host 校验。
- 工作区信任后才允许执行/安装 Skill。
- 审批在服务端；扩展显示/跳转，不做策略决策。
- 日志/产物脱敏；不把 secret 写入 Webview/localStorage。
- 扩展更新/安装签名；Marketplace/VSIX 供应链。

## 11. 验收

- 启动/发现/停止/版本检查。
- 执行 Workflow → SSE 事件 → 完成/失败 → 产物查看。
- 审批等待 → Web UI 审批 → 恢复。
- 断线/重连/取消/超时。
- 未启动/未认证/版本不兼容/离线提示。
- 扩展不直接执行脚本（代码审查/测试）。

## 12. 相关

- [Extension 架构](extension-architecture.md)
- [MCP 集成](mcp-integration.md)
- [Local Runtime（Rust）](../13-tech-stack/local-runtime-rust.md)
- [MCP Server](../06-mcp/mcp-server.md)
