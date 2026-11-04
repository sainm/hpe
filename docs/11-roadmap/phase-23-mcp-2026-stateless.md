# Phase 23：MCP 2026-07-28 server-stateless 官方 conformance

> 状态：**本地验收完成**（2026-11-04）。升级官方 conformance 套件到 0.2 alpha，并通过 2026-07-28 `server-stateless` 场景 28/28；完整 2026 requirements 的其余场景仍待补齐。

## 1. 范围

- 升级 `@modelcontextprotocol/conformance` 到 `0.2.0-alpha.12`，2025-11-25 核心场景保持 9/9。
- `scripts/mcp-conformance.mjs` 支持 `HPA_CONFORMANCE_REQUIREMENTS`（`--requirements`）与 `HPA_CONFORMANCE_BIN`；新增 `pnpm test:mcp-conformance:2026` 运行 2026 `server-stateless`。
- 现代协议错误语义：
  - 缺少 `_meta` / `protocolVersion` / `clientCapabilities` → JSON-RPC `-32602`，HTTP 400。
  - 不支持的版本 → `-32022`，HTTP 400，data 含 `requested` 与 `supported`。
  - Header 与 `_meta.protocolVersion` 不一致或 `Mcp-Method`/`Mcp-Name` 不匹配 → `-32020`，HTTP 400。
  - 缺失客户端能力 → `-32021`，HTTP 400，data 含 `requiredCapabilities`。
  - 2026 下已移除方法（initialize/ping/logging/setLevel/resources subscribe/unsubscribe）与未知方法 → HTTP 404 + `-32601`。
- 2026 `subscriptions/listen`：首帧改为 `notifications/subscriptions/acknowledged`，后续通知携带 `_meta.subscriptionId`（值为 listen 请求 id）。
- `HPA_MCP_CONFORMANCE=1` 增加诊断工具 `test_missing_capability`、`test_streaming_elicitation`、`test_logging_tool`、`test_trigger_tool_change`；常规 Runtime 不注册。

## 2. 行为契约

- Legacy（2025-11-25 及更早）行为不变：initialize、ping、logging 与 legacy SSE 继续可用。
- 现代请求必须同时提供 `MCP-Protocol-Version` 与 `_meta` 的协议版本/客户端能力；错误码和 HTTP 状态按 SEP-2575。
- 订阅确认必须先于任何通知；通知的 subscriptionId 等于打开流的请求 id。
- 2025 核心 conformance 与 2026 server-stateless 均通过官方套件；Phase 23 本身不等同于 2026 全 requirements 通过，后续场景由 Phase 24/25 补齐。

## 3. 验收

```bash
pnpm test:mcp-conformance
pnpm test:mcp-conformance:2026
```

- 2025-11-25 核心 9 个场景：9/9 PASS。
- 2026-07-28 `server-stateless`：28/28 PASS（0 failed，2 warnings）。

完整 2026 requirements（37 个计分 server 场景）可用以下命令查看剩余项：

```bash
HPA_CONFORMANCE_REQUIREMENTS=2026-07-28 pnpm test:mcp-conformance
```

Phase 23 完成时失败场景集中在 MRTR `input_required`（10 个计分场景）；工具媒体内容、Resources/Prompts、completion、caching、JSON Schema 与 Header 已由 [Phase 24](phase-24-mcp-content-conformance.md) 补齐，MRTR 已由 [Phase 25](phase-25-mcp-mrtr-conformance.md) 补齐，tasks extension 已由 [Phase 26](phase-26-mcp-tasks-extension.md) 补齐。

门禁：2025-11-25 官方核心 9/9、2026-07-28 `server-stateless` 28/28；全量 Rust 247 passed / 1 ignored；真实 PostgreSQL 18.4 上 31 个 PG 集成测试文件（32 用例）通过；备份演练 21 个 migration、71 个业务关系、68 张 FORCE RLS 表一致；Web 12 Vitest、扩展 65 Vitest + 7 host checks 通过；文档 180 文件 874 链接及品牌/IR/契约/镜像/OTel/供应链门禁通过。

## 4. 非目标

- 不实现完整 2026 requirements 的全部 server fixture 与 MRTR 场景（MRTR 已由 Phase 25 完成）。
- 不改变 2025 legacy 路径或真实 MCP 客户端行为。
- 不实现 tasks extension（SEP-2663）的官方 conformance 要求（已由 Phase 26 补齐）；持久 tasks 仍由仓库双节点测试覆盖。
