# 许可证分析

> 核查对象：实际下载的版本（见 [研究 README](README.md)），包含根许可证、子目录许可证与直接依赖声明。本文是工程判断，不构成法律意见；商业发布前必须由法务复核。

## 1. 结论总表

| 项目 | 固定版本 | 许可证文件 | SPDX 近似 | 商业 SaaS 嵌入 | 代码复用 | Hisha 结论 |
| --- | --- | --- | --- | --- | --- | --- |
| Dify | 1.18.0 | `LICENSE` | 修改版 Apache-2.0 | **需商业授权**（多租户） | 限制 | Do Not Use Code |
| n8n | 2.45.7 | `LICENSE.md` + `LICENSE_EE.md` | SUL 1.0 / Enterprise | **禁止** | 禁止 | Do Not Use Code |
| Kestra | 2.0.7 | `LICENSE` | Apache-2.0 | 可以 | 可以（保留 NOTICE） | Design Reference |
| Activepieces | 0.92.3 | `LICENSE` + `packages/ee/LICENSE` | MIT + Enterprise | 核心可以；EE 不可 | 核心可以 | Design Reference |
| LangGraph | 1.2.19 | `LICENSE` + 子包 | MIT | 可以 | 可以 | Integration |
| Temporal | 1.32.1 | `LICENSE` | MIT | 可以 | 可以 | Integration |
| Flowable | 8.0.0 | `LICENSE` | Apache-2.0 | 可以 | 可以 | Design Reference |
| Node-RED | 5.0.9 | `LICENSE` | Apache-2.0 | 可以 | 可以 | Design Reference |
| Flowise | 3.1.4 | `LICENSE.md` + EE | Apache-2.0 + Commercial | 核心可以；EE 不可 | 核心可以 | Design Reference |
| Airflow | 3.3.2 | `LICENSE` + `NOTICE` | Apache-2.0 | 可以 | 可以 | Design Reference |
| Dagster | 1.13.28 | `LICENSE` + 子包 | Apache-2.0 | 可以（排除 cloud） | 可以 | Design Reference |
| LiteLLM | 1.106.6 | `LICENSE` + `enterprise/LICENSE.md` | MIT + Enterprise | 核心可以；EE 不可 | 核心可以 | Integration |
| OpenTelemetry Spec | 1.62.0 | `LICENSE` | Apache-2.0 | 可以 | 可以 | Direct Reuse |
| MCP Servers | 2.2.0 | `LICENSE` | MIT→Apache 过渡 | 需逐文件确认 | 需逐文件确认 | Test Fixtures |
| Anthropic Skills | main | 各 Skill `LICENSE.txt` | 混合 | 仅 Apache 部分 | 仅 Apache 部分 | Standard + 部分示例 |
| awesome-copilot | main | `LICENSE` + 内容许可 | MIT | 可以（保留版权） | 可以 | Test Corpus |
| MCP Spec | 2026-07-28 | `LICENSE` | MIT→Apache + CC-BY-4.0 | 可以（按文件） | 可以（按文件） | Direct Reuse |
| Graphon | 0.8.0 | `LICENSE` | Apache-2.0 | 可以 | 可以 | Adaptation/参考 |

## 2. 关键限制逐项

### 2.1 Dify：修改版 Apache-2.0

根 `LICENSE` 明确：

- **多租户服务**：未获 Dify 书面授权，不得用 Dify 源码运营多租户环境；Dify 把一个 workspace 视为一个 tenant。
- **LOGO/版权**：使用其前端时不得移除/修改控制台或应用中的 LOGO/版权信息；不涉及前端的用途不受此限。
- 其余条款沿用 Apache-2.0。

影响：Hisha 的 SaaS/多工作区形态不能把 Dify 作为内核；即使私有化单租户，前端品牌限制仍需处理。`graphon` 是独立 Apache-2.0 项目，可单独评估，不继承 Dify 的附加限制。

### 2.2 n8n：Sustainable Use License 1.0

`LICENSE.md`：

