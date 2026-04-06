## Context

当前主链路里，消息 ownership 被拆在三处：

- `Thread` 只保存已持久化历史；
- `AgentLoop` 在本轮内额外维护 `request_system_messages`、`live_chat_messages`、`commit_messages`；
- `Router/Session` 再根据成功或失败路径重新组装要持久化和要发送的消息。

这会导致三个边界脱钩：

- 本轮 `llm.generate(...)` 看到的消息视图；
- 用户在 channel 中感知到的输出；
- 最终线程里真正落盘的消息状态。

用户本轮要求把这三者收敛到同一个 owner，并把 `Turn` 变成“线程持久化边界 + 对外发送边界”。这意味着本次 change 不只是 loop 内部重构，而是要重写 `Thread`、`AgentLoop`、`Worker`、`Router`、`Session` 之间的消息 ownership 契约。

## Goals / Non-Goals

**Goals:**

- 让 `Thread` 成为当前 turn 消息的唯一 owner。
- 定义 `Turn` 为“一次输入及其对应的全部输出”，而不是内部单次 loop 迭代。
- 让 `Turn` 成为线程持久化与 channel 派发的共享边界。
- 让 `Router` / `Session` 停止组装 user / assistant / tool / error 消息。
- 让 `init_thread()` 成为稳定 system / feature prompt 的唯一注入入口，并从 loop 中移除 `request_system_messages`。
- 让 compact 与 auto-compact 面向 thread-owned active message view，而不是 loop 局部消息集合。

**Non-Goals:**

- 本次不做“每次 `thread.push(...)` 都立即 durable checkpoint”的持久化策略；turn finalization 仍是正式持久化边界。
- 本次不重做 channel 协议或 LLM/tool provider 协议，只调整主链路 ownership。
- 本次不在 `model/**` 架构文档中直接落地变更；先通过 openspec 收敛边界。
- 本次不决定 memory 命中内容在 channel 中是否需要单独可见，只要求 ownership 归 `Thread`。

## Decisions

### 1. `Thread` 引入 turn-owned working set，成为唯一消息 owner

系统将把当前 turn 的 user input、assistant 输出、assistant tool-call message、tool result、compact replacement 和 turn 内待发送事件都收口到 `Thread` 管理，而不是继续放在 `AgentLoop` 的局部 `Vec` 中。

新的边界是：

- `Thread` 继续持有稳定 system prefix 与已持久化历史；
- `Thread` 额外持有当前 turn 的 working set / buffered outputs；
- `Thread.messages()` 导出当前请求视图；
- turn 成功或失败时，由 `Thread` 产出唯一的 finalized thread snapshot 与 turn 级结果边界。

拒绝的方案：

- 方案 A：保留 `AgentLoop` 的 `live_chat_messages` / `commit_messages`，只是换个结构体包装
  原因：ownership 仍然停留在 loop 之外，`Router` / `Session` 依旧需要根据增量消息重建最终状态。

### 2. `Router` / `Session` 只消费 turn 级结果，不组装消息

系统将把成功与失败路径都改为：

1. `AgentLoop` 直接改写 `Thread`
2. `Thread` 在 turn finalization 时产出 finalized thread snapshot
3. `AgentLoop` 在同一 turn 范围内收集并导出 turn 级 event batch
4. `Router` 只负责派发该 turn event batch，并把对应 thread snapshot 交给 `Session`
5. `Session` 只负责保存 thread snapshot 与 dedup record

`Router` SHALL NOT 再拼装 user / assistant / tool / error 消息，也 SHALL NOT 再根据 `commit_messages` append 历史；它只转发 loop 已经完成的 turn 级事件结果。

拒绝的方案：

- 方案 B：保留当前 `commit_messages_with_thread_context(...)` 风格，由 router 继续组装提交消息
  原因：这样最终 thread 历史仍然有两套来源，无法满足“消息都必须让 thread own 住”。

### 3. turn finalization 是对外发送与持久化的唯一节拍

系统将把文本输出、tool 事件、compact 事件从“逐个完成立即发送”改成“由 loop 在当前 turn 内收集 event batch，turn 结束后统一派发”。这里的 `Turn` 指的是“一次用户输入驱动的完整 ReAct 执行”，即可能包含多次内部 `generate -> tool -> generate` 迭代，但对外只算一个 turn。

这保证：

- 用户看到的结果与持久化 thread 状态一致；
- channel 不会看到一串未提交的中间 event；
- 失败 turn 不会出现“用户看到了中间消息，但 thread 没有保存”的偏差。

这里的关键不是把所有消息合并成一条，而是把“可见性时机”和“持久化时机”统一到 turn 结束；loop 仍然可以产出结构化 event，只是改为 turn 级 batch。

拒绝的方案：

