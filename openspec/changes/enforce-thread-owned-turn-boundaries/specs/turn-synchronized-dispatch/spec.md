## ADDED Requirements

### Requirement: Turn SHALL 表示一次输入及其全部输出
系统 SHALL 将 `Turn` 定义为“一次用户输入及其对应的全部输出”。一个 turn 内可以包含多次内部 `generate -> tool -> generate` loop 迭代，但这些内部迭代对外都属于同一个 turn。

#### Scenario: 多次内部 loop 迭代仍属于同一个 turn
- **WHEN** 某次用户输入触发多轮 `generate -> tool -> generate` 执行
- **THEN** 这些内部迭代共同构成同一个 turn
- **THEN** 该 turn 的持久化与对外发送边界只在最终 turn 完成时发生

### Requirement: 系统 SHALL 以 finalized turn 作为外发与持久化的共享边界
系统 SHALL 只在当前 turn finalization 完成后，才对外发送 agent 结果并持久化对应 thread snapshot。用户在 channel 中感知到的本轮结果 MUST 与该 finalized turn 对应的持久化 thread 状态一致。

#### Scenario: 成功 turn 在统一边界外发与持久化
- **WHEN** 某个 turn 成功结束并产出 final reply
- **THEN** 系统会先得到同一 turn 对应的 event batch 与 finalized thread snapshot
- **THEN** channel 派发与 thread snapshot 持久化都基于这同一个 turn 边界

### Requirement: AgentLoop SHALL 产出 turn 级 event batch
系统 SHALL 允许 AgentLoop 在 turn 执行期间继续产出结构化 event，但这些 event MUST 先进入当前 turn 的 event batch，而不是每完成一个 event 就立即发送。只有 turn 结束后，这些用户可见结果才可以按 finalized turn 定义的顺序统一对外派发。

#### Scenario: turn 未结束时不会立即发送中间 event
- **WHEN** AgentLoop 在同一 turn 内连续产生文本输出、tool call 与 tool result
- **THEN** 这些用户可见事件会先写入当前 turn buffer
- **THEN** 在 turn finalization 之前，channel 不会收到这些中间 event

### Requirement: Router SHALL 发送 turn 级 event batch 而不是组装消息
Router SHALL 发送由 AgentLoop 产出的 finalized turn event batch，并使用同一 turn 对应的 thread snapshot 进入持久化链路。Router SHALL NOT 在 `Thread` 之外重建 user / assistant / tool / error 消息，也 SHALL NOT 再根据事件流手工拼装最终回复。

#### Scenario: Router 按 turn event batch 派发成功结果
- **WHEN** 某个 turn 已经最终化为成功的 turn event batch 与 thread snapshot
- **THEN** Router 会直接派发该 turn event batch 中定义的用户可见结果
- **THEN** Router 不会重新拼接 assistant 或 tool 相关消息

### Requirement: failed turn SHALL 保持用户可见状态与持久化状态一致
当某个 turn 执行失败时，系统 SHALL 由 `Thread` 基于当前 turn state 生成统一的失败 turn 结果。channel 对用户可见的失败结果与 Session 中持久化的 thread snapshot MUST 来自这同一个失败 turn 边界。

#### Scenario: failed turn 不发送未提交的部分结果
- **WHEN** 某个 turn 在已缓冲部分中间 event 后失败
- **THEN** 系统会由 `Thread` 生成统一的失败 turn 结果
- **THEN** channel 与持久化层看到的是同一个失败结果
- **THEN** 不会出现“用户看到了中间结果，但 thread 没有保存相同状态”的情况

### Requirement: Session SHALL 将 dedup 与 finalized turn snapshot 绑定
Session SHALL 把 external-message dedup record 与 finalized turn snapshot 绑定到同一提交边界。后续同一 external message 的重放判断 MUST 基于已经提交的 finalized turn，而不是未完成 turn 的临时状态。

#### Scenario: finalized turn 提交后重复消息被同一边界拦截
- **WHEN** 某个 external message 对应的 turn 已成功完成提交
- **THEN** Session 中保存的 dedup record 与 thread snapshot 对应同一个 finalized turn
- **THEN** 后续重放该 external message 时，系统会看到与用户上次感知一致的已提交状态
