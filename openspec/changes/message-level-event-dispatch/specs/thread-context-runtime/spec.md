## MODIFIED Requirements

### Requirement: Agent loop SHALL 基于 `ThreadContext + current user input` 组装请求
系统 SHALL 让 AgentLoop 主链路接收 `ThreadContext` 与当前轮 user input，而不是接收由外部预组装的 `MessageContext`。发送给 LLM 的 messages SHALL 通过 `ThreadContext.messages()` 从线程内部统一导出，AgentLoop SHALL 通过 `ThreadContext.push_message(...)` 注入当前轮 user input 与 turn 内正式消息，而 SHALL NOT 在普通请求轮次中自动注入 active memory 正文、摘要或其他 memory recall message。对外发送语义不再绑定到 turn finalization；凡是已经进入 `Thread` 当前 turn state 的单条 committed message / event，都可以在 turn 未结束时由外部模块按顺序消费并发送。

#### Scenario: worker 只传当前 user input
- **WHEN** worker 准备把某个线程请求交给 AgentLoop
- **THEN** worker 不需要先构造完整的 `MessageContext`
- **THEN** AgentLoop 会基于目标 `ThreadContext` 和当前 user input 自行组装本轮 LLM request
- **THEN** Router 不会在转发过程中主动操控 memory 或其他 message 上下文

#### Scenario: committed turn message 可在 finalization 前被发送
- **WHEN** AgentLoop 在当前 turn 内把 assistant 文本或 tool result 写入 `Thread`
- **THEN** 该 message / event 已经成为 `Thread` 当前 turn state 的正式组成部分
- **THEN** 外部模块可以在 turn finalization 之前按顺序发送它

#### Scenario: tool call 与 tool result 不需要等成对
- **WHEN** AgentLoop 已经把某个 tool call 写入 `Thread`，但对应 tool result 尚未产生
- **THEN** 外部模块可以先发送这条 tool call item
- **THEN** 后续 tool result 会在写入 `Thread` 后作为另一条独立 item 单独发送

### Requirement: 稳定 `System` 前缀、conversation history 与当前 turn working set SHALL 明确分层
稳定 `System` 前缀 SHALL NOT 作为普通 `ConversationTurn` 消息重复落盘，也 SHALL NOT 被视为普通 chat history 的一部分。线程恢复后，系统 SHALL 可以分别恢复稳定 `System` 前缀、conversation history 以及当前 turn 的 in-progress working set / dispatch checkpoint，而不是把这些边界混成一条扁平消息序列。

#### Scenario: 稳定前缀不进入 turn 历史
- **WHEN** 某个线程完成一轮普通 user / assistant 对话并被持久化
- **THEN** 落盘的 `ConversationTurn` 中只包含该轮 conversation messages
- **THEN** 稳定 `System` 前缀不会作为重复前缀被写入每个 turn

#### Scenario: 恢复时可以区分最终历史与中间发送进度
- **WHEN** 某个线程在 turn 未结束时已经发送过部分 message / event，随后发生恢复
- **THEN** 系统可以分别读出稳定 `System` 前缀、已持久化 conversation history 和当前 turn 的 dispatch checkpoint
- **THEN** 恢复链路不会把“已发送但未 finalized 的当前 turn 进度”误判为普通历史前缀
