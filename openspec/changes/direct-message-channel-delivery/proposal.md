## Why

当前实现为了支持按消息级发送，把 dispatch item、ack cursor 和恢复账本都塞进了 `Thread`，同时又让 router/session 参与发送确认。这导致 thread 锁被错误地扩展成了整轮执行期的共享锁，和“thread 负责消息事实，agent 负责执行和发事件”的边界冲突。

现在需要把发送语义进一步收紧为：`agent.commit_message(...) = thread.push_message(...) + send event`。也就是说，`Thread` 在 `push_message(...)` 内完成消息写入与持久化，agent 在 commit 成功后立刻把 committed event 发到消息信道；session/router 不再维护 dispatch 账本或发送确认状态。

## What Changes

- **BREAKING** 移除 `Thread` 和 `Session` 中的 dispatch ledger、ack cursor、pending dispatch turn 等发送账本概念，不再持久化“哪些消息已发送”这类 dispatch 状态。
- **BREAKING** 用户可见文本、tool call、tool result、terminal failure 改为在 `thread.push_message(...)` commit 成功后，由 agent 立即经消息信道逐条发送 committed event；系统不再维护中断后的自动恢复/补发语义。
- 为 `Thread` 增加 thread-scoped commit persistence 语义，使 `push_message(...)` 自带持久化；session 只作为该持久化能力的基础设施提供方，而不是 dispatch 管理者。
- 调整 `AgentLoop`、`Worker`、`Router`、`Session` 的协作边界，使 thread 锁只覆盖初始化、消息 commit、turn finalization 等事实写入点；发送事件是 agent 的职责，router 不再回写 thread。
- 保留 turn finalization，但其职责收缩为 turn 完成态、审计、external-message dedup 与失败收尾；它不再承担消息外发或发送确认聚合职责。

## Capabilities

### New Capabilities
- `message-channel-delivery`: 定义基于消息信道的逐条发送、agent 事件发射职责与 committed message identity 语义。

### Modified Capabilities
- `thread-context-runtime`: 调整 thread 的锁边界、turn working set ownership 与 turn finalization 语义，明确 router 不再回写 thread dispatch 状态。

## Impact

- 影响代码路径：`src/thread.rs`、`src/agent/agent_loop.rs`、`src/agent/worker.rs`、`src/router.rs`、`src/session.rs` 及对应测试。
- 影响恢复语义：系统不再尝试对未发出的中间 event 做自动恢复或补发；重新请求时依赖已有消息上下文继续执行。
- 影响运行时并发模型：worker 不再应整轮持有 thread 锁，router 也不再通过 session mutate thread。
- 影响当前 `message-level-event-dispatch` 的实现方向；后续实现需要删掉其中引入的 thread dispatch 账本路径。
