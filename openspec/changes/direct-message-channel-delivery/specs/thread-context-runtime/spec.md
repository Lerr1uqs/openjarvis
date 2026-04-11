## MODIFIED Requirements

### Requirement: Agent loop SHALL 基于 `ThreadContext + current user input` 组装请求
系统 SHALL 让 AgentLoop 主链路接收 `ThreadContext` 与当前轮 user input，而不是接收由外部预组装的 `MessageContext`。发送给 LLM 的 messages SHALL 通过 `ThreadContext.messages()` 从线程内部统一导出，AgentLoop SHALL 通过 `ThreadContext.push_message(...)` 注入当前轮 user input 与 turn 内正式消息，而 SHALL NOT 在普通请求轮次中自动注入 active memory 正文、摘要或其他 memory recall message。当前 turn 中一旦有用户可见项完成 commit，agent 就 SHALL 在 `thread.push_message(...)` 成功后立刻发出 committed event；router SHALL NOT 在转发过程中主动操控 memory、重组消息上下文或回写 thread/session 发送状态。

#### Scenario: worker 只传当前 user input
- **WHEN** worker 准备把某个线程请求交给 AgentLoop
- **THEN** worker 不需要先构造完整的 `MessageContext`
- **THEN** AgentLoop 会基于目标 `ThreadContext` 和当前 user input 自行组装本轮 LLM request
- **THEN** AgentLoop/agent 会在消息 commit 成功后自行发出 committed event
- **THEN** Router 不会在转发过程中主动操控 memory、其他 message 上下文或 thread/session 发送状态

## ADDED Requirements

### Requirement: `Thread.push_message(...)` SHALL 自带持久化语义
系统 MUST 让 `Thread.push_message(...)` 成为一个完整的 commit 边界：消息写入 thread 后，`push_message(...)` 在成功返回前 SHALL 已经完成对应持久化。`Thread` MAY 通过 runtime attachment 使用抽象的 persistence capability，但 SHALL NOT 依赖具体 `SessionManager` 实现。

#### Scenario: push_message 成功返回代表消息已落盘
- **WHEN** AgentLoop 调用 `Thread.push_message(...)` 并收到成功结果
- **THEN** 这条消息已经成为 thread 的正式内容并完成持久化
- **THEN** agent 可以在锁外安全地继续发出这条消息的 committed event

### Requirement: turn 生命周期接口 SHALL NOT 承担即时持久化
系统 MUST 将 `open_turn(...)`、`begin_turn(...)`、`finalize_turn_success(...)` 和 `finalize_turn_failure(...)` 视为本地 turn 生命周期接口。它们 SHALL NOT 像 `push_message(...)` 那样承担“正式消息写入后立即持久化”的职责；turn 的最终快照提交由回合结束后的 finalized turn commit 负责。

#### Scenario: begin_turn 只建立本地生命周期状态
- **WHEN** thread 调用 `begin_turn(...)`
- **THEN** 系统只会建立新的 active turn 生命周期状态
- **THEN** 该调用不会像 `push_message(...)` 一样触发正式消息即时持久化

### Requirement: Thread 变更 SHALL 只发生在事实提交边界
系统 MUST 将 thread 的即时持久化边界限定在正式消息 commit。worker SHALL NOT 在整轮请求执行期间长期持有 thread 锁，router 也 SHALL NOT 获取 thread 锁。

#### Scenario: message commit 之外不长期占用 thread 锁
- **WHEN** AgentLoop 正在进行一次普通请求轮次，期间尚未发生新的 thread commit
- **THEN** worker 不会因为“等待后续 LLM/tool 执行”而持续占用目标 thread 锁
- **THEN** 后续消息外发不依赖这把 thread 锁

### Requirement: turn 生命周期接口 SHALL NOT 承担 event buffer 语义
系统 MUST 将 `open_turn(...)`、`finalize_turn_success(...)` 和 `finalize_turn_failure(...)` 视为纯 turn 生命周期接口。它们 SHALL 只负责开启/关闭 turn、写入完成态和记录日志，而 SHALL NOT 维护 turn event buffer、pending dispatch、failure 兜底消息或其他用户可见消息生成逻辑。

#### Scenario: open_turn 只开启 turn 不产生消息
- **WHEN** thread 调用 `open_turn(...)` 或 `begin_turn(...)`
- **THEN** 系统只会建立新的 active turn 生命周期状态
- **THEN** thread 不会因为开启 turn 而自动产生任何用户可见消息或 event

#### Scenario: finalize_turn_success 只关闭 turn 不追加消息
- **WHEN** thread 调用 `finalize_turn_success(...)`
- **THEN** 系统只会结束当前 turn 并记录成功完成态
- **THEN** thread 不会因为 finalize 成功而自动补齐任何 turn event 或用户可见消息

### Requirement: Thread 与 Session SHALL NOT 持有 dispatch 发送账本状态
线程上下文 SHALL 只保存 request context、conversation history、active turn working set、turn 完成态与 dedup 相关事实，而 SHALL NOT 保存 router ack cursor、pending dispatch ledger 或其他消息送达进度状态。Session SHALL 只作为 thread commit persistence 与 dedup 的基础设施，而 SHALL NOT 承担 dispatch/checkpoint 管理职责。

#### Scenario: thread snapshot 中不再包含待发送账本
- **WHEN** 某个 turn 已经 commit 了若干消息，但外发尚未全部完成
- **THEN** thread snapshot 只反映这些消息已经成为当前 turn 的正式内容
- **THEN** session 和 thread 中都不会额外出现 dispatch/checkpoint 字段来追踪“哪些消息已经送达”
