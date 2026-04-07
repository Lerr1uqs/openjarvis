# Thread

## 定位

- `Thread` 是线程级持久化聚合。
- 它负责保存线程身份、持久化消息和线程级非消息状态。
- 它不负责 LLM 调用、不负责 Router 编排，也不负责 request-time live working set。

```rust
pub struct Thread {
   pub locator: ThreadContextLocator,
   pub thread: ThreadContext,
   pub state: ThreadState,
}
```

## 严格边界

- `ThreadContext` 只负责持久化消息域。
- `ThreadState` 只负责 feature/tool/approval 这类非消息状态。
- request-time 临时消息只属于 `AgentLoop` 局部执行期，不属于 `Thread` 模型。
- `Turn` 只保留为提交概念，不再作为主存储结构。

## 关键概念

- `ThreadContextLocator`
  线程的稳定定位信息，包含 `session_id`、`channel`、`user_id`、`external_thread_id`、`thread_id`。
- `thread_key`
  归一化线程键，格式固定为 `user_id:channel:external_thread_id`。
- `ThreadContext`
  持久化消息序列，以及 `created_at` / `updated_at`。
- `ThreadState`
  线程级 feature override、loaded toolsets、tool event、approval 状态。

## 消息模型

- `Thread.thread.messages`
  当前线程全部持久化消息。
- 稳定 `System` messages 在 `init_thread()` 时一次性注入到 `Thread` 并持久化。
- 这些稳定 `System` messages 必须位于持久化消息序列前缀。
- `Thread::messages()`
  返回全部持久化消息。

## 初始化 Ownership

- `init_thread()` 属于 worker，不属于 `AgentLoop`。
- worker 在进入 live loop 前准备 feature/tool registry，并构造稳定 system messages。
- 初始化后的 system messages 直接持久化进 `Thread`，之后 loop 只消费已初始化线程。
- 初始化如果改动了线程，必须立即同步到 session/store。

## compact 边界

- compact 的输入边界是 message 序列，不是 turn slice。
- 主链路只 compact 全部非 `System` message。
- compact 写回时保留持久化 `System` 前缀，只替换非 `System` 历史。
- compact 是否执行由调用方决定，`Thread` 本身只提供消息读写边界。

## Turn 概念

- `Turn` 仍然表示“一次用户输入驱动的一轮提交”。
- 程序只按message进行落盘和发送 `Turn`只是事件概念 不再有数据结构概念

## 核心能力

- 根据 `channel + user_id + external_thread_id` 派生稳定 `thread_id`。
- 以 message 为最小持久化单位保存线程历史。
- 持久化线程级 toolset 状态和 tool event 审计信息。
- 在清空线程时保留线程身份，只重置消息和线程状态。
