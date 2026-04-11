## ADDED Requirements

### Requirement: 系统 SHALL 按单条 committed message / event 级别对外发送结果
系统 SHALL 在当前 turn 执行期间，按单条 committed message / event 级别对外发送用户可见结果，而不是等待 turn finalization 后再统一发送。只要某条 assistant text、tool 事件、tool 结果或失败通知已经进入 `Thread` 的正式当前 turn state，并完成对应 checkpoint，它就可以成为可外发 dispatch item。tool call 与 tool result SHALL 分别作为独立 dispatch item 发送，而 SHALL NOT 被要求成对打包后再发送。

#### Scenario: 文本输出在 turn 未结束前即可发送
- **WHEN** AgentLoop 在当前 turn 内生成了一条新的 assistant 文本消息，且该消息已经写入 `Thread`
- **THEN** 系统会将该消息作为新的 dispatch item 对外发送
- **THEN** channel 不需要等待 turn finalization 才看到这条文本

#### Scenario: tool call 与 tool result 各自独立发送
- **WHEN** AgentLoop 在同一 turn 内先提交一个 tool call，随后提交对应的 tool result
- **THEN** 系统会先发送该 tool call dispatch item
- **THEN** 系统会在稍后单独发送该 tool result dispatch item
- **THEN** 这两个 dispatch items 不需要成对打包后再一起发送

### Requirement: 系统 SHALL 为 message-level dispatch 保持稳定顺序
系统 SHALL 为同一 turn 内所有可外发 dispatch items 分配稳定的顺序号，并按该顺序对外发送。Router、channel adapter 和恢复链路 MUST 基于这个顺序号判断“哪些事件已经发送、哪些事件仍待发送”。

#### Scenario: tool result 不会越过更早的文本事件
- **WHEN** 同一 turn 内先后提交 assistant 文本、tool call 和 tool result
- **THEN** 这些 dispatch items 会按各自的稳定顺序号依次对外发送
- **THEN** 后提交的 tool result 不会越过更早提交的文本事件

### Requirement: 外发前 SHALL 存在对应的 in-progress checkpoint
系统 SHALL 在对外发送某个 dispatch item 之前，先保存能够重建当前发送进度的 in-progress checkpoint。该 checkpoint SHALL 至少包含当前 thread snapshot、turn identity 和已发送或待发送 dispatch cursor，使恢复链路能够避免盲目重发。

#### Scenario: checkpoint 先于外发建立
- **WHEN** 系统准备把某个新的 dispatch item 发送到 channel
- **THEN** Session/store 中已经存在可对应这条 dispatch item 的 in-progress checkpoint
- **THEN** 后续恢复流程可以基于该 checkpoint 判断是否需要继续发送或跳过该 item

### Requirement: failed turn SHALL 追加终止事件而不是回滚已发送结果
当某个 turn 在已经发送过部分 dispatch items 后失败时，系统 SHALL 追加一个 terminal failure dispatch item 作为结束信号。系统 SHALL NOT 假设此前已经发送给用户的文本输出或 tool 结果可以被隐式撤回。

#### Scenario: 已发送部分结果后 turn 失败
- **WHEN** 当前 turn 已经向用户发送了部分文本或 tool 结果，随后执行失败
- **THEN** 系统会继续发送一个 terminal failure dispatch item
- **THEN** 此前已经发送的 dispatch items 不会被当作“未发生”而从语义上回滚

### Requirement: finalized turn SHALL 不再是唯一外发边界
系统 SHALL 将 turn finalization 保留为完成态、审计态和 dedup 提交边界，但 SHALL NOT 再把它定义为唯一的用户可见结果外发边界。也就是说，message-level dispatch 可以先发生，而 finalized turn 用于宣告该输入对应回合的结束状态。

#### Scenario: turn 已有中间发送但仍未 finalization
- **WHEN** 当前 turn 已经外发了若干 committed dispatch items，但回合尚未结束
- **THEN** 这些中间结果对用户已经可见
- **THEN** turn finalization 仍会在稍后单独完成，以提交最终状态和 dedup 边界
