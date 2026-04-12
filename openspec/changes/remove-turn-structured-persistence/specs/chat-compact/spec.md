## REMOVED Requirements

### Requirement: 系统 SHALL 将 compact 结果写回为一个 compacted turn
**Reason**: compact 产物不再以 turn 结构持久化；本次变更要求 compact 直接改写线程正式消息序列。
**Migration**: 将 compact 输出改为一组直接写回线程消息序列的 compacted messages，继续保留 `assistant` summary 与 follow-up `user` message 的语义。

### Requirement: compacted turn SHALL 参与后续 compact
**Reason**: 后续 compact 仍会复用 compact 产物，但参与对象已经不再是 turn，而是普通正式消息。
**Migration**: 将“旧 compacted turn 参与后续 compact”迁移为“旧 compacted messages 参与后续 compact”。

## ADDED Requirements

### Requirement: 系统 SHALL 将 compact 结果写回为一组 compacted messages
系统 SHALL 将 compact 结果直接写回为线程正式消息序列中的一组 compacted messages，而不是写入 compacted turn。首版 compact 输出 SHALL 继续包含两条普通 chat message：
- compacted `assistant` message：明确说明“这是压缩后的上下文”，并保留任务目标、用户约束、当前背景、当前规划、已完成、未完成和关键事实；
- follow-up `user` message：固定写入“继续”，让后续对话自然续接。

#### Scenario: compact 输出直接成为正式消息
- **WHEN** 当前线程完成一次 compact
- **THEN** 系统会把 compacted `assistant` message 和 follow-up `user` message 直接写入线程正式消息序列
- **THEN** 这两条 message 不会再被包装成任何 turn 结构

### Requirement: compacted messages SHALL 参与后续 compact
系统 SHALL 将 compact 生成的 compacted messages 视为后续 chat history 的一部分。在未来再次 compact 时，已有 compacted messages SHALL 和后续新增 chat 一起参与压缩。

#### Scenario: 再次 compact 会包含旧 compacted messages
- **WHEN** 某个线程已经存在 compacted messages 且后续又产生新的 chat history
- **THEN** 下一次 compact 时，这些 compacted messages 会作为输入的一部分参与新的 compact
- **THEN** 系统不会因为它们来自旧 compact 而把它们当作 turn 边界特殊处理

## MODIFIED Requirements

### Requirement: 首版 SHALL 直接替换被 compact 的 active chat history
首版 compact 完成后，系统 SHALL 从 active chat history 中移除被 compact 的旧 chat messages，并用新的 compacted messages 替换它们。系统设计 SHALL 为未来 archive / shadow copy 策略保留扩展点，但首版 active history 中 SHALL NOT 保留被替换的旧 chat messages。

#### Scenario: 旧 chat 被 compacted messages 替换
- **WHEN** 首版 compact 成功执行
- **THEN** 被 compact 的旧 chat messages 不再出现在 active chat history 中
- **THEN** active chat history 只保留 compacted messages 及其后的新消息
