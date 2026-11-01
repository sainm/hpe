# 开源研究：方法、证据与限制

> 证据冻结：2026-11-01。研究对象是当日可获取的真实源码 tarball，不是 README 摘要。

## 1. 方法

1. 用 GitHub REST API 获取仓库元数据与最新 release（未使用 token；`/tmp/hisha-research/meta/` 保存原始 JSON）。
2. 从 `codeload.github.com` 下载 release tag 对应的源码 tarball；无 release 的两个仓库取 `main` 分支快照。GitHub 的 raw 域名在本机不可达，因此不采用“只读单个网页文件”的方式。
3. 计算 tarball SHA-256，作为本次研究证据的不可变标识。核对解压后的根目录名与 tag 一致。
4. 逐项目检查：
   - 根目录与 `LICENSE`、`LICENSE_EE`、`NOTICE`、子目录许可证；
   - 语言与框架 manifest（`package.json`、`pyproject.toml`、`go.mod`、`pom.xml`、`build.gradle`）；
   - 执行引擎/调度器/队列/持久化代码路径；
   - 扩展机制（节点、Piece、插件、Skill、MCP）；
   - API 路由、前端技术栈、部署产物；
   - 版本、发布日、仓库活跃度。
5. 对直接复用候选核对直接依赖的声明许可证（npm/PyPI 元数据），并记录仍需传递依赖扫描的边界。
6. 对当前 MCP 规范与外部客户端官方文档单独建标准研究，避免把旧协议当成现状。

## 2. 版本与证据

| 项目 | 仓库 | 固定版本 | 发布日 | 仓库声明许可证 | tarball SHA-256（前 16 位） |
| --- | --- | --- | --- | --- | --- |
| Dify | langgenius/dify | 1.18.0 | 2026-10-23 | 修改版 Apache-2.0（多租户/LOGO 附加条件） | `16654704373c1b6c` |
| n8n | n8n-io/n8n | n8n@2.45.7 | 2026-10-30 | Sustainable Use License 1.0 + Enterprise | `b4c2ce499557eab0` |
| Kestra | kestra-io/kestra | v2.0.7 | 2026-10-20 | Apache-2.0 | `6ff9a1a7cebdaa5c` |
| Activepieces | activepieces/activepieces | 0.92.3-hotfix.2 | 2026-10-28 | MIT + `packages/ee` Enterprise | `0af2c323921a1713` |
| LangGraph | langchain-ai/langgraph | 1.2.19 | 2026-10-30 | MIT | `eafecec6be9a3719` |
| Temporal | temporalio/temporal | v1.32.1 | 2026-10-08 | MIT | `9a1766efcdc2acf0` |
| Flowable | flowable/flowable-engine | flowable-8.0.0 | 2026-02-27 | Apache-2.0 | `c8ecb65911cd0deb` |
| Node-RED | node-red/node-red | 5.0.9 | 2026-10-21 | Apache-2.0 | `b3a133c343f60b6c` |
| Flowise | FlowiseAI/Flowise | flowise@3.1.4 | 2026-07-29 | Apache-2.0 核心 + Enterprise 目录 | `77ab8115dd39f031` |
| Airflow | apache/airflow | 3.3.2 | 2026-09-17 | Apache-2.0 | `14710da45ccd36cf` |
| Dagster | dagster-io/dagster | 1.13.28 | 2026-10-30 | Apache-2.0 | `75446eb942e5ee80` |
| LiteLLM | BerriAI/litellm | v1.106.6 | 2026-10-28 | MIT 核心 + `enterprise/` 商业许可 | `64aca892cc25672d` |
| OpenTelemetry Specification | open-telemetry/opentelemetry-specification | v1.62.0 | 2026-10-22 | Apache-2.0 | `87d5741e92ed1201` |
| MCP Servers | modelcontextprotocol/servers | v2.2.0 | 2026-10-28 | MIT → Apache-2.0 过渡（文档 CC-BY-4.0） | `51239f3abffced0b` |
| Anthropic Skills | anthropics/skills | main 快照 | 无 release | 各 Skill 独立许可，仓库无根许可证 | `6fbb56693915c88d` |
| awesome-copilot | github/awesome-copilot | main 快照 | 无 release | MIT | `3007f36b8b5bb38c` |
| MCP Specification | modelcontextprotocol/modelcontextprotocol | 2026-07-28 | 2026-07-28 | MIT → Apache-2.0 过渡；文档 CC-BY-4.0 | `7df4915b4a5cdc99` |
| Graphon（Dify 引擎，补充） | langgenius/graphon | v0.8.0 | 2026-09-09 | Apache-2.0 | `45609df57f396623` |

