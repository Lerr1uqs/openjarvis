# Thread

## 定位

- `Thread` 是线程级句柄和正式事实写入边界。
- 它负责通过精炼接口驱动线程身份、正式消息和线程级非消息状态的原子持久化。
- 它不负责平台入口 dedup，不负责 Router 编排，也不把 request-time 临时状态暴露成公共结构体。

## 严格边界

- `PersistedThreadSnapshot` 只负责持久化消息序列、线程状态和 revision。
- 其中稳定前缀直接表现为持久化消息序列开头的一组 `System` messages，而不是额外的 request context 成员。
- `ActiveRequestState` 只负责请求期临时状态，例如日志锚点、临时工具审计缓冲和串行约束。
- request-time 临时状态可以存在于 `Thread` 内部，但不能形成公共 turn 结构或持久化结构。
- `Turn` 只保留为日志/事件概念，不再作为公共结构体或主存储结构。

## 关键概念

- `ThreadContextLocator`
  线程的稳定定位信息，包含 `session_id`、`channel`、`user_id`、`external_thread_id`、`thread_id`。
- `thread_key`
  归一化线程键，格式固定为 `user_id:channel:external_thread_id`。
- `PersistedThreadSnapshot`
  线程正式持久化事实，包含持久化消息序列、线程状态和 revision。
- `ThreadState`
  线程级 feature flags、loaded toolsets、tool event、approval 状态。
- `ActiveRequestState`
  线程内部的请求期临时状态，不持久化、不对外暴露。

## 消息模型

- `Thread.messages()`
  当前线程全部正式消息。
- 稳定 `System` messages 在 `init_thread()` 时一次性注入到 `Thread` 并持久化。
- 这些稳定 `System` messages 必须位于持久化消息序列前缀。
- `Thread.push_message(...)`
  成功返回即代表该消息已经完成持久化。

## 初始化 Ownership

- `init_thread()` 属于 worker，不属于 `AgentLoop`。
- worker 在进入 live loop 前准备 feature/tool registry，并构造稳定 system messages。
- 初始化后的 system messages 直接持久化进 `Thread`，之后 loop 只消费已初始化线程。
- 初始化如果改动了线程，必须立即通过 thread-owned 持久化入口写回。

## compact 边界

- compact 的输入边界是 message 序列，不是 turn slice。
- 主链路只 compact 全部非 `System` message。
- compact 写回时保留持久化 `System` 前缀，只替换非 `System` 历史。
- compact 是否执行由调用方决定，`Thread` 本身只提供消息读写边界。

## Turn 概念

- `Turn` 仍然表示“一次用户输入驱动的一轮执行”。
- 程序只按 message 和 thread state 进行正式落盘；`Turn` 只是日志和事件概念，不再有公共数据结构概念。

## 核心能力

- 根据 `channel + user_id + external_thread_id` 派生稳定 `thread_id`。
- 以 message 为最小持久化单位保存线程历史。
- 以同样的原子写入模型持久化线程级 toolset 状态和 tool event 审计信息。
- 在清空线程时保留线程身份，只重置消息和线程状态。

## 验收标准

- `Thread` 对外接口小而少，主链路只暴露消息写入、状态写入和正式消息读取边界。
- `Thread` 不暴露 turn/finalized snapshot 结构，也不承担平台 dedup。
