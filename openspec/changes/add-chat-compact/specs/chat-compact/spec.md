## ADDED Requirements

### Requirement: 系统 SHALL 对线程 `chat` 历史执行 token 感知的 runtime compact
系统 SHALL 在每次发送 LLM 请求前基于完整请求估算上下文占用，并在达到 compact 运行阈值时对当前线程的 `chat` 历史执行 compact。`system` 和 `memory` SHALL NOT 被 compact。

#### Scenario: 到达 runtime 阈值时压缩 chat
- **WHEN** 当前线程的完整请求估算 token 占用达到 compact 运行阈值
- **THEN** 系统会在发送下一次 LLM 请求前对当前线程 `chat` 历史执行 compact
- **THEN** `system` 和 `memory` 内容保持不变

### Requirement: 系统 SHALL 将 compact 结果写回为一个 synthetic turn
系统 SHALL 将 compact 结果写回为一个 synthetic turn，而不是写入 memory。该 synthetic turn SHALL 包含两条 message：
- synthetic `user` message：保留任务目标、用户约束和当前请求背景
- synthetic `assistant` message：保留当前规划、已完成、未完成和关键事实

#### Scenario: compact 结果保留为两条 message
- **WHEN** 当前线程完成一次 compact
- **THEN** 系统会在 chat 历史中写入一个包含 synthetic `user` 和 synthetic `assistant` 的 compacted turn
- **THEN** 这两条 message 可以被后续对话继续使用

### Requirement: synthetic compact turn SHALL 参与后续 compact
系统 SHALL 将 compact 生成的 synthetic turn 视为后续 chat history 的一部分。在未来再次 compact 时，已有 synthetic compact turn SHALL 和后续新增 chat 一起参与压缩。

#### Scenario: 再次 compact 会包含旧 compacted turn
- **WHEN** 某个线程已经存在 synthetic compact turn 且后续又产生新的 chat history
- **THEN** 下一次 compact 时，该 synthetic compact turn 会作为输入的一部分参与新的 compact

### Requirement: 首版 SHALL 直接替换被 compact 的 active chat history
首版 compact 完成后，系统 SHALL 从 active chat history 中移除被 compact 的旧 chat messages，并用新的 compacted synthetic turn 替换它们。系统设计 SHALL 为未来 archive / shadow copy 策略保留扩展点，但首版 active history 中 SHALL NOT 保留被替换的旧 chat messages。

#### Scenario: 旧 chat 被 synthetic turn 替换
- **WHEN** 首版 compact 成功执行
- **THEN** 被 compact 的旧 chat messages 不再出现在 active chat history 中
- **THEN** active chat history 只保留 compacted synthetic turn 及其后的新消息

### Requirement: 系统 SHALL 通过 `CompactStrategy` 执行压缩策略
系统 SHALL 通过 `CompactStrategy` 抽象执行 compact，并允许不同策略输出不同的压缩计划。首版默认策略 SHALL 选择当前线程全部历史 chat 作为 compact 输入。

#### Scenario: 首版默认策略压缩全部 chat
- **WHEN** 系统使用首版默认 compact 策略
- **THEN** 当前线程已有的全部 chat messages 都会被纳入 compact 输入
- **THEN** compact 结果会替换这些 messages 的 active history 表达

### Requirement: 系统 SHALL 提供完整请求级别的上下文容量估算
系统 SHALL 对完整 LLM 请求进行容量估算，并至少区分 `system`、`memory`、`chat`、visible tools 和预留输出的 token 占用。系统 SHALL 能输出当前线程上下文容量占比，供 runtime compact 和 auto-compact 共用。

#### Scenario: 预算报告覆盖 message 与 tools
- **WHEN** 系统计算某个线程的上下文预算
- **THEN** 预算报告中包含 `system`、`memory`、`chat`、visible tools 和预留输出的占用信息
- **THEN** 系统可以据此计算当前容量占比

### Requirement: `auto_compact` 关闭时 SHALL NOT 暴露 compact tool 或容量信息给模型
当 `auto_compact` 未开启时，系统 SHALL 继续保留 runtime compact，但 SHALL NOT 向模型暴露 compact tool，也 SHALL NOT 注入供模型决策的上下文容量信息。

#### Scenario: 未开启 auto-compact 时模型无感知
- **WHEN** 当前配置中 `auto_compact` 为关闭状态
- **THEN** runtime compact 仍可在需要时执行
- **THEN** 模型看不到 compact tool，也收不到上下文容量信息

### Requirement: `auto_compact` 开启时 SHALL 允许模型主动触发 compact
当 `auto_compact` 开启且当前线程预算到达可提前压缩的可见阈值时，系统 SHALL 向模型注入上下文容量信息，并 SHALL 暴露 compact tool，让模型可以自行选择压缩时机。

#### Scenario: 开启 auto-compact 后模型可见 compact tool
- **WHEN** `auto_compact` 已开启且当前线程预算达到 compact tool 的可见阈值
- **THEN** 当前模型请求中包含上下文容量信息
- **THEN** 当前模型请求中可见 compact tool，模型可主动调用它