- “Content of branches other than the main branch (i.e. "master") are not licensed.”——非 master 分支内容未授权。
- `.ee.` 文件/`.ee` 目录不属于 SUL，必须持有 n8n Enterprise License。
- 其余内容：仅可为自己的内部业务目的、非商业或个人用途使用/修改；仅可免费、非商业地分发；不得移除许可/版权声明。

影响：Hisha 是商业产品，**不能**复制 n8n 代码/节点/前端；只能做黑盒行为参考。SUL 不是 OSI 开源许可。

### 2.3 Activepieces：MIT + EE

`LICENSE`：

- `packages/ee/` 与 `packages/server/api/src/app/ee` 适用 `packages/ee/LICENSE`（Enterprise，生产需订阅/协议）。
- 其余为 MIT Expat。
- Piece 包可能有独立许可证；不能假设 760 个 Piece 都是 MIT。

影响：核心运行时/服务端可商用；EE 功能不可用；若导入 Piece，需要逐包许可清单。

### 2.4 Flowise：Apache-2.0 + Enterprise

`LICENSE.md`：

- `packages/server/src/enterprise` 与带明确版权声明的文件适用 Commercial License。
- 其余为 Apache-2.0。
- 第三方组件沿用原许可。

影响：核心画布/节点可商用；企业功能不可用；`agentflow` 包虽是 Apache-2.0，但版本 `0.0.0-dev.14`，不作为稳定依赖。

### 2.5 LiteLLM：MIT + Enterprise

`LICENSE`：

- `enterprise/` 目录适用 BerriAI Enterprise License（生产需商业协议）。
- 其余为 MIT。

影响：可直接集成 MIT 部分的 Proxy/SDK；SSO/RBAC/审计等企业功能不能用；Hisha 需要自建身份/权限。

### 2.6 MCP：MIT → Apache-2.0 过渡

`modelcontextprotocol/servers` 与 `modelcontextprotocol/modelcontextprotocol` 的 `LICENSE` 均写明：

- 项目正从 MIT 迁移到 Apache-2.0；新代码/规范贡献 Apache-2.0，文档（不含规范）CC-BY-4.0。
- 已获重许可同意的贡献 Apache-2.0；未同意的 MIT 贡献仍为 MIT。
- 不授予超出原许可的权利。

影响：不能笼统标注“Apache-2.0”或“MIT”；复用前按文件/包核查并保留归属。MCP 规范文本的引用需遵守 CC-BY-4.0（署名）。

### 2.7 Anthropic Skills：混合许可

- 仓库无根 LICENSE。
- 14 个 Skill Apache-2.0（academy-guide、algorithmic-art、brand-guidelines、canvas-design、claude-api、discernment-nudge、frontend-design、internal-comms、mcp-builder、skill-creator、slack-gif-creator、theme-factory、web-artifacts-builder、webapp-testing）。
- 4 个专有 source-available：docx、pdf、pptx、xlsx（© Anthropic，使用受 Anthropic 协议约束）。
- 1 个无许可证：doc-coauthoring（默认保留所有权利）。

影响：可复用标准结构与 Apache-2.0 示例；不得再分发专有/无许可 Skill。Hisha 的示例 Skill 应自行编写或只用 Apache 部分并附 NOTICE。

### 2.8 awesome-copilot

根 MIT；部分内容/扩展有独立 LICENSE/NOTICE（例如 `skills/anti-ui-slop/LICENSE`、`skills/azure-role-selector/LICENSE.txt`）。作为测试语料使用时保留版权；不要把内容打包进产品分发。

## 3. 直接依赖许可证抽查

