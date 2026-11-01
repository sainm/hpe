# ADR-0018：表达式引擎采用 jq/jaq 子集

- 状态：已接受
- 日期：2026-11-01
- 决策者：架构负责人（技术栈）

## 背景

早期设计使用 JSONata 作为映射/条件语言。Rust Core 需要同一语义的本地解释器；Rust JSONata 实现不成熟（`jsonata-rs` 0.3.4 停更、`jsonata` 0.0.0），而 JSONata JS 无法安全/一致地嵌入 Rust 进程。需要一个纯 Rust、Active、可限制的表达式引擎。

## 决策

- 采用 **jq 子集**（jaq-core 3.1.2 + jaq-std 3.0.4 + jaq-json 2.0.3，MIT）。
- `$expr` 是 jq filter；`$template` 的 `${...}` 使用同一 profile。
- Context 根包含 `input`/`vars`/`nodes`/`env`/`execution`/`loop`/`parallel`。
- 条件必须返回布尔；缺失字段为 `null`（严格模式可报错）。
- 禁用 I/O/环境/时间/调试/模块内建（`input`/`inputs`/`$ENV`/`env`/`now`/`debug`/`stderr`/`halt*`/`include`/`import`）；使用 `.execution.now`。
- 限制表达式长度/AST 深度/正则/输入输出/执行时间。
- TS 前端不内嵌 jq 执行器；预览/校验调用 Rust API；不双实现语义。
- YAML 用 Saphyr 0.3.3（YAML 1.2 + span），替代 deprecated serde_yaml。

## 备选

- JSONata：易读但 Rust 实现不成熟；JS 运行时不可接受。
- JMESPath：查询成熟但变换能力弱；可作为只读子集，但不满足映射。
- CEL：安全/强类型但生态弱。
- 自研 DSL：长期成本/生态不足。

## 后果

- 文档/示例从 JSONata 迁移到 jq；旧 Workflow 需迁移/拒绝。
- jq 对非开发者学习成本更高；UI 提供模板/变量选择/预览。
- 需要 jaq 版本/内建白名单/资源限制/conformance 测试。
- 表达式错误/确定性/性能需专门测试。

## 证据

[表达式与数据映射](../03-workflow/expression-and-mapping.md)、[Rust 技术栈](../13-tech-stack/rust-stack.md)、[证据](../13-tech-stack/evidence.md)。
