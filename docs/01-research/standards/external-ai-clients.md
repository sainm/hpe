# 外部 AI 客户端兼容性研究

> 目标：让 Hisha 的 Skill/MCP 能力被 Claude、GitHub Copilot、OpenAI Codex 等真实客户端使用。不能假设一个配置文件适配所有客户端。
> 核查日期：2026-11-01。来源：各客户端官方文档页面 + Codex 官方源码。OpenAI 文档站从本机返回 403，Codex 结论以官方仓库源码/文档为准。

## 1. 兼容性矩阵

| 客户端 | MCP 配置 | Skill 加载 | 传输 | 认证 | 关键差异 |
| --- | --- | --- | --- | --- | --- |
| Claude Code | `.mcp.json`（项目/插件）；`mcpServers` | `.claude/skills/<name>/SKILL.md` | stdio / HTTP | OAuth/环境变量（按服务器） | frontmatter 支持 `allowed-tools`、`disallowed-tools` 等客户端扩展；插件市场 |
| VS Code + Copilot | 工作区 `.vscode/mcp.json` 使用 `servers`；可移植 `.mcp.json` 使用 `mcpServers`；扩展可用 `contributes.mcpServerDefinitionProviders` + `vscode.lm.registerMcpServerDefinitionProvider` | 项目 `.github/skills/`、`.claude/skills/`、`.agents/skills/`；个人 `~/.copilot/skills/` 等；扩展 `chatSkills` 贡献 | stdio / HTTP | 由服务器/客户端配置；支持信任提示 | **`servers` 与 `mcpServers` 不能混用**；工具审批/工作区信任 |
| GitHub Copilot CLI | `mcp-config.json`（`mcpServers`） | 与 VS Code 不同，需单独验证 | stdio / HTTP | 按 CLI 配置 | 与 VS Code 配置键不同；不能复制粘贴 |
| OpenAI Codex | `config.toml` 的 `[mcp_servers.<name>]` | `.agents/skills/`、`$CODEX_HOME/skills/`、插件根 `skills/` | stdio / Streamable HTTP（rmcp） | OAuth/环境/企业身份（源码含 OAuth 发现） | 客户端用 Rust rmcp；支持 streamable HTTP |
| Claude Desktop/Web | 连接器/远程 MCP | Claude Skills（按产品） | HTTP/stdio（按产品） | OAuth/连接器 | 面向终端用户，非开发者配置 |
| 自定义 Agent Runtime | 自定义 | 标准 Skill 目录 | stdio/HTTP | 自定义 | 需按 Hisha 契约实现 |

> 结论：至少存在 **三种 MCP 配置形状**（`servers`、`mcpServers`、`[mcp_servers]`）和 **至少三种 Skill 路径**。Hisha 的导出/集成必须按客户端生成适配包。
>
> 2026-11-01 官方补充：VS Code 支持 `.github/skills`、`.claude/skills`、`.agents/skills` 与个人 `~/.copilot/skills` 等；扩展可通过 `chatSkills` 贡献技能；MCP 可通过 extension provider 注册。具体版本/功能以 VS Code/Copilot 当前文档与功能矩阵为准。详见 [VS Code/Copilot 模块](../../14-vscode-copilot/README.md)。

## 2. 配置样例

### VS Code（工作区）

```json
{
  "servers": {
    "hisha": {
      "type": "http",
      "url": "https://hisha.example.com/mcp",
      "headers": { "Authorization": "Bearer ${input:hisha-token}" }
    }
  }
}
```

### Claude Code（项目）

```json
{
  "mcpServers": {
    "hisha": {
      "type": "http",
      "url": "https://hisha.example.com/mcp",
      "headers": { "Authorization": "Bearer ${HISHA_TOKEN}" }
    }
  }
}
```

### Codex（config.toml）

```toml
[mcp_servers.hisha]
url = "https://hisha.example.com/mcp"
bearer_token_env_var = "HISHA_TOKEN"
```

> 以上是形态示意；字段名/认证方式必须以对应客户端当前官方文档为准。特别是 Codex 的配置键已在源码 `codex-rs/core/src/config/edit.rs` 中确认为 `mcp_servers`，但具体 HTTP/OAuth 字段需按版本验证。

## 3. Skill 导出

| 客户端 | 目标路径 | 备注 |
| --- | --- | --- |
| Claude Code | `.claude/skills/<name>/` | 标准 SKILL.md；可使用 `allowed-tools`/`disallowed-tools` |
| Codex | `.agents/skills/<name>/` 或 `$CODEX_HOME/skills/<name>/` | 源码 `codex-rs/skills/src/selection.rs` 可见；插件根 `skills/` |
| VS Code/Copilot | 按当前官方文档 | 不能假设与 Claude Code 相同 |
| Hisha Local Runtime | `~/.hisha/skills/<name>/` | 标准目录 + `hisha.skill.json` |

导出器必须：

1. 保留标准字段与文件；
2. 不写入客户端不认识的私有字段到 `SKILL.md`；
3. 生成每个客户端的安装说明/路径；
4. 附带许可/NOTICE（仅导出 Apache-2.0 等允许的 Skill）；
5. 对专有/无许可 Skill 拒绝导出。

## 4. MCP 版本与认证

- 客户端可能只支持 2025-11-25 或更早；Hisha Gateway 必须双模式（见 [MCP 2026-07-28 研究](mcp-2026-07-28.md)）。
- 远程 HTTP 客户端可能支持 OAuth；也可能只支持静态 Bearer/环境变量。Hisha 需同时提供：
  - OAuth 2.1（RFC 9728/8414/CIMD/DCR）用于支持的用户；
  - 受控的 Personal Access Token（PAT）/服务账号 token 用于不支持 OAuth 的 CLI；
  - stdio 本地模式用环境变量注入短期凭证。
- 禁止 token passthrough：Hisha 用客户端 token 识别调用者，再用自己的上游凭证调用外部系统。

## 5. 兼容性测试清单（PoC B）

对每个客户端：

1. 能发现 Hisha MCP Server（`server/discover` 或 initialize）。
2. 能列出工具，且 schema 正确。
3. 能调用一个只读工具（如 `workflow.list`）。
4. 能调用一个写工具（如 `skill.execute`），并触发 Hisha 审批。
5. 能加载 Hisha 导出的 Skill，并按 skill 指导调用 MCP 工具。
6. 认证过期/权限不足/租户不匹配时得到可理解的错误。
7. 审计日志包含 client、tenant、tool、参数摘要、结果、审批。
8. 断线/超时/取消行为符合客户端预期。
9. 旧协议客户端仍可工作（至少 2025-11-25）。
10. 不依赖一个客户端专属配置。

## 6. 风险

- 客户端快速迭代，配置/能力会变；必须版本化适配器与回归测试。
- VS Code/Copilot 的 `servers` 与可移植 `mcpServers` 差异是常见踩坑点。
- Codex 的 Rust rmcp 与官方 TS SDK 行为可能不同；需要真实客户端测试。
- Claude 的 `allowed-tools`/`disallowed-tools` 属于客户端扩展，不能当作跨客户端标准。
- 远程 MCP 的 OAuth 发现/CIMD 在企业 IdP 上兼容性不一。
- 外部客户端可能绕过 Hisha UI 的审批提示；Hisha 必须在服务端强制审批，不能依赖客户端。

## 7. 结论

- 对外提供 **一个 MCP 端点 + 多客户端适配说明**，不是一个“万能配置文件”。
- Skill 以标准目录导出，按客户端路径/字段生成适配。
- 外部客户端是 Hisha 的“分发面”，不是权限边界；所有授权/审批/审计在 Hisha 服务端执行。
