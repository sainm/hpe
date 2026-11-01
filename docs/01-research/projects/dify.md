# Dify 1.18.0 研究报告

> 仓库：`langgenius/dify` · tag `1.18.0`（2026-10-23）· tarball SHA-256 `16654704373c1b6c...`<br>
> 研究方式：源码 tarball（15,203 文件）+ 许可证 + 关键执行路径；未构建运行。<br>
> 结论：**Design Reference；Do Not Copy Code**（许可证限制见 §12）。

## 1. 项目定位

面向 LLM 应用的可视化开发平台：Chatflow、Workflow、Agent、RAG、知识库、插件、发布为 API/WebApp。它把 AI 应用作为中心，而不是通用自动化平台。2026 年的版本已经把 Agent 和 Workflow 引擎拆成独立组件：`graphon` 工作流引擎、`dify-agent`（Python/Pydantic AI）与 `dify-agent-runtime`（Go）。

## 2. 核心功能

- 可视化 Workflow / Chatflow 编辑器，节点含 LLM、知识检索、Agent、数据源、人工输入、触发器。
- 应用发布：API、WebApp、MCP、工作流即工具（workflow-as-tool）。
- Agent v2：Pydantic AI 运行、工具/知识/文件绑定、会话存储、结构化输出。
- 插件体系：模型、工具、数据源、Agent 策略、触发器、端点由 plugin daemon 管理。
- 人工输入节点：表单、接收人、暂停/恢复、审批面。
- 触发器：Webhook、Schedule、插件事件。
- 内置 `skills/difyctl/SKILL.md`，用标准 Agent Skill 指导外部 AI 操作 Dify。

## 3. 技术架构

- `api/`：Python、Flask-RESTX、SQLAlchemy、Celery；Console/Service/OpenAPI/Inner API 分层。
- `web/`：Next.js + React + reactflow + Monaco + i18next + 自有 UI 包。
- `graphon==0.8.0`：外部图执行引擎，`api/core/workflow/workflow_entry.py` 调用 `graphon.engine.Engine`、`RuntimeState`、`VariablePool`、容器处理器（Loop/Iteration）。
- `dify-agent/`：Python，Pydantic AI；`runtime/runner.py`、`run_scheduler.py`、`compaction.py`。
- `dify-agent-runtime/`：Go，含 `internal/landlock`（Linux Landlock）、`cmd/shellctl`、SQLite；用于本地/沙箱执行。
- 插件：`api/core/plugin/impl/*`（model、tool、agent、trigger、oauth）通过 plugin daemon 调用；执行不在 API 进程内。
- 基础设施：Postgres/MySQL、Redis、对象存储、sandbox、plugin_daemon；`docker/docker-compose.yaml` 中 api/worker/web/db/redis/sandbox/plugin_daemon 分服务。

## 4. 技术栈

后端 Python 3.x + Flask-RESTX + SQLAlchemy + Celery + Pydantic；工作流引擎 graphon（Python）；Agent 运行时 Pydantic AI；本地沙箱 Go + Landlock；前端 Next.js + React + TypeScript + reactflow + Monaco + i18next；部署 Docker Compose。没有 Rust/Java 核心依赖。

## 5. 数据模型（从代码可见）

`App` → `Workflow`（`version` 区分 draft/发布，`version_number` 单调递增，`api/models/workflow.py`）→ `WorkflowRun` → 节点执行/事件；会话与消息；知识库/文档/分段；插件安装与工具；MCP Server；人工输入表单与接收人（`models/human_input.py`）。发布版本不可变，运行绑定版本。

## 6. 执行机制

- graphon 是帧化图执行器：root frame、ready queue、固定 worker pool、dispatcher、event stream；容器节点（Loop/Iteration）拥有子 frame 与独立变量池。
- Dify 在 `workflow_entry.py` 中构造 Engine，过滤事件，注入变量池、文件运行时、人工输入过滤器。
- 异步执行由 Celery 承担；`GraphEngine` 通过 Redis 命令通道接收中止指令（`execution_coordinator.py`）。
- 人工输入节点在图中暂停，持久化表单与接收人，等待恢复令牌；恢复继续同一运行。
- Agent v2 与 Workflow 是不同执行体：Agent 节点把运行委托给 dify-agent，Workflow 节点仍由 graphon 驱动。
- 触发器节点（schedule/webhook/plugin）把外部事件转为运行。

## 7. 扩展机制

- 插件 daemon 是主要扩展面：模型、工具、数据源、Agent 策略、触发器、端点；插件可独立安装/升级。
- 工作流 DSL 的导入/导出（Console 与 OpenAPI）。
- 标准 Agent Skill：`skills/difyctl`，含 `references/setup.md`、`operations.md`、`build-workflow.md`，通过 `difyctl` 操作服务器。
- 前端组件和节点配置由插件 schema 动态提供。