完整 64 位哈希与下载 URL 见本文件末尾附录。研究快照位于 `/tmp/hisha-research/`，不进入仓库；仓库内只保留可复现的命令、版本与结论。
Rust/TypeScript/Python/CSS 依赖版本与许可证见 [技术栈证据](../13-tech-stack/evidence.md)。

## 3. 研究限制

- **未大规模构建/运行每个项目**。研究以源码结构、manifest、许可证与关键执行路径为主；个别结论（如运行时行为）标注为“待 PoC 验证”。
- **未完成全量传递依赖许可证扫描**。已核对直接复用候选的直接依赖；正式引入前必须生成 SBOM（`syft`/`cdxgen`）并执行许可证门禁。
- **无 release 的两个仓库**使用 `main` 快照，不能代表稳定版本；报告中明确标注。
- GitHub 星数、`pushed_at` 只用于活跃度参考，不作为技术判断依据。
- 本机无法访问部分官方文档（OpenAI 文档返回 403）；Codex 结论来自其官方仓库源码与文档，其他客户端结论来自官方文档页面。
- 许可证结论是工程判断，不构成法律意见；Dify、n8n、EE 目录、MCP 过渡期与 Anthropic 混合许可必须由法务复核。

## 4. 复现命令

```bash
# 下载固定版本源码（示例：Temporal）
curl -L -o temporal.tar.gz \
  https://codeload.github.com/temporalio/temporal/tar.gz/refs/tags/v1.32.1
shasum -a 256 temporal.tar.gz
# 期望：9a1766ef...（完整值见附录）

# 查看许可证，而不是只看仓库首页
tar -xzf temporal.tar.gz
grep -n '^' temporal-1.32.1/LICENSE | head -20

# 查看后端技术栈
grep -nE '^(module|go )' temporal-1.32.1/go.mod
```

> 复现时注意：本仓库的 SHA-256 是对本次下载的 tarball 计算的；GitHub 重新打包同一 tag 时字节可能不同，应以 tag 内容为准并记录日期。

## 5. 结论摘要

- 没有任何一个项目可以整体作为 Hisha 的内核：它们分别偏向 AI 应用、SaaS 自动化、JVM 数据编排、BPMN、本地消息流或 Agent 库。
- 可**直接依赖**的成熟组件：Temporal（长期 durable execution 适配）、LangGraph Python（可选 Agent）、LiteLLM（模型网关）、Rust RMCP（MCP 协议）、OpenTelemetry（可观测性）。
- 必须**自研**的部分：Canonical IR + 编译器/校验器、跨入口的统一能力注册与权限、Skill 本地运行时、MCP Gateway 与 2026-07-28 兼容层、Hybrid Worker 协议、审批与执行策略。
- 必须**避免**的代码复用：n8n（SUL 限制商业分发）、Dify（多租户 SaaS 需商业授权、LOGO 限制）、Activepieces/Flowise/LiteLLM 的 `ee`/`enterprise` 目录、Anthropic 的 docx/pdf/pptx/xlsx 专有 Skill、MCP 参考服务器作为生产实现。

## 附录：完整哈希与下载地址

