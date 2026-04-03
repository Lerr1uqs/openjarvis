## Why

当前 worker 和 AgentLoop 仍然通过 `MessageContext` 传递一次临时请求上下文，其中 `chat` 历史和 `ThreadContext.load_messages()` 重复，导致线程事实与 request 组装边界分裂。现在 `ThreadContext` 已经是线程运行时唯一宿主，需要让线程自己持有初始化后的 request context，使主链路每轮只需要提供当前 user input。

## What Changes

- 在线程初始化时，为 `ThreadContext` 建立线程级 request context snapshot；首版固化 system prompt snapshot。
- Router / `AgentWorker` 不再为 AgentLoop 构造 `MessageContext`，只传当前轮 user input 和目标 `ThreadContext`。
- AgentLoop 在发送 LLM 请求前，通过 `ThreadContext.push_message(...)` 注入 active memory、当前 user input 和 runtime instructions，再由零参 `ThreadContext.messages()` 统一导出 messages。
- 线程级 request context 与 conversation history 显式分层；它不会作为普通 turn 落盘，也不会被当作 compact 的 chat 输入。
- `MessageContext` / `ContextMessage` 类型先保留为兼容和辅助结构，但退出 worker / agent loop 热路径并标记为 deprecated。

## Capabilities

### New Capabilities

无。

### Modified Capabilities

- `thread-context-runtime`: 为线程引入初始化后的 request context，并将 agent 请求组装入口调整为 `ThreadContext + current user input`。

## Impact

- Affected code: `src/thread.rs`、`src/agent/worker.rs`、`src/agent/agent_loop.rs`、`src/context/mod.rs` 以及对应测试。
- API impact: AgentLoop 与 worker 主链路的入参和调用关系会调整，不再依赖 `MessageContext` 透传完整请求上下文。
- Runtime impact: system prompt 将变成线程级初始化快照；memory 仍保持 AgentLoop 运行期动态注入，而不是 Router 预组装或线程持久化 chat history。
