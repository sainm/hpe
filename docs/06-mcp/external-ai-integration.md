# 外部 AI 集成

> 目标：Claude、GitHub Copilot、OpenAI Codex、企业 Agent 能通过 MCP 使用 Hisha 能力，并能加载 Hisha 导出的标准 Skill。一个配置文件不可能适配所有客户端。

## 1. 交付物

| 交付物 | 内容 |
| --- | --- |
| MCP 端点 | 远端 HTTPS `/mcp`；本地 stdio/HTTP |
| 认证指南 | OAuth 2.1/PAT/stdio 环境变量；scope 说明 |
| 客户端配置 | Claude Code、VS Code/Copilot、Copilot CLI、Codex 各自样例 |
| Skill 导出 | 标准目录 + 客户端安装路径说明 |
| 兼容性测试 | 每个客户端的发现/调用/审批/Skill 加载 |
| 故障排查 | 协议版本/认证/网络/工具权限 |

## 2. MCP 端点

- 远端：`https://hpa.example.com/mcp`（token 识别租户/工作区）。
- 本地：`http://127.0.0.1:4780/mcp` 或 stdio。
- 协议：2026-07-28 默认；旧版兼容（2025-11-25/2025-06-18）。
- 工具示例：`workflow.list`、`workflow.execute`、`workflow.get_execution`、`agent.run`、`agent.get_status`、`skill.search`、`skill.get`、`artifact.get`、`approval.request`。
- 高风险：`workflow.execute`/写操作/脚本执行默认审批。

## 3. 认证

### OAuth 2.1（推荐）

1. 客户端请求 MCP 端点 → 401 + `WWW-Authenticate`/资源元数据 URL。
2. 客户端读取 RFC 9728 → AS 发现（RFC 8414/OIDC）。
3. 授权码 + PKCE；CIMD 优先/DCR 兼容；RFC 8707 resource。
4. Token 绑定 tenant/workspace/scopes；刷新/撤销。
5. Hisha **不**把 token 透传给上游。

### PAT（CLI/无 OAuth）

- 在 Hisha Settings 创建短期 PAT；scope 最小；可撤销。
- 通过 `Authorization: Bearer` 或客户端环境变量配置。
- 不适合长期/共享；审计绑定用户/租户。

### stdio

- 本地进程；凭证通过环境变量注入；不走 OAuth。
- 能力令牌绑定工作区/工具/过期时间。

## 4. 客户端配置（形态示意）

> 字段名/认证方式必须按对应客户端当前官方文档验证；以下是 2026-11-01 的形态。

### Claude Code

```json
{
  "mcpServers": {
    "hisha": {
      "type": "http",
      "url": "https://hpa.example.com/mcp",
      "headers": { "Authorization": "Bearer ${HISHA_TOKEN}" }
    }
  }
}
```

- Skill：`.claude/skills/<name>/SKILL.md`。

### VS Code + Copilot

```json
{
  "servers": {
    "hisha": {
      "type": "http",
      "url": "https://hpa.example.com/mcp",
      "headers": { "Authorization": "Bearer ${input:hpa-token}" }
    }
  }
}
```

- 工作区 `.vscode/mcp.json` 使用 `servers`；可移植 `.mcp.json` 使用 `mcpServers`，不能混用。
- 扩展可用 `contributes.mcpServerDefinitionProviders` + `vscode.lm.registerMcpServerDefinitionProvider` 注册，无需写 `.vscode/mcp.json`；详见 [VS Code/Copilot 模块](../14-vscode-copilot/README.md)。
- 工具审批/工作区信任由 VS Code 管理；Hisha 仍服务端审批。

### Copilot CLI

- `mcp-config.json` 顶层 `mcpServers`（与 VS Code 的 `servers` 不同）。
- 需单独验证版本/认证支持。

### Codex

```toml
[mcp_servers.hisha]
url = "https://hpa.example.com/mcp"
bearer_token_env_var = "HISHA_TOKEN"
```

