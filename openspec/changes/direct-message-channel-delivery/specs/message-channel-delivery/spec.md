## ADDED Requirements

### Requirement: 系统 SHALL 在消息 commit 后由 agent 经消息信道逐条发送
系统 SHALL 在 assistant 文本、tool call、tool result、terminal failure 等用户可见项完成 thread commit 后，由 agent 立即经消息信道逐条发送 committed event，而 SHALL NOT 先在 thread/session 内累计 dispatch 批次再统一外发。发送顺序 MUST 与消息在当前 turn 内的 commit 顺序一致。

#### Scenario: assistant 文本在 commit 后立即进入消息信道
- **WHEN** AgentLoop 将一条新的 assistant 文本成功 commit 到当前 turn
- **THEN** agent 会把这条 committed 消息立即发送到 router 的消息信道
- **THEN** router 不需要等待 turn finalization 才能开始向 channel 外发这条消息

#### Scenario: tool call 与 tool result 分别按各自 commit 时机发送
- **WHEN** 当前 turn 先 commit 一个 tool call，随后再 commit 对应的 tool result
- **THEN** agent 会让这两个 committed 项按各自 commit 顺序分别进入消息信道
- **THEN** 系统不会把它们重新组装成 turn 级 dispatch batch 再发送

### Requirement: router SHALL 只消费 committed event 而不回写 thread/session 发送状态
router MUST 只消费 agent 发出的 committed event 并向外部 channel 发送，而 SHALL NOT 通过修改 `Thread` 或 `Session` 来记录“哪条消息已经发出”。系统 SHALL NOT 再维护 thread/session 级 dispatch cursor、ack ledger 或 pending dispatch turn。

#### Scenario: router 发送成功后不会回写 thread 或 session
- **WHEN** router 成功向某个 channel 发送了一条 committed 消息
- **THEN** router 不会调用 thread mutation 接口推进任何 dispatch cursor 或 ack 状态
- **THEN** session 中也不会额外记录这条消息的 dispatch/checkpoint 状态

### Requirement: 系统 SHALL NOT 维护 committed event 的自动补发语义
系统 SHALL NOT 为 committed event 维护 dispatch cursor、补发队列或自动恢复语义。若某个请求在中间 event 发出前被打断，系统只保证已经持久化的正式消息仍留在线程上下文中，而不会在后续自动补发之前未送达的中间 event。

#### Scenario: 中断后不会自动补发旧的中间 event
- **WHEN** 某个请求在中途被打断，且部分中间 event 尚未发到外部 channel
- **THEN** 系统不会在下一次请求开始时自动补发这些旧的中间 event
- **THEN** 后续请求会直接基于线程中已经持久化的消息上下文继续执行

### Requirement: failed turn SHALL 以新增失败消息收尾
当一个 turn 在已 commit 并可能已发送部分消息后失败时，系统 SHALL 新增一条 terminal failure committed 消息作为收尾，而 SHALL NOT 通过清空、回滚或撤销此前 committed 消息来表达失败。

#### Scenario: 已发送部分消息后 turn 失败
- **WHEN** 当前 turn 已经 commit 并发送了部分 assistant 文本或 tool 结果，随后执行失败
- **THEN** 系统会再 commit 一条 terminal failure 消息并按同样的消息信道流程发送
- **THEN** 此前 committed 的消息不会因为 turn 失败而从 thread 中被隐式删除

### Requirement: 用户可见消息 SHALL 只来自 agent 的显式 commit
系统 MUST 只让 agent 的显式消息提交产生用户可见 committed event。`open_turn(...)`、`finalize_turn_success(...)` 和 `finalize_turn_failure(...)` SHALL NOT 在内部缓冲、补齐、伪造或隐式追加用户可见消息/event。

#### Scenario: finalize_turn_failure 不会自动补一条失败消息
- **WHEN** 某个 turn 执行失败，但 agent 在 finalize 前没有显式 commit failure message
- **THEN** `finalize_turn_failure(...)` 只会结束 turn 并记录失败状态
- **THEN** 系统不会因为 finalize 而自动多出一条用户可见 failure event
