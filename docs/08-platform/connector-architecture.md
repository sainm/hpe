# 连接器架构

> Connector 是外部系统适配器；Node/Tool/MCP Tool 是它的不同入口。凭证、网络策略、限流、重试、审计由运行时统一负责。

> 当前已落地的 HTTP/File/PostgreSQL/Git/Redis MQ/S3 实现和明确边界见 [Connector 生产扩展](connector-implementation.md)。

## 1. 概念

| 概念 | 作用 |
| --- | --- |
| Connector | 外部系统适配器：认证/操作/schema/文档 |
| Connection | Connector + Credential + 配置的实例 |
| Operation | Connector 暴露的操作（如 `createIssue`） |
| Node | Workflow 中调用 Operation（或内置节点） |
| Tool | Agent/MCP 可调用的操作（同一 Operation 的绑定） |
| Credential | 加密凭证引用（Vault） |

同一底层能力可以注册为 Node 与 Tool/MCP Tool，但每个入口独立权限/审批/限流。

## 2. 类型

| 类型 | 示例 | 认证 | 特殊控制 |
| --- | --- | --- | --- |
| HTTP/REST | 任意 API | OAuth2/PAT/Basic/HMAC | SSRF、域名白名单、重定向 |
| GraphQL | API | OAuth2/PAT | 查询深度/复杂度 |
| Database | Postgres/MySQL | 用户名/密码/IAM | 只读/写策略、语句白名单 |
| File | 本地/S3 | 路径/凭证 | 路径/工作区限制 |
| Git | Git/SVN/GitHub/GitLab/Azure DevOps | PAT/SSH/OAuth | 仓库/分支权限 |
| Email | SMTP/IMAP | 用户/密码/OAuth | 收件人/频率 |
| Message Queue | Kafka/RabbitMQ/SQS | 凭证 | topic/队列白名单 |
| Object Storage | S3/MinIO | 凭证 | bucket/前缀 |
| MCP Server | 外部 MCP | OAuth/PAT/stdio env | 工具白名单/审批 |

## 3. 接口

```ts
interface ConnectorDefinition {
  id: string;
  version: string;
  displayName: string;
  auth: AuthMethod[];
  operations: ConnectorOperation[];
  docs: string;
}

interface ConnectorOperation {
  name: string;
  description: string;
  inputSchema: JsonSchema;
  outputSchema: JsonSchema;
  risk: "read" | "write" | "destructive";
  idempotency: "supported" | "required" | "unsupported";
  rateLimit?: { perMinute: number; burst: number };
  invoke(args: unknown, ctx: ConnectorContext): Promise<unknown>;
}
```

`ConnectorContext`：credential（解密后短期）、http client、logger、idempotencyKey、trace、abort signal、artifact store。

## 4. 认证

- OAuth2：授权码/客户端凭证/设备码；token 刷新；scope。
- PAT/API Key/Basic：Vault 存储；短期注入。
- SSH/Git：密钥/证书；不落盘。
- HMAC/签名：请求签名/时间戳/重放保护。
- 凭证绑定 tenant/workspace/connection；不可跨租户复用。
- 连接测试：真实调用轻量端点；不记录 secret；结果分类。

## 5. 凭证

- 只存引用；`credentialRef`。
- 加密：AES-256-GCM + KMS/主密钥；租户密钥（Phase 6）。
- 轮换/撤销；过期提醒；连接测试。
- 运行时短期注入；不写日志/checkpoint/工作流文件。
- 访问审计：谁/何时/哪个连接/操作。

## 6. 网络

- 域名/IP 白名单；禁止 link-local/云元数据/回环（本地例外）。
- DNS 解析后校验；防 DNS rebinding；重定向逐跳校验。
- 代理支持；出站审计；超时/大小限制。
- 数据库连接：只读/写策略；SQL 参数化；禁止多语句（按策略）。
- Git：仓库 URL 白名单；分支保护；不执行 hooks。

## 7. 限流/重试/超时

- 连接器声明/平台策略；按 tenant/connection/operation。
- 重试：网络/5xx/限流；指数退避；`Retry-After`；幂等键。
- 超时：连接/请求/总时长；取消传播。
- 熔断：连续失败；恢复探测。
- 错误分类：认证/权限/校验/限流/超时/上游/网络；可重试标志。

## 8. 幂等

- 写操作要求 `idempotencyKey`（至少一次）；连接器映射到上游幂等键/请求 ID。
- 上游不支持时：Hisha 记录“可能已发生”；补偿/人工对账。
- 重试复用同一 key；不因 attempt 改变。
- 读操作可缓存（按策略）；写操作不缓存。

## 9. 版本/扩展

- Connector/Operation 版本；发布冻结；旧版本保留执行。
- 自定义 Connector 通过 Plugin SDK；内置 Connector 随平台版本。
- Marketplace 分发；许可/签名/审核。
- 连接器升级：配置迁移/连接测试；不影响运行中执行（绑定版本）。

## 10. 审计/监控

- 审计：连接测试/调用/凭证访问/失败/限流；资源/租户/操作。
- 指标：成功率/延迟/错误码/限流/重试。
- 日志：请求摘要/响应摘要；脱敏；不记录 body 全量（可配置）。
- 告警：认证失败/连接异常/高错误率。

## 11. 示例（HTTP）

```json
{
  "connector": "http",
  "connection": "conn_orders_api",
  "operation": "request",
  "config": {
    "baseUrl": "https://api.example.com",
    "auth": { "kind": "oauth2", "credentialRef": "cred_orders" },
    "allowedHosts": ["api.example.com"],
    "timeoutMs": 30000
  }
}
```

## 12. 测试

- 认证/刷新/过期/撤销。
- 连接测试/错误分类。
- SSRF/重定向/DNS rebinding/协议走私。
- 限流/重试/超时/熔断。
- 幂等/重复写。
- 数据库只读/写策略；Git 仓库/分支。
- 凭证不落日志/不跨租户。
- 版本/升级/回滚。

## 13. 相关文档

- [插件架构](../07-plugin/plugin-architecture.md)
- [Plugin SDK](../07-plugin/plugin-sdk.md)
- [权限模型](permission-model.md)
- [安全架构](../02-architecture/security-architecture.md)
- [节点规范](../03-workflow/node-specification.md)
