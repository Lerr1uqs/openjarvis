# AgentLoop

## 定位

- `AgentLoop` 是单轮 Agent 执行核心。
- 它维护这一轮里的 `generate -> tool -> generate` 循环，直到模型不再请求工具。

## 边界

- 负责 prompt 组装、可见工具计算、工具执行、compact、hook 触发、事件生成。
- 不负责请求排队，不负责 session 持久化，不负责 channel 出站。

## 关键概念

- `working_chat_messages`
  当前轮正在工作的 chat 视图，包含线程历史和本轮新增消息。
- `RequestState`
  本次 generate 的消息、工具和预算快照。
- `AgentLoopEvent`
  Router 可直接回发的结构化事件。
- `AgentLoopOutput`
  单轮最终结果，包含 turn messages、thread_context 和元数据。

## 核心能力

- 基于 `ThreadContext` 计算当前线程可见工具。
- 在每次 generate 前估算预算，并按需触发 runtime compact。
- 支持模型主动调用 `compact`，也支持系统被动 compact。
- 将文本输出、工具调用、工具结果实时事件化，而不是只在结尾返回一次结果。
- 把工具审计事件先挂到 `pending_tool_events`，等 turn 落盘时再绑定。

## 使用方式

- Loop 的输入对象应该是 `ThreadContext`，不是零散的 history/toolset 参数。
- 只要某个线程级状态要跨轮保留，就应写回 `ThreadContext` 再由 Session 层持久化。
