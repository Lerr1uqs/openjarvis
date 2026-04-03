## Context

当前 `MessageContext` 是一个面向请求组装的三段式 DTO：`system`、`memory`、`chat`。在旧链路中，worker 会先把 `ThreadContext` 的历史回填到 `context.chat`，再把本轮 user message 追加进去；进入 AgentLoop 后，又重新从 `ThreadContext` 读取历史，只真正用到 `context.chat.last()` 作为当前 user input。

这意味着：

- 线程真实历史已经由 `ThreadContext` 持有，但 request 组装仍然依赖额外 DTO 透传。
- `system` prompt 现在是每轮由 worker 重新注入，而不是线程初始化后持有的线程级事实。
- 如果直接把完整 `MessageContext` 原样塞进 `ThreadContext`，会把 request-time 的 memory 和 chat history 再次耦合到一起，破坏 compact 与持久化边界。

因此，这个 change 的目标不是把完整 `MessageContext { system, memory, chat }` 原封不动搬进线程，而是把线程真正稳定的 request context 收口到 `ThreadContext`，并把动态 memory / 运行期 system 指令都改成由 AgentLoop 直接向线程 live messages 注入。

## Goals / Non-Goals

**Goals:**

- 让 `ThreadContext` 持有线程级初始化 request context，首版至少覆盖 system prompt snapshot。
- 让 AgentLoop 主链路只接收 `ThreadContext` 与当前轮 user input，不再接收外部预组装的 `MessageContext`。
- 让 Router 只负责转发当前 user message，不负责操控 message 上下文。
- 明确 request context、conversation history 和 compact source history 的边界，避免 system prompt 或 dynamic memory 进入 turn 持久化与 compact 输入。

**Non-Goals:**

- 不在这个 change 中重做 memory 子系统；memory 仍然是 AgentLoop 运行期可选注入。
- 不要求立刻删除 `MessageContext` / `ContextMessage` 类型本身；它们保留为 deprecated 兼容入口。
- 不在这个 change 中改变 compact 的摘要格式、策略选择或 tool 可见性规则。

## Decisions

### 1. 引入线程级 request context，而不是把完整 `MessageContext` 持久化进线程

系统将为 `ThreadContext` 增加一个线程级 request context 概念。首版该上下文只固化线程初始化时的 system prompt snapshot，并作为后续请求组装的稳定前缀事实来源。

之所以不直接把完整 `MessageContext` 放进线程，是因为：

- `chat` 已经由 `ThreadConversation` 持有，再复制一份会继续制造双份事实来源。
- `memory` 是 request-time 动态注入，不适合作为线程初始化快照固化。
- 完整 `MessageContext` 入线程后，会让 compact 和 turn 落盘更容易误把 transient prefix 当作 chat history 处理。

被拒绝的方案：

- 方案 A: 把 `MessageContext { system, memory, chat }` 原样持久化到 `ThreadContext`
  原因: 混淆线程事实与请求临时组装数据，且会把 dynamic memory 与 chat history 耦合到一起。

### 2. AgentLoop 改为消费 `ThreadContext + current incoming`

worker 在热路径中不再构造 `MessageContext`。新的主链路改为：

1. 线程创建或恢复后，`ThreadContext` 已持有自己的 request context snapshot。
2. router / worker 只传递当前 `incoming` 与目标 `ThreadContext`，不再额外透传 `thread/history/loaded_toolsets` 这类可从线程宿主反推的数据。
3. AgentLoop 在请求前通过 `thread.push_message(...)` 统一注入：
   - active memory
   - 当前 incoming 对应的 user input
   - 当前轮 runtime system 指令

其中 `run_v1` 作为主入口直接接收 `event sender + incoming + ThreadContext`。最终发给 LLM 的消息序列不再由 loop 里的独立 helper 组装，而是由零参 `ThreadContext.messages()` 统一导出，保证消息导出边界和线程事实绑定在同一个宿主里。

这样可以把 request 组装逻辑收口到真正拥有线程事实的宿主中，而不是让 worker 和 loop 分别拼一半。

被拒绝的方案：

- 方案 B: 继续由 worker 传完整 `Vec<ChatMessage>`
  原因: 虽然比 `MessageContext` 少一层 DTO，但 request 组装边界仍然分散在 loop 外部。

### 3. request context 不属于 conversation history，也不属于 compact source history

线程级 request context 是线程元数据，不是 `ConversationTurn`。它不会进入 `ThreadContext.load_messages()` 的返回值，也不会作为 stored turn 的普通消息落盘。由 AgentLoop 注入的 active memory 和运行期 system 指令虽然会进入线程 live messages，但也不会进入持久化 turn 或 compact source。

对应地，compact 仍然只作用于 conversation chat history：

- request context 中的 system prompt snapshot 不进入 compact source
- request-time 注入的 memory 也不进入 compact source
- compact 后替换的是 thread conversation 的 active chat history，不是线程级 request context

被拒绝的方案：

- 方案 C: 把 system prompt 前缀预写入每个 turn
  原因: 会造成历史重复和 budget 膨胀，也会把 prefix 错误纳入 compact 边界。

## Risks / Trade-offs

- [Risk] 线程初始化时固化的 system prompt 之后如果发生全局升级，老线程会继续使用旧 snapshot
  Mitigation: 把 prompt 更新视为显式迁移问题，而不是静默覆盖旧线程快照。

- [Risk] 未来 memory provider 接入后，如果没有统一 live thread 注入点，可能再次把 memory 塞回 Router 或线程初始化上下文
  Mitigation: 在 spec 中明确 memory 只能由 AgentLoop 在运行时注入，不属于 Router，也不属于线程初始化 request context。

- [Risk] 现有测试大量依赖 `build_context`
  Mitigation: 迁移时保留兼容 builder 或测试 helper，并新增基于 `ThreadContext + user input` 的 UT。

## Migration Plan

1. 在线程模型中增加线程级 request context，并补齐初始化、恢复和持久化语义。
2. 调整 worker / AgentLoop 主链路，让 loop 只接收 `ThreadContext + current incoming`。
3. 更新 compact 输入边界，确保 request context 与 active memory 不进入 conversation history 和 compact source。
4. 将最终 LLM messages 导出收敛到零参 `ThreadContext.messages()`，并把 `MessageContext` 改成 deprecated 兼容路径。

## Open Questions

- 首版线程级 request context 是否只保存单条默认 system prompt，还是直接允许多条 system messages snapshot。
