# 外部 AI 客户端发布验收

> 状态：**BLOCKED_EXTERNAL**。本机没有 Claude Code、Codex、GitHub Copilot/Chat 的可用模型会话凭证，也没有企业 IdP、真实外部 Collector、CA/Vault 和多节点基础设施。以下命令和证据要求用于解除阻塞，不能由 mock、CLI RPC 或截图替代。

## 1. 共同 fixture

使用同一 Workflow、Skill 和 tenant：

```text
wf_fixture_sequence@1.0.0
skill_invoice_audit@1.0.0
tenant: local
MCP: http://127.0.0.1:8787/mcp
```

验收序列必须由真实模型主动选择工具：

1. 发现 `workflow.list` 与 Skill 工具；
2. 执行只读 `workflow.list`；
3. 请求 `workflow.execute`，触发服务端审批；
4. 在客户端/审批端批准，客户端重试；
5. 通过 `tasks/get` 或 `execution.get` 得到完成结果；
6. 拒绝高风险调用的负向回合；
7. 记录 tenant、session、traceId、审批 ID、execution ID 和最终状态。

仅完成 `initialize`、`tools/list`、手写 curl 或 HPA 自带 MCP Client 调用不计为外部模型 PASS。

## 2. Claude Code

所需权限/凭证：

- Claude Code 可登录的测试账号或 CI 凭证；
- 允许项目级 `.mcp.json`；
- 模型访问额度；
- 本地 HPA bearer token（仅通过安全输入，不写仓库）。

复现：

```bash
cd <isolated-acceptance-workspace>
cat .mcp.json
claude
```

Prompt：

```text
Use the HPA MCP server. List published workflows, execute
wf_fixture_sequence version 1.0.0 with {"value":1}, complete the required
server-side approval, and report the final execution result. Also attempt one
high-risk call that I will reject.
```

保留：客户端版本、完整 transcript、MCP request/response 日志（脱敏）、审批与 execution 证据。

## 3. OpenAI Codex

所需权限/凭证：

- Codex CLI/IDE 的测试登录或 CI 凭证；
- `config.toml` 中 HPA `mcp_servers` 配置；
- 模型会话权限与额度；
- 本地 HPA bearer token。

复现：

```bash
codex --version
codex
```

Prompt 与 Claude 相同。保存 Codex 原生日志/transcript 和 HPA 端 trace/审计。不得用 `codex exec` 之外的 HPA MCP Client 调用补足模型回合。

## 4. GitHub Copilot / VS Code

所需权限/凭证：

- VS Code 1.143+、GitHub Copilot Chat/Agent 可用账号；
- MCP 工具与 Agent Skills 功能开关；
- 工作区信任；
- HPA Extension/本地 Runtime。

步骤：

1. 安装当前 VSIX；
2. 打开隔离工作区；
3. 确认 Copilot 发现 `.github/skills/<skill>/SKILL.md`；
4. 使用 `vscode: MCP: List Servers` 确认 HPA provider；
5. 在 Copilot Chat 输入与上面相同的真实模型 prompt；
6. 记录 Copilot Chat 工具卡片、审批、execution/trace；
7. 重启 VS Code 后确认 session 不影响服务端权限与审批。

## 5. 三种部署对照

同一 fixture 至少在以下部署各跑一次：

| 部署 | 必需外部资源 | 当前状态 |
| --- | --- | --- |
| 本地 SQLite | 无 | 可准备，外部模型凭证缺失 |
| 单控制面 Hybrid Worker | 第二台 Worker/主机 | BLOCKED_EXTERNAL |
| PostgreSQL 多节点 | 2+ 控制面、PostgreSQL HA、负载均衡 | BLOCKED_EXTERNAL |

对照字段：工具发现版本、MCP task ID、审批结果、execution 状态、traceId、错误码、时延、模型 usage。不得把 SQLite mock 结果填到其他两列。

## 6. Release gate

发布前必须附：

- 至少两个真实外部 AI 客户端的成功证据；
- 一个高风险拒绝证据；
- MCP conformance 结果；
- SBOM、包签名、checksums；
- migration/rollback、HA/PITR、Collector 与 Secret Manager 演练；
- 已知限制与 BLOCKED 项清单。

当前以上外部模型证据均未取得，因此 Phase 13 P13-08 保持 `BLOCKED_EXTERNAL`。
