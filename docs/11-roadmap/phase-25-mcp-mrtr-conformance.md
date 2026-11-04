# Phase 25：MCP 2026-07-28 MRTR conformance

> 状态：**本地验收完成**（2026-11-04）。SEP-2322 `InputRequiredResult` 的 14 个官方场景全部通过，2026-07-28 计分 server requirements 达到 37/37；tasks extension（SEP-2663）仍未实现且不计分。

## 1. 范围

- 工具 fixture：`test_input_required_result_elicitation`、`test_input_required_result_sampling`、`test_input_required_result_list_roots`、`test_input_required_result_request_state`、`test_input_required_result_multiple_inputs`、`test_input_required_result_multi_round`、`test_input_required_result_tampered_state`、`test_input_required_result_capabilities`。
- Prompt fixture：`test_input_required_result_prompt`，验证 `prompts/get` 也能返回 `InputRequiredResult`。
- 首轮无 `inputResponses` 时返回 `resultType: "input_required"`、`inputRequests`，需要状态时附带 `requestState`；重试从请求参数顶层读取 `inputResponses` 与回传的 `requestState`。
- 多输入请求同时包含 elicitation、sampling 与 `roots/list`；多轮场景每轮签发新状态。
- `requestState` 使用进程内密钥的 HMAC-SHA256 完整性标签；篡改或跨标签回传返回 JSON-RPC `-32602`。
- 能力过滤读取 `_meta["io.modelcontextprotocol/clientCapabilities"]`，只请求客户端声明的 elicitation/sampling/roots。
- 缺失或错误键重发 `InputRequiredResult`；`inputResponses` 为 `null` 或非对象时返回 `-32602`；未知额外键忽略。
- `tools/list` 与 `prompts/list` 永不返回 `input_required`。

## 2. 行为契约

- 所有 fixture 仅在 `HPA_MCP_CONFORMANCE=1` 的临时 Runtime 注册；MRTR prompt 分支只在 `2026-07-28` 现代协议启用。
- 首轮与重试均为无状态请求；不接受客户端伪造的完成结果，完成判定只来自服务端 fixture 逻辑。
- `inputResponses` 只用于 conformance fixture 的交互补全，不构成生产授权依据；生产高风险审批仍走 `mcp_approvals` 的一次性审批与参数哈希绑定。
- fixture 的 HMAC 密钥为进程随机值，仅覆盖单进程验收；跨节点生产状态不使用该标签。
- legacy `2025-11-25` 路径不变，legacy 请求不会进入 MRTR prompt 分支。

## 3. 验收

```bash
cargo build -p hpa-local
pnpm test:mcp-conformance:2026-mrtr
HPA_CONFORMANCE_REQUIREMENTS=2026-07-28 pnpm test:mcp-conformance
```

- `test:mcp-conformance:2026-mrtr`：14 个场景、40 项检查全部 SUCCESS，0 failed、0 warnings，包含 wire schema 校验。
- 完整 2026-07-28 requirements：37/37 计分 server 场景通过；总计 **162 passed / 29 failed**，29 个失败全部位于不计分的 tasks extension（9 个场景失败，`tasks-status-notifications` 通过），命令返回 `ok: true`。
- 2025-11-25 核心矩阵重跑 9/9；`server-stateless` 28/28；内容/Header 场景 18/18。
- 全量 Rust：250 passed / 0 failed / 1 ignored；Clippy `-D warnings` 与 fmt 通过。
- 真实 PostgreSQL 18.4：31 个测试文件（32 用例）全部通过；备份演练恢复到临时库后 21 个 migration、71 个业务关系、68 张 FORCE RLS 表一致。
- Web 12 Vitest；扩展 65 Vitest + 7 个 Extension Host 检查；品牌/IR/契约/文档（182 文件 881 链接）/镜像/OTel/供应链门禁通过。

## 4. 非目标

- 不实现 tasks extension（SEP-2663）的 fixture、任务信封、状态通知与取消语义；官方将其列为不计分扩展，当前 9 个场景失败保留在报告中，不用跳过或隐藏失败来换取表面全绿。
- 不把 conformance fixture 暴露到常规 Runtime，也不改变生产 MRTR 审批的数据库状态机。
- 不宣称真实外部 AI 客户端已完成模型工具调用；该部分仍为 `BLOCKED_EXTERNAL`。
