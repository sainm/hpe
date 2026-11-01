# VS Code Extension 架构

> 语言：TypeScript；运行在 VS Code Extension Host。扩展是 **薄客户端**：UI + 认证 + MCP provider + Skill 安装；执行全部交给 Auralis Local Runtime/MCP Server。

## 1. 工具链

| 项 | 选择 | 版本（2026-11-01） | License | 说明 |
| --- | --- | --- | --- | --- |
| VS Code API types | `@types/vscode` | 1.140.0 | MIT | MCP provider/TreeView/SecretStorage |
| 打包 | `@vscode/vsce` | 4.1.0 | MIT | `.vsix` |
| 测试 | `@vscode/test-cli` / `@vscode/test-electron` | 0.0.15 / 3.1.0 | MIT | Extension Host 测试 |
| 构建 | esbuild | 0.28.2 | MIT | 单 bundle；`vscode` external |
| Webview UI | `@vscode-elements/elements` | 2.5.1 | MIT | 或原生 HTML/CSS；不使用已过时的 toolkit |
| 语言 | TypeScript | 5.9.3 | Apache-2.0 | 与平台一致 |
| 校验 | Zod | 4.6.7 | MIT | 配置/消息校验 |

`engines.vscode` 以实际使用的 MCP provider/Custom Agents API 最低版本为准；本实现为 `^1.143.0`，在 1.143.1 实测；类型 1.140.0。

## 2. 目录

```text
apps/vscode-extension/
├── package.json
├── package.nls.json
├── package.nls.ja.json
├── package.nls.zh-cn.json
├── l10n/
├── src/
│   ├── extension.ts
│   ├── container.ts
│   ├── config.ts
│   ├── auth/
│   ├── runtime/
│   │   ├── discovery.ts
│   │   ├── client.ts
│   │   └── events.ts
│   ├── copilot/
│   │   ├── mcpProvider.ts
│   │   ├── skills.ts
│   │   ├── instructions.ts
│   │   └── agents.ts
│   ├── views/
│   │   ├── overview.ts
│   │   ├── workflows.ts
│   │   ├── agents.ts
│   │   ├── skills.ts
│   │   ├── mcp.ts
│   │   ├── plugins.ts
│   │   ├── runtime.ts
│   │   ├── executions.ts
│   │   ├── artifacts.ts
│   │   └── settings.ts
│   ├── webviews/
│   ├── services/
│   └── i18n/
├── skills/                 # 可选：chatSkills 只读技能
├── media/
└── test/
```

- 复用 `packages/ts/api-client`（OpenAPI 生成）与 `packages/ts/sdk`；不手写重复 DTO。
- 扩展不依赖 Rust crate；只通过 Local API/HTTP MCP/Control Plane API 通信。
- 扩展不直接读写本地 Runtime 数据库/凭证文件。

## 3. package.json 贡献

```json
{
  "name": "workbench",
  "displayName": "Auralis",
  "publisher": "hpa",
  "engines": { "vscode": "^1.143.0" },
  "activationEvents": ["onStartupFinished"],
  "main": "./dist/extension.js",
  "contributes": {
    "viewsContainers": {
      "activitybar": [{ "id": "hpa", "title": "Auralis", "icon": "media/hpa.svg" }]
    },
    "views": {
      "hpa": [
        { "id": "hpa.overview", "name": "Overview" },
        { "id": "hpa.workflows", "name": "Workflows" },
        { "id": "hpa.agents", "name": "Agents" },
        { "id": "hpa.skills", "name": "Skills" },
        { "id": "hpa.mcp", "name": "MCP Servers" },
        { "id": "hpa.plugins", "name": "Plugins" },
        { "id": "hpa.runtime", "name": "Local Runtime" },
        { "id": "hpa.executions", "name": "Executions" },
        { "id": "hpa.artifacts", "name": "Artifacts" },
        { "id": "hpa.settings", "name": "Settings" }
      ]
    },
    "commands": [
      { "command": "hpa.runtime.start", "title": "Auralis: Start Local Runtime" },
      { "command": "hpa.runtime.status", "title": "Auralis: Runtime Status" },
      { "command": "hpa.workflow.run", "title": "Auralis: Run Workflow" },
      { "command": "hpa.skill.install", "title": "Auralis: Install Skill" },
      { "command": "hpa.mcp.register", "title": "Auralis: Register MCP Server" },
      { "command": "hpa.execution.open", "title": "Auralis: Open Execution" }
    ],
    "configuration": { "title": "Auralis", "properties": {} },
    "mcpServerDefinitionProviders": [
      { "id": "hpa-local", "label": "Auralis Local MCP" }
    ],
    "chatSkills": [
      { "path": "./skills/hpa-workflow/SKILL.md" }
    ]
  }
}
```

- `chatSkills` 是可选只读技能；路径必须指向 SKILL.md，目录名与 `name` 一致。
- `mcpServerDefinitionProviders` id 必须与 `vscode.lm.registerMcpServerDefinitionProvider('hpa-local', ...)` 一致。
- 只注册一个本地 provider；远程通过同一 provider 的配置切换，避免重复服务器。

## 4. 激活与生命周期

1. `activate()`：读取配置 → 初始化 SecretStorage → 发现 Local Runtime → 注册 Tree/Webview/命令/状态栏 → 注册 MCP provider →（可选）注册 chatSkills 已由 package 贡献。
2. `onStartupFinished` 后异步探测，不阻塞 VS Code 启动。
3. Runtime 上线/下线/配置变化 → 触发 `onDidChangeMcpServerDefinitions`、刷新 Tree/Status。
4. `deactivate()`：关闭 SSE/AbortController、dispose 注册；不停止用户 Runtime（除非用户明确要求扩展启动/停止）。

