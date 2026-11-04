# Phase 24：MCP 2026-07-28 内容与 Header conformance

> 状态：**本地验收完成**（2026-11-04）。补齐工具媒体内容、Resources/Prompts、completion、caching、JSON Schema 2020-12 与 Header 校验场景；MRTR 与 tasks extension 仍待后续阶段。

## 1. 范围

- 工具 fixture：`test_image_content`、`test_audio_content`、`test_embedded_resource`、`test_multiple_content_types`、`test_tool_with_progress`、`json_schema_2020_12_tool`、`test_custom_headers`。
- Resources：`test://static-text`、`test://static-binary`、`test://template/{id}/data`，列表与模板在 conformance 模式下出现。
- Prompts：`test_simple_prompt`、`test_prompt_with_arguments`、`test_prompt_with_embedded_resource`、`test_prompt_with_image`。
- `completion/complete` 返回合法 completion 结构；`server/discover` 声明 completions 能力。
- 2026 `resources/read` 结果带 `cacheScope` 与 `ttlMs`，满足 caching 检查。
- Header 校验：`Mcp-Method`/`Mcp-Name`/`Mcp-Param-*`，支持 OWS 修剪、`=?base64?...?=` 解码、缺省与不匹配拒绝（-32020 / HTTP 400）。
- 带 `progressToken` 的 `tools/call` 使用 SSE 返回 `notifications/progress` 0/50/100，再返回最终结果。

## 2. 行为契约

- 所有 fixture 仅在 `HPA_MCP_CONFORMANCE=1` 的临时 Runtime 注册，常规工具目录不出现。
- 2026/legacy 既有路径不变；legacy 请求不触发 2026 专用校验。
- Header 解析按 RFC 9110 去除前后 OWS；`Mcp-Name` 仍区分大小写。
- `Mcp-Param-*` 仅在存在对应参数值时校验；base64 包裹值必须可解码且与 body 一致。
- 内容 fixture 只用于协议验收，不代表生产工具输出。

## 3. 验收

```bash
pnpm test:mcp-conformance:2026
pnpm test:mcp-conformance:2026-content
```

`test:mcp-conformance:2026-content` 覆盖 18 个场景：tools 媒体内容与进度、resources 列表/读取/模板、prompts 四种读取、completion、caching、JSON Schema 2020-12、标准 Header 与自定义 Header 校验；全部 0 failed。

Phase 24 完成时完整 2026 requirements 为 **142 passed / 39 failed**；剩余失败集中在 MRTR `input_required`（10 个计分场景）与 tasks extension（不计分）。MRTR 已由 [Phase 25](phase-25-mcp-mrtr-conformance.md) 补齐，计分 server 场景现为 37/37。

门禁：2025-11-25 官方核心 9/9、2026-07-28 `server-stateless` 28/28、内容场景 18/18；全量 Rust 247 passed / 1 ignored；真实 PostgreSQL 上 31 个 PG 集成测试文件（32 用例）通过；备份演练 21 个 migration、71 个业务关系、68 张 FORCE RLS 表一致；Web 12 Vitest、扩展 65 Vitest + 7 host checks；文档 181 文件 877 链接及品牌/IR/契约/镜像/OTel/供应链门禁通过。

## 4. 非目标

- 不实现 MRTR `InputRequiredResult` fixture（已由 Phase 25 完成）。
- 不实现 tasks extension（SEP-2663）官方场景。
- 不把 conformance fixture 暴露到常规 Runtime。
