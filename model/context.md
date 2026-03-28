# Context

## 定位

- `MessageContext` 是送给 LLM 的统一消息组织层。
- 它不关心消息从哪来，只关心这些消息在 prompt 里怎么排。

## 边界

- 负责组织 `system / memory / chat` 三段上下文。
- 不负责持久化，不负责线程身份，不负责模型协议序列化。

## 关键概念

- `ChatMessage`
  统一消息对象，覆盖 user、assistant、tool_call、tool_result 等角色。
- `ChatMessageRole`
  消息语义标签。
- `MessageContext`
  LLM 输入的三段式容器。

## 核心能力

- 累加 system prompt、memory 和 chat 历史。
- 把 thread 历史展开成 LLM 可消费的消息序列。
- 为协议兼容层提供统一消息顺序。

## 使用方式

- Worker 会把线程历史和当前用户输入装进 `MessageContext`。
- AgentLoop 在真正请求模型前，再把它扩展成完整的 ReAct 请求消息。
