# Context

## 定位

- `context` 模块只提供统一的消息基础类型。
- 它是 `Thread`、`AgentLoop`、`ToolRegistry`、`LLMProvider` 共享的消息协议层。

## 严格边界

- 负责定义 `ChatMessage`、`ChatMessageRole`、`ChatToolCall` 这些统一消息协议。
- 不负责持久化，不负责线程身份，不负责 session/thread 初始化。
- 不负责在 Router、Worker 或 AgentLoop 热路径里组装请求上下文。

## 关键概念

- `ChatMessage`
  统一消息对象，覆盖 `system`、`user`、`assistant`、`toolcall`、`tool_result`。
- `ChatMessageRole`
  消息语义标签。
- `ChatToolCall`
  assistant 发起的结构化工具调用描述。
- `ContextTokenKind`
  请求预算统计时使用的 token 桶类型。

## 使用方式

- 持久化消息由 `Thread` 管理。
- LLM 请求序列由 `AgentLoop` 在运行时从 `Thread` 导出。