- Skill：`.agents/skills/<name>/` 或 `$CODEX_HOME/skills/`。
- 源码使用 rmcp（Rust），支持 stdio/Streamable HTTP；具体字段按版本验证。

### 企业 Agent

- 通过 Hisha MCP Client SDK/HTTP 接入；或直接用 Hisha Agent Runtime。
- 需要实现租户/认证/审计；不共享用户 token。

## 5. Skill 导出

- 标准目录（SKILL.md + scripts/references/assets）。
- 可选 `hpa.skill.json`；外部客户端忽略。
- 按客户端路径生成安装说明：
  - Claude Code：`.claude/skills/<name>/`
  - Codex：`.agents/skills/<name>/` / `$CODEX_HOME/skills/`
  - VS Code/Copilot：按官方文档
- 只导出允许许可的内容；附 NOTICE/署名。
- 导出包签名/校验和；不包含本地路径/secret。

## 6. 兼容性矩阵

| 能力 | Claude Code | VS Code/Copilot | Copilot CLI | Codex |
| --- | --- | --- | --- | --- |
| MCP stdio | 支持 | 支持 | 支持 | 支持 |
| MCP HTTP | 支持 | 支持 | 支持 | 支持（rmcp） |
| 2026-07-28 无状态 | 需验证 | 需验证 | 需验证 | 需验证 |
| 2025-11-25 兼容 | 支持 | 支持 | 支持 | 支持 |
| OAuth | 支持（按服务器） | 支持（按服务器） | 需验证 | 源码含 OAuth 发现 |
| Skills | `.claude/skills` | `.github/skills`/`.claude/skills`/`.agents/skills`；扩展 `chatSkills` | 单独验证 | `.agents/skills`/CODEX_HOME |
| 扩展注册 | — | `contributes.mcpServerDefinitionProviders` + `lm.registerMcpServerDefinitionProvider` | — | — |
| 审批 | 客户端提示 + Hisha 服务端 | 客户端提示 + Hisha 服务端 | 同左 | 同左 |

> 标记“需验证”的项必须在 PoC B 中用当前版本实测，不能凭规范推断。

## 7. 集成测试（PoC B）

1. 客户端发现 Hisha MCP（discover/initialize）。
2. 列出工具/schema。
3. 调用只读工具（`workflow.list`）。
4. 调用写工具（`workflow.execute`），触发审批；Hisha 服务端等待。
5. 审批后执行；客户端获取结果。
6. 加载 Hisha 导出 Skill；按 Skill 指导调用 MCP 工具。
7. 本地脚本执行/产物返回；审计完整。
8. 断网/本地模式（如适用）。
9. 旧协议客户端兼容。
10. 不同客户端配置文件互不混用。

## 8. 故障排查

| 症状 | 检查 |
| --- | --- |
| 401/403 | token/租户/scope/OAuth 发现 |
| 工具列表为空 | 权限/租户/Server 状态 |
| 工具不存在 | 版本/注册/客户端缓存 |
| 调用超时 | 工具超时/上游/审批等待 |
| 审批不出现 | 风险策略/服务端审批配置 |
| Skill 不加载 | 路径/frontmatter/客户端版本 |
| 协议错误 | 版本协商/discover/`_meta` |
| 配置不生效 | `servers` vs `mcpServers` vs `[mcp_servers]` |

## 9. 安全

- 外部客户端不是权限边界；审批/授权/审计在 Hisha 服务端。
- 不信任客户端 token passthrough；上游凭证由 Vault。
- 工具结果/资源内容可能包含 prompt injection；Agent/客户端需防护。
- 客户端配置中的 token 使用环境变量/密钥管理，不提交仓库。

## 10. 相关文档

- [MCP 架构](mcp-architecture.md)
- [MCP Server/Gateway](mcp-server.md)
- [MCP 安全](mcp-security.md)
- [外部客户端研究](../01-research/standards/external-ai-clients.md)
- [PoC B](../11-roadmap/poc-plan.md)
