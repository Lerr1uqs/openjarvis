## Why

当前 agent 主链路把消息 ownership 分散在 `Thread`、`AgentLoop` 的局部 working set，以及 `Router/Session` 的提交组装逻辑之间。这会让当前 turn 的用户可见输出、线程持久化状态和下一轮请求视图不再由同一份 thread state 推导，因此需要把消息管理收口到 `Thread`，并把 `Turn` 定义成统一的持久化与外发边界。

## What Changes

- **BREAKING** `Thread` 成为 agent 执行期消息的唯一 owner；`AgentLoop` 不再维护 `live_chat_messages`、`commit_messages`、`request_system_messages` 这类 loop 局部消息集合。
- **BREAKING** `Router` 和 `Session` 不再组装 user / assistant / tool / error 消息，也不再依据增量 `commit_messages` append 历史；它们只消费 thread snapshot 与 turn 级结果。
- 新增 turn 同步派发能力：`Turn` 被定义为“一次输入及其对应的全部输出”；当前 turn 的文本输出、tool 事件和 compact 事件先进入 turn 级 event batch，只有 turn finalization 时才能对外发送；用户感知结果必须与持久化 thread 状态一致。
- `init_thread()` 负责把全部稳定 system prompt 和 feature system message 注入 thread 开头前缀；loop 中移除 `request_system_messages`，也不再引入替代性的 runtime system message 容器。
- compact 和 auto-compact 改为基于 thread-owned message view 工作，不再依赖 loop 外的临时消息拼装或 transient request system messages。

## Capabilities

### New Capabilities

- `turn-synchronized-dispatch`: 定义 turn 作为“一次输入及其全部输出”的共享派发与持久化边界，确保 turn 级 event batch 与 thread snapshot 一致。

### Modified Capabilities

- `thread-context-runtime`: 线程运行时改为由 `Thread` 独占当前 turn 的 message ownership、请求导出和稳定 system prefix 前缀约束。
- `chat-compact`: compact 改为直接面向 thread-owned active message view 运行，并移除对 transient request system messages 的依赖。

## Impact

- Affected code: `src/thread.rs`、`src/session.rs`、`src/session/store/**`、`src/agent/agent_loop.rs`、`src/agent/worker.rs`、`src/agent/feature/mod.rs`、`src/router.rs`、`src/compact/**` 及对应测试。
- API impact: `AgentLoopOutput`、`Thread` message mutation/export API、router worker 事件载荷、session 提交接口都会调整；外部模块不再传递 `commit_messages` 风格的增量消息。
- Behavior impact: agent 对外消息改为按 turn 边界发送，loop 继续产 event，但粒度从单条消息改为 turn 级 event batch；失败 turn 的用户可见结果与持久化结果必须来自同一个 turn finalization 结果。
