# 测试、发布与兼容

## 1. 版本/工具

| 项 | 版本（2026-11-01） | License |
| --- | --- | --- |
| `@types/vscode` | 1.140.0 | MIT |
| `@vscode/vsce` | 4.1.0 | MIT |
| `@vscode/test-cli` | 0.0.15 | MIT |
| `@vscode/test-electron` | 3.1.0 | MIT |
| esbuild | 0.28.2 | MIT |
| `@vscode-elements/elements` | 2.5.1 | MIT |
| TypeScript | 5.9.3 | Apache-2.0 |
| Zod | 4.6.7 | MIT |

`engines.vscode` 以实际使用的 MCP provider/Custom Agents API 最低版本为准；本实现使用 `^1.143.0`，在 VS Code 1.143.1 实测，类型固定为 1.140.0 以限制可用 API。

## 2. 测试层级

| 层级 | 内容 | 工具 |
| --- | --- | --- |
| Unit | 配置/错误映射/Skill 路径/消息校验 | Vitest |
| Extension Host | 激活、Tree、命令、SecretStorage、信任 | `@vscode/test-cli`/`test-electron` |
| Contract | Local API/SSE/MCP provider 消息 | Mock Runtime + schema |
| Integration | 真实 Local Runtime/MCP/Skill 安装 | VS Code + Rust binary |
| E2E | Copilot 加载 Skill → MCP → 执行 | 手动/真实 VS Code（PoC） |
| Security | 路径/符号链接/未信任/token/审批 | 专用用例 |
| Compatibility | VS Code/Copilot 版本/路径/配置 | 版本矩阵 |
| Packaging | VSIX 安装/升级/卸载 | `vsce` |

## 3. 关键用例

### Extension

- 激活不阻塞；Runtime 未启动时 Offline。
- Tree 10 个 View 空/加载/错误/只读。
- 状态栏/命令/when 条件。
- 多根/Remote/未信任。
- 三语/高对比/键盘。
- Webview CSP/消息校验/大结果。

### MCP Provider

- `provide` 无用户交互；返回 HTTP definition。
- `resolve` 获取/刷新 token；失败时提示。
- Runtime 状态变化触发 `onDidChange`。
- 工具列表/schema；高风险审批等待。
- 旧版 VS Code 无 provider 时的兼容路径（`servers`）不写 token。

### Skill

- 安装到 `.github/skills`/个人；目录名匹配。
- 更新/移除/回滚；本地修改保护。
- 路径穿越/符号链接/未信任拒绝。
- `chatSkills` 内置技能可加载；同名冲突。
- 不执行脚本。

### Local Runtime

- 发现/健康/版本不兼容。
- Workflow 执行/SSE/取消/审批/产物。
- 断线/重连/离线。
- 不同平台（Win/macOS/Linux）。

### 安全

- 未信任工作区不能写/执行。
- Token 不入 settings/日志/Webview。
- 高风险工具必须服务端审批；拒绝/超时。
- 自定义 Instructions/Agents 不能绕过。
- 扩展无任意脚本执行（代码审查 + 运行时检查）。

## 4. 测试命令（建议）

```bash
cd apps/vscode-extension
npm install
npm run build
npm test
npm run test:host
npm run package
```

- `test:host` 使用 `/tmp` 下的临时 user-data/extensions 目录与本机 VS Code；不写用户 profile。
- 本机 macOS 可执行文件为 `Contents/MacOS/Code`；Linux CI 使用 `xvfb-run`。
- 不上传测试 secret；使用 mock/fixture。

## 5. CI

- OS matrix：Windows/macOS/Linux；VS Code stable（必要时 insiders）。
- 单元 + Extension Host + 集成；VSIX 构建 artifact。
- 许可证/SBOM/依赖扫描；VSIX 内容检查（不包含 secret/测试文件）。
- 三语/打包 smoke；版本/兼容矩阵。
- 发布需人工批准；不自动发布 Marketplace。

## 6. 发布

1. 版本 bump + CHANGELOG（扩展独立 semver）。
2. 全部门禁绿；VSIX 构建/签名。
3. Marketplace（公开）或私有 VSIX 分发（企业）。
4. 发布说明：VS Code/Copilot 最低版本、已知限制、配置示例。
5. 回滚：保留上一版 VSIX；Marketplace 版本不可删除，发布修复版。
6. 兼容矩阵更新：VS Code/Copilot/Local Runtime/协议版本。

产物：

- `.vsix`（含 `dist/`、`skills/`、`media/`、`package.nls.*`）。
- checksums/SBOM；发布说明。
- 兼容性测试记录。

## 7. 兼容策略

- 不假设所有 VS Code/Copilot 版本支持 `chatSkills`/MCP provider/Custom Agents。
- 启动时检测 API/版本；不足时禁用对应功能并提示升级。
- 旧版兼容路径（`.vscode/mcp.json` `servers`）需用户确认/备份。
- Copilot CLI/Codex 不共享 VS Code 配置；导出/文档分开。
- Local Runtime/协议版本不兼容 → 禁用执行；不自动升级。

## 8. 验收

- VSIX 安装/升级/卸载；激活无错误。
- 10 个 View/命令/状态栏/设置。
- MCP provider/Copilot 工具调用成功。
- Skill 安装/更新/移除/校验。
- 高风险审批/拒绝/超时。
- 未信任/离线/版本不兼容的负向测试。
- 三语/高对比/键盘/远程工作区。

## 9. 相关

- [Extension 架构](extension-architecture.md)
- [Skill 集成](skill-integration.md)
- [MCP 集成](mcp-integration.md)
- [Local Runtime 桥接](local-runtime-bridge.md)
- [官方证据](evidence.md)