| 项目 | 下载地址 | SHA-256 |
| --- | --- | --- |
| Dify 1.18.0 | `https://codeload.github.com/langgenius/dify/tar.gz/refs/tags/1.18.0` | `16654704373c1b6cfe81a77bf0402b4f75cab7791ce2485d6fcee5f18e1d9ec4` |
| n8n 2.45.7 | `https://codeload.github.com/n8n-io/n8n/tar.gz/refs/tags/n8n@2.45.7` | `b4c2ce499557eab09e5b400b1f8d0b8ab54f2396ac9c8d6943c20183ac2d2553` |
| Kestra 2.0.7 | `https://codeload.github.com/kestra-io/kestra/tar.gz/refs/tags/v2.0.7` | `6ff9a1a7cebdaa5c46e30a8420df9bcfa0dc092ac4accac16fc393fc5b6c0208` |
| Activepieces 0.92.3-hotfix.2 | `https://codeload.github.com/activepieces/activepieces/tar.gz/refs/tags/0.92.3-hotfix.2` | `0af2c323921a171309e1943cf81ee0a3bc1ca06a8153aa877d8fddb2b3478ba0` |
| LangGraph 1.2.19 | `https://codeload.github.com/langchain-ai/langgraph/tar.gz/refs/tags/1.2.19` | `eafecec6be9a371973e2d0b800f5e9198185ca8ad58fdb21339e42468aaab73d` |
| Temporal v1.32.1 | `https://codeload.github.com/temporalio/temporal/tar.gz/refs/tags/v1.32.1` | `9a1766efcdc2acf03165e0304549d00930e18ff4fbf2c5abe27732fe7c04f7ee` |
| Flowable 8.0.0 | `https://codeload.github.com/flowable/flowable-engine/tar.gz/refs/tags/flowable-8.0.0` | `c8ecb65911cd0debefbe297c585b9c41f3afc13884986f5a6f4a36b107230efd` |
| Node-RED 5.0.9 | `https://codeload.github.com/node-red/node-red/tar.gz/refs/tags/5.0.9` | `b3a133c343f60b6c4e35bdc574cdf51b786f36a77800539f6be03488809b5acb` |
| Flowise 3.1.4 | `https://codeload.github.com/FlowiseAI/Flowise/tar.gz/refs/tags/flowise@3.1.4` | `77ab8115dd39f031063be0054eca1224945c21a7521ffa544df4a2c595a126f8` |
| Airflow 3.3.2 | `https://codeload.github.com/apache/airflow/tar.gz/refs/tags/3.3.2` | `14710da45ccd36cf9fcd22c9ac3523f9d3a871918baf20413c2d60d501e4c1f4` |
| Dagster 1.13.28 | `https://codeload.github.com/dagster-io/dagster/tar.gz/refs/tags/1.13.28` | `75446eb942e5ee80bb9dbfc740df28f7d513daba32be60b196f9ee4be4a8cb45` |
| LiteLLM v1.106.6 | `https://codeload.github.com/BerriAI/litellm/tar.gz/refs/tags/v1.106.6` | `64aca892cc25672da099162d2922c8933157b4bd4153a0273b2ec679bc4c3688` |
| OpenTelemetry Spec v1.62.0 | `https://codeload.github.com/open-telemetry/opentelemetry-specification/tar.gz/refs/tags/v1.62.0` | `87d5741e92ed1201b79fca1cbed9d79715ae0f969aeb33a47802241d9446bb68` |
| MCP Servers v2.2.0 | `https://codeload.github.com/modelcontextprotocol/servers/tar.gz/refs/tags/v2.2.0` | `51239f3abffced0bcefba2da6a682ef46233abffb84b464cee79ee8d0c26fd76` |
| Anthropic Skills main | `https://codeload.github.com/anthropics/skills/tar.gz/refs/heads/main` | `6fbb56693915c88d6d57d4ea92b3c581c6159cac838402e268cb37708204d76a` |
| awesome-copilot main | `https://codeload.github.com/github/awesome-copilot/tar.gz/refs/heads/main` | `3007f36b8b5bb38c8702f03deece736f5850cc6d2192aab842f0cd2f73099359` |
| MCP Spec 2026-07-28 | `https://codeload.github.com/modelcontextprotocol/modelcontextprotocol/tar.gz/refs/tags/2026-07-28` | `7df4915b4a5cdc999876f5a80b7af9912bdea21490827224e8c7a10f2c81470a` |
| Graphon v0.8.0 | `https://codeload.github.com/langgenius/graphon/tar.gz/refs/tags/v0.8.0` | `45609df57f396623892cca6a81f85376067ab00d1b87d7546e200a39970648ea` |