## 5. Runtime 发现与客户端

- 实际 Local API：`http://127.0.0.1:8787/api/*`；MCP HTTP：`http://127.0.0.1:8787/mcp`。
- 发现：`GET /api/health`，校验 `internalId=hpa`；没有 `/api/v1/runtime/info`，扩展不新增核心端点。
- VS Code 工具名限制通过请求头 `X-HPA-Tool-Name-Style: slug` 适配，详见 [本地实现](local-implementation.md)。
- 认证：
  - 本地：loopback + 短期本地 token（通过 Local API/CLI 获取；不写入 workspace）。
  - 远程：PAT/OIDC；存 SecretStorage；`hpa.auth.signIn` 命令。
- API 客户端：`packages/ts/api-client`；错误映射到 VS Code 通知/树节点。
- 事件：`fetch` + SSE（`text/event-stream`）；`AbortController` 取消；断线重连/退避。
- 超时/重试：幂等 GET 可重试；执行/写操作使用 idempotencyKey；不重试未知副作用。
- 多根工作区：选择 primary root；Skill 安装目标显式；不跨根静默写入。

## 6. UI 实现

- **Tree View**：每类资源一个 `TreeDataProvider`；`contextValue` 驱动菜单（运行/打开/更新/移除）。
- **Webview**：Overview、Execution Detail、Artifact Preview；CSP、`asWebviewUri`、`retainContextWhenHidden` 谨慎；消息 Zod 校验。
- **Status Bar**：Runtime 状态（Running/Degraded/Offline）、当前 workspace/project；点击打开 Overview。
- **Command Palette**：所有主动作有命令；`when` 条件按 Runtime/信任/权限。
- **Output Channel**：`Auralis Runtime`/`Auralis MCP` 日志；不输出 token。
- **Notifications**：审批等待/执行失败/Runtime 断线；提供打开 Web UI/重试动作。
- **无完整 Designer**：Workflow 编辑通过 `openExternal` 打开 Auralis Web UI 深链。

## 7. 认证/配置

设置（示例）：

```json
{
  "hpa.mode": "auto",
  "hpa.local.endpoint": "http://127.0.0.1:8787",
  "hpa.local.mcpEndpoint": "http://127.0.0.1:8787/mcp",
  "hpa.remote.endpoint": "",
  "hpa.remote.mcpEndpoint": "",
  "hpa.skills.installTarget": "project",
  "hpa.copilot.instructions": true,
  "hpa.copilot.customAgents": false,
  "hpa.telemetry.enabled": false
}
```

- Token 只存 `SecretStorage`；不写 settings/workspace `.vscode`。
- 远程必须 HTTPS；本地只允许 loopback。
- `hpa.mode`: `auto|local|remote`；auto 先本地后远程（按配置）。
- 不自动下载/启动 Runtime；启动需用户命令/终端可见。

## 8. 远程开发/工作区信任

- `extensionKind: ["workspace"]`：扩展运行在 Remote SSH/Container 的工作区端；Local Runtime 必须位于该环境。
- 可选 `ui` 模式：在本地扩展宿主连接本机 Runtime；需显式配置，避免“在远端工作区调用本机文件”的混淆。
- 未信任工作区：禁止写 Skill/Instructions/Agents、禁止执行 Workflow；只允许查看/诊断。
- 多根工作区：Skill 安装默认主根；用户可切换；显示目标路径。

## 9. i18n

- VS Code 原生本地化：`package.nls.json`、`package.nls.ja.json`、`package.nls.zh-cn.json`；运行时 `vscode.l10n.t`。
- Webview 使用传入的 locale/messages；不加载远程翻译。
- 默认 `ja`（与平台一致），支持 `en`/`zh-cn`；数字/日期用 `Intl`。
- 不硬编码用户可见文案；命令标题/设置描述/通知/错误均本地化。

## 10. 错误/离线

| 状态 | 行为 |
| --- | --- |
| Runtime 未运行 | 状态栏 Offline；提供 Start/文档；不阻塞编辑器 |
| 未登录 | 提示 Sign in；MCP provider resolve 时认证 |
| Runtime 版本不兼容 | 显示要求版本；禁用执行；不自动升级 |
| MCP 不可用 | 工具列表为空/错误；提示重试；不影响 Workflow 树 |
| 执行失败 | Execution 树错误节点；打开日志/Web UI |
| 审批等待 | 状态 Waiting；打开 Approval Center；可取消 |
| 断网 | 本地可用；远程禁用；队列/重试提示 |
| 工作区未信任 | 只读；禁止写入/执行 |

## 11. 测试

- 单元：配置/错误映射/Skill 路径/指令生成/MCP provider 消息。
- Extension Host：`@vscode/test-cli` + `test-electron`；Tree/命令/SecretStorage/信任。
- 集成：Mock Local Runtime（health/SSE/MCP）；契约测试。
- E2E：真实 VS Code + Local Runtime + Copilot 手动验收（PoC）。
- 打包：`vsce package`；`.vsix` 安装/升级/卸载。
- 三语/高对比/键盘/远程工作区。

## 12. 相关

- [Copilot 集成](copilot-integration.md)
- [Skill 集成](skill-integration.md)
- [MCP 集成](mcp-integration.md)
- [Local Runtime 桥接](local-runtime-bridge.md)
- [Extension UI](extension-ui.md)
- [测试与发布](testing-release.md)