| 复用候选 | 直接依赖（声明） | 结论 |
| --- | --- | --- |
| `rmcp@3.8.0`（Rust MCP） | 直接依赖以 Cargo.toml 为准；crate 声明 Apache-2.0（LICENSE 含 MIT→Apache 过渡） | 需保留 NOTICE/法务复核；替代 TS SDK 主实现 |
| `langgraph@1.2.19`（Python AI Worker） | `langchain-core` MIT、`pydantic` MIT 等 | 全部宽松；仍需传递依赖 SBOM |
| `@temporalio/client@1.25.0`（薄适配 Worker，非核心） | `@grpc/grpc-js` Apache-2.0、`@temporalio/common`/`proto` MIT、`nexus-rpc` MIT、`uuid` MIT | 宽松；保留 Apache NOTICE；仅 durable PoC |
| `@temporalio/worker@1.25.0`（薄适配 Worker，非核心） | `@swc/core` Apache-2.0、`webpack` MIT、`rxjs` Apache-2.0 等 | 宽松；构建链较重 |
| OpenTelemetry（TS/Python SDK/Collector） | Apache-2.0（以实际包为准） | 宽松 |

上述是**直接依赖声明**，不是完整传递依赖扫描。正式引入前必须：

1. 锁定 lockfile；
2. 生成 CycloneDX/SPDX SBOM；
3. 运行许可证策略检查（禁止 AGPL/SSPL/未知许可进入核心；允许清单需法务确认）；
4. 保留所有 NOTICE/版权文件；
5. 对 Python 包执行 `pip-licenses`/`syft`。

## 4. 义务清单

### Apache-2.0（Kestra、Flowable、Node-RED、Airflow、Dagster、OTel、graphon、Flowise 核心等）

- 保留 LICENSE/NOTICE 与版权声明；
- 修改文件需显著说明；
- 不得暗示原项目背书；
- 专利授权条款随分发生效；
- 若分发二进制/容器，附带 NOTICE。

### MIT（LangGraph、Temporal、LiteLLM 核心、awesome-copilot、MCP 参考等）

- 保留版权与许可文本；
- 无 copyleft；可与商业代码组合；
- 仍需 SBOM 与归属页。

### SUL（n8n）

- 仅内部业务/非商业/个人用途；商业分发禁止；
- 不改许可声明；
- Hisha 不使用。

### 修改版 Apache（Dify）

- 多租户需商业授权；
- 前端 LOGO/版权不可移除；
- Hisha 不使用代码。

### CC-BY-4.0（MCP 文档）

- 署名、链接许可、标明修改；
- 规范文本引用需保留出处。

### 企业/专有（EE、Anthropic 专有 Skill）

- 不在 Hisha 产品/仓库中出现；
- CI 路径/许可证门禁阻断。

## 5. 商业形态决策

Hisha 自身许可证尚未确定。可选方案（需要业务/法务裁决）：

1. **Open Core**：核心 IR/SDK/本地运行时 Apache-2.0，云/企业治理商业许可。生态友好，符合“分发能力”的定位；需要清晰的功能边界。
2. **Apache-2.0 全开源 + 托管服务收费**：最大化采用，但竞争对手可直接托管；适合以生态/标准为护城河。
3. **AGPL/SSPL**：保护托管收入，但会阻碍企业嵌入与客户端分发，与“Publish Everywhere”冲突；不建议。
4. **双许可**：社区版 + 商业版；需要贡献者许可协议（CLA）与商标策略。

建议方向：IR/规范/SDK/本地运行时 Apache-2.0；云端多租户/企业治理/市场商业授权。最终由业务方决定，见 [open-issues](../12-decisions/open-issues.md)。

## 6. 门禁

- 仓库根 `LICENSES/` 保存第三方许可与 NOTICE；
- CI 对 `ee/enterprise/proprietary` 路径、AGPL/SSPL/未知许可证失败；
- 发布 SBOM（CycloneDX）与归属页；
- 每次升级直接依赖重新扫描；
- 禁止把 n8n/Dify 代码复制进仓库，即使是“参考片段”；
- 禁止把 Anthropic 专有 Skill/awesome-copilot 内容打包进产品资产。

## 7. 证据

见各项目报告 §12 与 [研究 README](README.md) 的版本/哈希表。关键文件：`LICENSE`、`LICENSE_EE.md`、`packages/ee/LICENSE`、`enterprise/LICENSE.md`、`LICENSE.md`、各 Skill `LICENSE.txt`。
