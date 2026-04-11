## MODIFIED Requirements

### Requirement: turn 生命周期接口 SHALL NOT 形成持久化身份
系统中的 active turn 只属于本地执行期生命周期。`begin_turn(...)`、`open_turn(...)`、`finalize_turn_success(...)` 和 `finalize_turn_failure(...)` MAY 维护本地执行期状态并输出日志，但 SHALL NOT 生成、暴露或持久化稳定 `turn_id`。

#### Scenario: thread snapshot 中不包含 active turn identity
- **WHEN** 某条消息通过 `push_message(...)` 成功持久化
- **THEN** 写入 store 的 thread snapshot 中不会包含 active turn 的稳定 identity
- **THEN** 后续重新加载 thread 时，只能看到已提交的正式消息和 state，而不会看到可恢复的 turn 身份

### Requirement: external message dedup SHALL NOT 记录 turn 关联
系统对 external message 的 dedup 只表达“某条 external message 是否已经完成处理”。session/store SHALL NOT 为 dedup 记录额外保存 `turn_id`。

#### Scenario: dedup 只记录完成时间
- **WHEN** 某个 external message 被标记为已完成处理
- **THEN** store 只会记录该 message 对应的 thread、external_message_id 和 completed_at
- **THEN** dedup 记录中不会保留 `turn_id`
