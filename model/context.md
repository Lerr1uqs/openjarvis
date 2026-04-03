# Context

## 定位

- `context` 模块提供统一的消息基础类型。
- `MessageContext` 仍然存在，但已经退化为兼容层，不再是主链路事实宿主。

## 边界

- 负责定义 `ChatMessage`、`ChatMessageRole`、`ChatToolCall` 这些统一消息协议。
- 不负责持久化，不负责线程身份，不负责模型协议序列化。
- 不负责在 Router 或 Worker 热路径里组装请求上下文。

## 关键概念

- `ChatMessage`
  统一消息对象，覆盖 user、assistant、tool_call、tool_result 等角色。
- `ChatMessageRole`
  消息语义标签。
- `MessageContext`
  已废弃的兼容三段式容器。

## 核心能力

- 提供 LLM、Tool、Thread 共享的统一消息结构。
- 为兼容路径保留 `MessageContext` 的简单拼装和渲染能力。

## 使用方式

- 主链路里，Router 只负责转发 `incoming user message + ThreadContext`，不负责操控 message 上下文。
- `ThreadContext.messages()` 才是对外导出的完整请求消息序列。
- `MessageContext` 现在只保留给兼容路径、测试 helper 和辅助调用点，并已标记为 deprecated。
