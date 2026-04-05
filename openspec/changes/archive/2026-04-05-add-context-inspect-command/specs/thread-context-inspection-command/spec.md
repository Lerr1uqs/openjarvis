## ADDED Requirements

### Requirement: 系统 SHALL 提供 `/context` 线程上下文容量摘要命令
系统 SHALL 提供线程级只读命令 `/context`。当调用方在某个线程中执行该命令时，系统 SHALL 基于当前线程已经持久化的 messages 使用统一的 deterministic 预算估算器计算上下文占用，并返回当前 thread 的总估算 token、`context_window_tokens`、utilization ratio 以及 bucket 拆分。首版结果 SHALL 至少包含 `system_tokens`、`chat_tokens`、`visible_tool_tokens`、`reserved_output_tokens` 四个 bucket，其中首版命令路径下 `visible_tool_tokens` SHALL 为 `0`。

#### Scenario: `/context` 返回当前线程的总体占用摘要
- **WHEN** 当前线程已经存在若干 persisted system/user/assistant message，且用户执行 `/context`
- **THEN** 命令返回成功
- **THEN** 返回内容包含当前线程的总估算 token、`context_window_tokens` 与 utilization 百分比
- **THEN** 返回内容包含 `system_tokens`、`chat_tokens`、`visible_tool_tokens`、`reserved_output_tokens` 的 bucket 拆分

#### Scenario: 空线程也可以返回最小占用摘要
- **WHEN** 当前线程尚未持久化任何 message，且用户执行 `/context`
- **THEN** 命令返回成功
- **THEN** 返回内容仍包含 `context_window_tokens` 与 bucket 拆分
- **THEN** 系统不会因为线程历史为空而报错

### Requirement: 系统 SHALL 提供 `/context role` 逐条 message 占比视图
系统 SHALL 提供 `/context role` 模式，用于逐条展示当前线程 persisted message 的上下文占用明细。返回结果 SHALL 按 persisted message 顺序输出；每条明细 SHALL 至少包含 message 序号、message role、估算 token 数、占 `context_window_tokens` 的比例，以及用于人工识别的短内容预览。

#### Scenario: `/context role` 列出每条消息的估算占比
- **WHEN** 当前线程包含多条不同 role 的 persisted message，且用户执行 `/context role`
- **THEN** 命令返回成功
- **THEN** 返回内容按 persisted message 顺序列出每条消息的 role、估算 token 和占窗口比例
- **THEN** 返回内容中的每条消息都包含截断后的短 preview，便于人工识别

#### Scenario: `/context role` 在空线程上返回空明细
- **WHEN** 当前线程尚未持久化任何 message，且用户执行 `/context role`
- **THEN** 命令返回成功
- **THEN** 返回内容显式表明当前线程没有可展示的 persisted message

### Requirement: Context inspect 命令 SHALL 保持只读并拒绝非法参数
`/context` 命令族 SHALL 是只读线程命令。系统 SHALL NOT 因为执行 `/context` 或 `/context role` 修改当前线程消息、feature 状态、tool 状态或审批状态。除空参数和 `role` 以外，任何其他参数组合都 SHALL 返回明确 usage 错误。

#### Scenario: 合法查询不会修改线程状态
- **WHEN** 当前线程已有历史消息和线程级状态，且用户执行 `/context` 或 `/context role`
- **THEN** 命令执行后线程消息与线程状态保持不变
- **THEN** 命令不会触发新的 agent dispatch

#### Scenario: 非法子命令返回 usage 错误
- **WHEN** 用户执行 `/context unexpected`
- **THEN** 命令返回失败
- **THEN** 返回内容明确说明只支持 `/context` 与 `/context role`
