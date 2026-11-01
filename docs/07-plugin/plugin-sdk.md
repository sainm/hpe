# Plugin SDK

> 目标：让 TypeScript 开发者用一套 SDK 定义 Connector/Node/Tool 的声明式元数据与 Schema，并生成可签名/可审核的插件包。
> **TS SDK 不等于把 TS 代码加载进 Rust 进程**：执行模式为声明式 HTTP/OpenAPI、Node 沙箱（可选）或 WASM；高风险/高性能逻辑用 Rust/WASM Plugin SDK。

## 1. 包结构

```text
my-plugin/
├── package.json
├── hpa.plugin.json      # 由 SDK 生成/校验
├── src/
│   ├── nodes/
│   ├── connectors/
│   ├── tools/
│   └── index.ts
├── skills/
├── agents/
├── mcp/
├── LICENSE
└── README.md
```

`package.json` 声明 `@hpa/plugin-sdk`、构建脚本、`files`。

## 2. SDK 接口

```ts
import { definePlugin, defineNode, defineConnector } from "@hpa/plugin-sdk";

export default definePlugin({
  id: "plugin_github_tools",
  name: "GitHub Tools",
  version: "1.2.0",
  license: "Apache-2.0",
  compatibility: { hisha: ">=1.0.0 <2.0.0" },
  nodes: [issueCreate],
  connectors: [github],
  tools: [issueCreateTool],
  skills: ["./skills/github-triage"],
  mcp: ["./mcp/servers.yaml"]
});

export const issueCreate = defineNode({
  type: "github.issue.create",
  typeVersion: "1.0.0",
  displayName: "Create GitHub Issue",
  category: "integration",
  configSchema: { /* JSON Schema */ },
  inputSchema: { type: "object" },
  outputSchema: { type: "object" },
  permissions: [{ kind: "network", hosts: ["api.github.com:443"] }],
  riskLevel: "write",
  async execute(ctx) {
    return ctx.connectors.github.createIssue(ctx.input);
  }
});

export const github = defineConnector({
  id: "github",
  version: "1.2.0",
  auth: [{ kind: "oauth2", scopes: ["repo"] }, { kind: "pat" }],
  operations: [{ name: "createIssue", inputSchema: {}, outputSchema: {} }],
  async invoke(op, args, ctx) {
    return ctx.http.post("https://api.github.com/repos/.../issues", args, {
      credential: ctx.credential,
      idempotencyKey: ctx.idempotencyKey
    });
  }
});
```

- `configSchema`/`inputSchema`/`outputSchema` 必须内联或引用可解析 JSON Schema。
- `execute`/`invoke` 是可选 JS handler：仅在 Node 沙箱（可选运行时）执行；没有 Node 时只能使用声明式 HTTP/OpenAPI 能力。
- 高风险/高性能插件建议用 Rust/WASM SDK；TS handler 不能绕过 Rust Plugin Host/Policy。
- 权限/风险/幂等在定义中声明；SDK 生成 manifest。

## 3. 构建/校验

```bash
hisha plugin init
hisha plugin dev            # 本地热加载（Local Runtime）
hisha plugin validate       # schema/权限/依赖/许可
hisha plugin test           # 单元/集成
hisha plugin build          # 生成 hpa.plugin.json + bundle
hisha plugin sign --key ... # Ed25519/Sigstore
hisha plugin pack           # .hplugin
```

- 构建：TS → ESM bundle；Node 原生依赖需声明/打包策略。
- 校验：manifest/schema/兼容性/依赖/权限/许可/文件路径。
- 测试：mock ctx（HTTP/凭证/日志/Artifact）；沙箱集成。
- 签名：发布者密钥；生成 digest/signature。

## 4. 本地开发

- `hisha plugin dev` 注册到本地 Runtime；节点/工具出现在 Designer/Registry。
- 热加载仅开发；生产安装走包。
- 使用测试凭证/本地 mock；不连生产。
- 调试：执行日志/输入输出/权限拒绝。

## 5. 权限

```ts
permissions: [
  { kind: "network", hosts: ["api.github.com:443"] },
  { kind: "filesystem", read: ["./inputs"], write: ["./artifacts"] },
  { kind: "commands", allow: ["git"] },
  { kind: "credential", provider: "github" }
]
```

- 最小权限；安装时展示；运行时强制。
- 高风险权限需管理员/审核。
- 插件不能自行提升权限；升级时权限差异需确认。

## 6. 版本/兼容

- 插件 semver；能力（Node/Connector/Tool）各自版本。
- Hisha 版本范围；不兼容拒绝安装。
- Node Type 版本与发布冻结；旧版本保留执行。
- 迁移：manifest `migrations`（可选）；文档说明。

## 7. 发布

1. `validate`/`test` 通过。
2. 生成 SBOM/许可/NOTICE。
3. 签名/打包。
4. 提交市场审核（自动+人工）。
5. 发布版本；不可变。
6. 监控/漏洞/撤回。

## 8. 测试

- 单元：schema/映射/权限/错误。
- 集成：mock HTTP/凭证/沙箱。
- 安全：SSRF/路径/命令注入/资源耗尽。
- 兼容：Hisha 版本/OS/运行时。
- 市场：安装/升级/回滚/卸载。
- 示例插件可运行。

## 9. 限制

- 不提供任意 UI 扩展（Phase 1）；节点配置由 schema 生成。
- 不提供后台常驻进程（除 MCP Server 由运行时管理）。
- 不允许插件修改平台核心/数据库。
- 不自动安装系统依赖；文档说明。

## 10. 相关文档

- [插件架构](plugin-architecture.md)
- [Marketplace](marketplace.md)
- [节点规范](../03-workflow/node-specification.md)
- [连接器架构](../08-platform/connector-architecture.md)
- [Skill 规范](../05-skill/skill-specification.md)
