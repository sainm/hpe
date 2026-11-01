# MCP Center

> 路由：`/mcp`（概览）、`/mcp/servers`、`/mcp/servers/:id`、`/mcp/explorer`、`/mcp/gateway`。默认日语。

## 1. 导航

| Tab | 内容 |
| --- | --- |
| Servers | 外部 MCP Server（Client）与本地 Server |
| Gateway | 对外 MCP 端点/租户/认证/工具暴露 |
| Tools | 工具注册/schema/风险/审批 |
| Resources | 资源 URI/权限/租户 |
| Prompts | 模板/参数/版本 |
| Explorer | 连接/发现/测试调用 |
| Audit | MCP 调用审计（权限） |

## 2. Servers 列表

- 列：名称、类型（remote/local）、传输（stdio/http/legacy）、状态（connected/degraded/disconnected）、工具数、租户/项目、最近检查、所有者。
- 操作：注册、测试连接、发现、启用/禁用、编辑、删除。
- 空状态：注册/示例/文档。
- 状态徽标：连接/认证/版本/错误。

## 3. Server 详情

```text
┌──────────────────────────────────────────────────────────────┐
│ ← MCP | 名称 [Connected] | テスト 发现 編集 削除             │
├──────────────────────────────┬───────────────────────────────┤
│ 接続設定                     │ ツール/リソース/プロンプト     │
│ transport/url/command        │ 一覧 + schema + リスク         │
│ auth/OAuth/PAT               │                               │
│ timeout/network              │                               │
│ tool allowlist               │                               │
├──────────────────────────────┴───────────────────────────────┤
│ 接続ログ/最新の discovery/エラー                             │
└──────────────────────────────────────────────────────────────┘
```

字段：

- 基本：名称/描述/租户/项目/标签。
- 传输：stdio（command/args/env refs）或 http（url/headers/auth）。
- 认证：OAuth（资源元数据/AS/scope）/PAT/无；凭证引用。
- 策略：允许工具/风险覆盖/审批/超时/网络白名单。
- 发现：协议版本（2026-07-28/2025-11-25）、工具/资源/Prompt、缓存。
- 健康：最近心跳/错误/延迟。

操作：

- 测试连接：认证/发现/最小工具调用；不记录 secret。
- 发现：刷新工具/schema；显示变更。
- 启用/禁用：禁用后新调用拒绝；运行中继续。
- 删除：影响检查（Agent/Workflow 引用）。

## 4. Gateway

- 端点 URL（租户）；复制/旋转 token。
- 协议版本：2026-07-28（默认）/旧版兼容；显示协商统计。
- 认证：OAuth 客户端/PAT/服务账号；scope。
- 暴露工具：从 Capability Registry 选择/绑定；风险/审批/限流。
- 资源/Prompt 可见性。
- 限流/配额/审计配置。
- 安全：Origin/CORS/网络；高风险审批。
- 测试：用 Explorer 以外部客户端方式调用。

## 5. Tools

- 列表：名称、Server、风险、审批、只读/写、最近调用、错误率。
- 详情：input/output schema、annotations、示例、权限、限流。
- 操作：启用/禁用、风险覆盖（只能提高）、审批策略、测试调用。
- 工具结果：结构化/文本/资源；大小限制。

## 6. Resources / Prompts

- Resources：URI/名称/MIME/租户/权限；预览；订阅状态。
- Prompts：名称/参数 schema/模板/版本；预览；测试。
- 外部客户端可见性；公共/租户/项目。
- 敏感资源标记；`cacheScope`。

## 7. Explorer

```text
┌──────────────────────────────┬───────────────────────────────┐
│ Server 選択 / 接続           │ リクエスト/レスポンス         │
│ Tools/Resources/Prompts      │ JSON-RPC / 整形 / コピー      │
│ パラメータフォーム           │ レイテンシ/エラー/トレース    │
│ [実行]                       │                               │
└──────────────────────────────┴───────────────────────────────┘
```

- 选择 Server/工具；schema 表单/JSON 输入。
- 执行：认证/权限/审批；显示等待/错误。
- 结果：JSON/文本/资源；大小/截断。
- 历史：最近调用；重放（只读）。
- 不显示 secret；参数脱敏。

## 8. 状态

| 状态 | 表现 |
| --- | --- |
| 未连接 | 注册/连接 |
| 连接中 | 进度/取消 |
| 认证失败 | 重新授权/检查 scope |
| 版本不支持 | 显示 supportedVersions/兼容层 |
| 空工具 | 权限/Server 能力/发现 |
| 限流 | 429/retry-after |
| 审批等待 | 状态/链接/取消 |
| 离线（本地） | 本地 Server 可用；远端禁用 |

## 9. 窄屏（607px）

- Tabs 横向滚动；详情单列。
- Explorer 上下分栏；请求/响应全屏切换。
- 工具列表卡片化；schema 可折叠。
- 顶部只保留返回/名称/测试/菜单。

## 10. 验收

- 注册/连接/发现/调用/禁用闭环。
- 2026-07-28 与旧版兼容；错误可解释。
- OAuth/PAT/stdio 认证正确；token 不泄漏。
- 工具风险/审批/限流/审计。
- 租户隔离；资源过滤。
- 外部客户端通过 Gateway 调用（PoC B）。
- 三语/窄屏/键盘。

## 11. 相关文档

- [MCP 架构](../06-mcp/mcp-architecture.md)
- [MCP Client](../06-mcp/mcp-client.md)
- [MCP Server/Gateway](../06-mcp/mcp-server.md)
- [MCP 安全](../06-mcp/mcp-security.md)
- [外部 AI 集成](../06-mcp/external-ai-integration.md)
- [UI 设计系统](ui-design.md)
