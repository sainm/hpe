# 用户场景

> 主场景对应需求 §7：VS Code → Copilot → Skill → MCP → Rust Local Runtime → 执行 → 结果。覆盖成功/失败/审批/离线/远程开发。

## S1 主场景：在 Copilot 中通过 Skill 调用 Auralis Workflow

### 前置

- VS Code 已安装 Auralis Extension；GitHub Copilot 可用。
- Auralis Local Runtime 已安装（Rust），本地 API 8787、MCP 8787/mcp。
- 用户已登录/本地 token 可用；工作区已信任。
- Auralis 中已有 `order-sync` Workflow 与对应 Skill `hpa-order-sync`。

### 步骤

1. **打开项目**：用户打开包含 `.github/skills/hpa-order-sync/` 的仓库。
2. **检测 Runtime**：Extension 启动，`GET /health/live`/`/runtime/info`；状态栏显示 Running，注册 MCP provider `hpa-local`。
3. **Copilot 识别任务**：用户在 Copilot Chat 输入“同步今天的订单并汇总失败项”。
4. **加载 Skill**：Copilot 依据 description 选择 `hpa-order-sync`（或用户 `/skills` 手动选择）；Skill 指导“先 `workflow.search`，再 `workflow.execute`，失败用 `workflow.get_execution`/`execution.logs`”。
5. **发现工具**：Copilot 通过 MCP 列出 `workflow.search`/`workflow.execute`/`workflow.get_execution`/`execution.logs`/`artifact.list`。
6. **调用 MCP**：Copilot 调用 `workflow.search {query:"order sync"}` → 得到 Workflow/version；调用 `workflow.execute {workflowId, version, inputs}`。
7. **Runtime 验证**：Rust MCP Server 校验 token/tenant/scope/schema；`workflow.execute` risk=high → 创建 Approval（若策略要求），返回 `executionId` + `WAITING_APPROVAL`；Extension 状态栏/树显示等待。
8. **审批**：用户在 Auralis Approval Center（Web UI 深链）批准；Runtime 恢复执行。
9. **执行**：Rust Runtime 在本地/Worker 执行节点；外部系统调用/脚本在沙箱；事件写入 Execution；产物内容寻址。
10. **结果回传**：Copilot 用 `workflow.get_execution`/`execution.get` 读状态/摘要；大结果用 `artifact.list`/`artifact.get`；Copilot 汇总给用户。
11. **监控**：Extension Executions/Artifacts 显示；可打开 Web UI；审计包含 client/tool/approval/execution。

### 成功标准

- Copilot 能发现/调用基线 MCP Tools。
- Skill 的指导被遵循（至少一次 workflow.search/execute）。
- 高风险调用经过服务端审批；客户端无法绕过。
- 执行结果/产物可从 Extension 和 Web UI 查看。
- 无脚本由 Extension 执行；全部由 Runtime。

## S2 安装/更新 Skill

1. 用户在 Auralis Skill Center 搜索 `hpa-order-sync`。
2. Extension 显示版本/许可/信任/权限/脚本。
3. 选择目标：项目 `.github/skills`（默认）或个人 `~/.copilot/skills`。
4. 调用 Runtime Skill Service 校验：frontmatter/name/目录/路径/脚本/许可。
5. 安装（原子写入）；sidecar 记录版本/hash/来源；刷新树。
6. 用户重启 Copilot Chat/新会话，Copilot 发现 Skill。
7. 更新：显示 changelog/权限差异；本地修改 dirty 时需确认；保留备份/回滚。
8. 移除：只删除 Auralis 安装文件；用户修改保留；依赖检查。

失败：

- 目录名与 `name` 不一致 → 拒绝并给修复。
- 路径穿越/符号链接 → 拒绝。
- 未信任工作区 → 禁止写入。
- 许可/来源不可信 → 警告/需显式确认。

## S3 本地/离线 + 无云

1. 断网；Extension 显示 Offline（远程不可用）；Local Runtime 仍 Running。
2. Copilot 加载本地 Skill；调用本地 MCP Server。
3. Runtime 执行确定性 Workflow/Skill；无模型也可（无 AI 模式）。
4. 产物/日志本地保存；Extension 可查看。
5. 网络恢复：同步/市场可用；不自动上传执行数据（按策略）。

失败：

- Runtime 未启动 → Offline；Start/文档。
- Python AI Worker 未安装 → Agent 禁用；Workflow/Skill 仍可用。
- 模型不可用 → LLM/Agent 节点失败；确定性节点继续/按错误策略。

## S4 远程开发/多根工作区

1. VS Code Remote SSH 打开远端仓库；Extension 运行在工作区端。
2. Local Runtime 必须位于远端 loopback；Extension 检测不到本机 Runtime 时提示“Runtime 位置”。
3. 用户选择远端 Runtime；Skill 安装到远端 `.github/skills`。
4. 多根：选择 primary root；Skill/执行目标显式；不跨根静默写入。
5. 若需本机 Runtime，配置端口转发/远程 endpoint；不假设自动可达。

失败：

- 远端无 Runtime → 指引安装/配置；不自动下载。
- 端口冲突/防火墙 → Doctor/诊断。
- 工作区未信任 → 只读。

## S5 审批拒绝/超时

1. Copilot 调用 `workflow.execute`（高风险）。
2. Approval 创建；Extension 显示 Waiting；用户打开 Approval Center。
3. 拒绝/超时 → Runtime 按策略失败/跳过；返回原因。
4. Copilot 可重规划/结束；Extension 显示拒绝/超时；审计记录。
5. 不允许 Copilot/客户端自行批准；不允许 Custom Instructions 绕过。

## S6 MCP/版本不兼容

1. VS Code 版本不支持 MCP provider/chatSkills → Extension 检测 API/版本。
2. 提示升级 VS Code/Copilot；只读功能可用；执行/注册禁用。
3. 若用户要求 `.vscode/mcp.json` 兼容：使用 `servers`，确认/备份；不使用 `mcpServers`。
4. Copilot CLI 配置另写 `mcp-config.json` `mcpServers`；不复制 VS Code 文件。

## 验收证据

- Extension 日志/命令执行记录。
- MCP provider 注册/工具列表截图或日志。
- Skill 安装路径/文件树。
- Execution/Approval/Artifact 记录。
- 客户端调用审计（tenant/tool/approval/trace）。
- 断网/未启动/未信任/版本不兼容的负向测试。

## 相关

- [Extension 架构](extension-architecture.md)
- [Copilot 集成](copilot-integration.md)
- [MCP 集成](mcp-integration.md)
- [Local Runtime 桥接](local-runtime-bridge.md)
