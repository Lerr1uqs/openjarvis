# AgentLoop

## 定位

- `AgentLoop` 是单轮 Agent 执行核心。
- 它维护这一轮里的 `generate -> tool -> generate` 循环，直到模型不再请求工具。

## 边界

- 负责请求前的 feature rebuild 时机判断、可见工具计算、工具执行、compact、hook 触发、事件生成。
- 负责把当前轮 user / assistant / tool 消息写入 `ThreadContext` 的 live chat。
- 不负责请求排队，不负责 session 持久化，不负责 channel 出站。
- 不再负责手工拼装 `toolset/skill/auto_compact/memory` prompt 向量。

## 关键概念

- `RequestState`
  本次 generate 的消息、工具和预算快照。
- `AgentLoopEvent`
  Router 可直接回发的结构化事件。
- `AgentLoopOutput`
  单轮最终结果，包含 turn messages、thread_context 和元数据。
- `FeaturePromptRebuilder`
  loop 在请求前调用的固定 provider 编排器，负责把静态 feature system prompt 和 live memory 写回 `ThreadContext`。
- `AutoCompactor`
  loop 在预算刷新后调用的动态容量注入器，负责把瞬时 context capacity 提示写入 `ThreadContext.live_system_messages`。

## 核心能力

- 基于 `ThreadContext` 计算当前线程可见工具。
- 在每次 generate 前先 rebuild `features_system_prompt` / live memory，再通过 `ThreadContext.messages()` 导出完整请求。
- 通过 `FeaturePromptProvider` 注入 toolset catalog、skill catalog、auto-compact 稳定说明和 memory。
- 通过 `AutoCompactor::notify_capacity(...)` 注入 auto-compact 的动态 context capacity 提示。
- 通过 `thread.push_message(...)` 把当前轮 user / assistant / tool 消息并入 live chat。
- 在每次 generate 前估算预算，并按需触发 runtime compact。
- 支持模型主动调用 `compact`，也支持系统被动 compact。
- 将文本输出、工具调用、工具结果实时事件化，而不是只在结尾返回一次结果。
- 把工具审计事件先挂到 `pending_tool_events`，等 turn 落盘时再绑定。

## 使用方式

- Loop 的主入口应该消费 `ThreadContext + 当前 incoming + event sender`。
- Loop 不直接拼 feature prompt；它只判断当前 feature state，并触发固定 provider rebuild。
- Loop 不为动态预算信息保留固定 slot；预算变化后只刷新 `live_system_messages`。
- 当前轮 user message 在 rebuild 后追加到 live chat，再由零参 `ThreadContext.messages()` 统一导出最终请求消息。
- Router 不负责拼 message 列表；`MessageContext` 只剩 deprecated 兼容入口。
- 只要某个线程级状态要跨轮保留，就应写回 `ThreadContext` 再由 Session 层持久化。