- 方案 C：保留当前逐 event 立即发送，但把 thread 也同步改写
  原因：一旦 turn 后续失败，仍然会出现“部分 event 已发出，但最终持久化语义不清晰”的问题。

### 4. 全部 system message 只通过 `init_thread()` 注入；loop 移除 `request_system_messages`

系统将保留 `init_thread()` 作为稳定 system prompt / feature system message 的唯一注入入口，并要求：

- 正常线程在产生任何 chat message 前，必须已经拥有 system prefix；
- 所有 system messages 都位于 thread 开头前缀；
- `AgentLoop` 内不再维护 `request_system_messages`。

这意味着当前 `AutoCompactor` 注入的动态容量 prompt 不能继续作为 loop 局部 transient system message 存在。本次设计选择：

- 保留稳定的 auto-compact 能力说明作为 feature prompt；
- 移除面向模型的 transient 容量 prompt；
- 不再为 `request_system_messages` 引入替代性的 runtime system message 容器；
- compact tool 的可见性仍可由运行时预算判断控制，但不再依赖额外的 loop-local system message。

拒绝的方案：

- 方案 D：把动态容量 prompt 直接持久化进 thread 历史
  原因：预算数值是高频变化信息，会污染历史并破坏稳定前缀。
- 方案 E：保留 `request_system_messages` 或新建等价 runtime system 容器仅服务 auto-compact
  原因：这与本次“thread 独占消息 ownership”目标冲突。

### 5. compact 改为直接改写 thread-owned active message view

runtime compact 与模型主动触发 `compact` 时，都改为读取 thread-owned active message view：

- 输入范围是 thread 的 active non-system messages；
- 其中既包括已持久化的 non-system history，也包括当前 turn 中已进入请求视图的 non-system messages；
- compact 完成后直接回写 `Thread` 的 active view，而不是同时清理外部 `live_chat_messages` / `commit_messages`。

稳定 system prefix 永远不进入 compact source。

拒绝的方案：

- 方案 F：继续把 compact 输入定义为 `thread + live_chat_messages`
  原因：这仍然依赖 loop 外部 working set，无法收口 ownership。

### 6. 中途中断模型遵从 turn 边界，而不是逐 push durable

本次 change 明确选择“turn 是正式持久化边界”。因此：

- turn 运行过程中，`Thread` 可以拥有 pending turn state；
- 但只有 turn finalization 后，thread snapshot 才进入正式持久化与 dedup；
- 如果 worker / 进程在 turn 中途崩溃，pending turn state 不保证跨进程恢复。

这个 trade-off 与“用户感知必须和持久化状态一致”的要求一致，但它不解决“中途工具执行后立即 durable checkpoint”的问题；后者可以作为未来单独 change。

拒绝的方案：

- 方案 G：每次 `thread.push(...)` 都立即保存 store
  原因：这会把 turn 边界重新打散，并引入更复杂的中途失败/回滚语义。

## Risks / Trade-offs

- [Risk] turn 中途崩溃会丢失 pending turn state
  Mitigation: 在 spec 中明确 turn-final persistence 边界，并把 mid-turn durable checkpoint 留给后续 change。

- [Risk] 移除 transient 容量 prompt 后，模型对 auto-compact 的主动感知会变弱
  Mitigation: 保留稳定的 compact 能力说明，并继续用 runtime 阈值自动触发 compact。

- [Risk] `Router` / `Session` 提交接口会发生较大变化，revision merge 与 dedup 逻辑容易回归
  Mitigation: 将 thread snapshot 持久化与 dedup 绑定为同一批提交语义，并补全 router/session/thread UT。

- [Risk] “turn 结束后统一派发”会改变现有 channel 侧体验
  Mitigation: 在 spec 中把行为变化定义为 BREAKING，并在实现阶段补明确的 channel 回归测试。

## Migration Plan

1. 为 `Thread` 设计 turn-owned working set、thread snapshot finalization 与 turn 级结果 API，收敛当前 turn 的消息 ownership。
2. 重写 `AgentLoop`，移除 `request_system_messages`、`live_chat_messages`、`commit_messages`，改为直接读写 `Thread`。
3. 重写 `Worker` / `Router` / `Session` 的成功与失败路径，让它们只保存 finalized thread snapshot，并派发同一 turn 对应的 event batch。
4. 将 compact 和 auto-compact 改为面向 thread-owned active view；删除 loop-local dynamic capacity prompt。
5. 更新 agent/router/session/compact/thread 测试，覆盖 turn-buffered dispatch、snapshot persistence 与失败 turn 一致性。

## Open Questions

- memory 命中内容在 thread-owned current turn 中是否需要单独的 request-only slot，还是直接复用统一 turn message 序列？
- failed turn 的最终结果是否需要保留部分中间 tool 结果，还是统一坍缩成 thread 决定的一条失败回复？
