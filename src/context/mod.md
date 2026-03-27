# context 模块总览

## 作用

`context/` 负责系统内部统一消息上下文建模，以及和上下文预算相关的基础概念。它的目标是把不同来源的消息组织成一套稳定的、可供 LLM 和运行时共同消费的表示。

## 子模块

- `token_kind.rs`
  上下文 token 桶类型定义。负责描述 system、memory、chat、visible tool、reserved output 这些预算分类。

## 核心概念

- `ChatMessageRole`
  单条消息在上下文中的角色类型，例如 system、memory、user、assistant、tool、tool_result。
- `ChatMessage`
  统一后的单条上下文消息，是后续传给 LLM 的基础单元。
- `ChatToolCall`
  assistant 发起的一次工具调用描述。
- `MessageContext`
  LLM 上下文容器，按 `system`、`memory`、`chat` 三段组织消息。
- `RenderedPrompt`
  面向兼容层的简化渲染结果。
- `ContextTokenKind`
  请求级预算桶类型，用于表达某一段 token 占用属于哪一类上下文。

## 边界

- 这里负责“消息和预算概念如何表示”。
- 这里不负责“预算如何估算”，那是 `compact/budget.rs` 的职责。
- 这里也不负责“消息如何落盘到 thread/session”，那是 `thread.rs` 和 `session.rs` 的职责。