## 8. 插件开发方式

插件以独立包交付，注册到 plugin daemon；API 通过 `core/plugin/impl` 的客户端调用。插件协议覆盖 model/tool/agent/trigger/endpoint；具体模板与 SDK 不在本仓库完整展开。对 Hisha 的启示是“控制面不加载插件代码，执行面通过协议调用”，而不是直接复用其插件 ABI。

## 9. API 设计

四类 API：Console（工作区管理）、Service API（应用运行）、OpenAPI（发布给外部）、Inner API（内部服务）。OpenAPI 层含 `app_run.py`、`app_workflow.py`、`app_dsl.py`、`workflow_events.py`；运行事件以流式返回。插件、MCP、人工输入都有独立控制器。API 以应用/会话为中心，不是通用能力注册 API。

## 10. 部署架构

官方源码提供 Docker Compose，含 API、Celery worker、Web、Postgres/MySQL、Redis、sandbox、plugin_daemon、可选 knowledge FS worker。Kubernetes/Helm 与商业版能力不在本快照内，不能据此声称完整支持。

## 11. 前端交互设计

三栏式编辑器：左侧节点面板/应用导航，中间画布，右侧配置；底部调试与运行历史；顶部版本、发布、API、日志。Agent 与 Workflow 有不同入口。人工输入有独立表单页。插件配置由 JSON Schema 驱动。其交互适合作为“AI 应用工作台”的参考，但默认信息密度低于 Hisha 目标。

## 12. 许可证

根 `LICENSE` 是**修改版 Apache-2.0**：

- 未获书面授权，不得用 Dify 源码运营多租户环境（一个 workspace 对应一个 tenant）。
- 使用其前端时不得移除/修改控制台或应用中的 LOGO/版权信息。
- 其他条款沿用 Apache-2.0。

因此：可以研究设计与接口；**不能**把 Dify 作为 Hisha SaaS 的多租户内核，也不能直接复制其 `web/` 前端。若商业合作，需要单独授权。法律结论需法务复核。

## 13. 可直接复用

无代码级直接复用。可复用的是**设计模式**：graphon 式帧/容器执行、插件 daemon 隔离、Agent 与 Workflow 分体、人工输入暂停恢复、标准 Skill 指导外部 AI 操作平台。

## 14. 需要二次开发

若以 Dify 为基础二次开发，需要重写多租户、权限、许可证隔离和前端品牌；成本高于自研核心。不建议。

## 15. 仅值得借鉴

- 工作流版本快照 + 运行绑定版本。
- 容器节点（Loop/Iteration）用子 frame 管理变量作用域。
- 人工输入节点的“表单 + 接收人 + 恢复令牌”模型。
- 插件执行与 API 进程分离。
- 用标准 `SKILL.md` 把平台操作暴露给外部 AI（difyctl 是很好的范例）。
- 本地 Agent 沙箱使用 OS 级 Landlock，而不是仅靠进程隔离。

## 16. 不适合采用

- 修改版 Apache 的多租户/品牌限制。
- Flask + Celery + Python 单体作为 Hisha 的 Control Plane。
- graphon 作为 Hisha 的唯一 IR/运行时（它是 Python 引擎，且 IR 与 Dify 节点绑定）。
- 把 Agent 与 Workflow 混在同一节点执行器里。

## 17. 与 Hisha 的兼容性

概念兼容度高，代码兼容度低。Hisha 采用 Rust Core + TypeScript Control Plane/Web；Dify 的帧/容器语义和人工输入模型可映射到 Hisha IR，但不能共享其数据模型与插件 ABI。graphon（Apache-2.0）可作为 Python Worker 的候选执行内核，需单独 PoC。

## 18. 引入后的维护成本

作为依赖：高（Python/Flask/Celery/插件 daemon 多层、升级频繁、许可证风险）。作为参考：低（只读设计）。Hisha 的选择是后者，并把 graphon 作为独立评估对象，而不是把 Dify 整体引入。

## 证据路径

`LICENSE`、`api/core/workflow/workflow_entry.py`、`api/core/workflow/node_factory.py`、`api/core/workflow/nodes/human_input/*`、`api/core/plugin/impl/*`、`api/core/mcp/*`、`dify-agent/src/dify_agent/runtime/*`、`dify-agent-runtime/internal/landlock/*`、`dify-agent-runtime/cmd/shellctl/*`、`web/package.json`、`docker/docker-compose.yaml`、`skills/difyctl/SKILL.md`、`api/models/workflow.py`。
