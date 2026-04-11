## MODIFIED Requirements

### Requirement: 系统 SHALL 在消息 commit 后由 agent 经消息信道逐条发送
系统 SHALL 在 assistant 文本、tool call、tool result、terminal failure 等用户可见项完成 thread commit 后，由 agent 立即经消息信道逐条发送 committed event。该 committed event SHALL NOT 携带 turn 级身份字段；消息的可见顺序只由 commit 顺序决定，而不是由 turn identity 决定。

#### Scenario: committed event 不带 turn 身份
- **WHEN** agent 在某个请求内连续 commit 多条用户可见消息
- **THEN** 每条 committed event 都会直接按 commit 顺序发送
- **THEN** event payload 中不会额外携带 `turn_id`

### Requirement: 系统 SHALL NOT 为 tool audit 维护 pending event buffer
系统 SHALL NOT 为 tool audit event 维护“先缓存、后绑定 turn identity”的 pending buffer。tool audit 只允许在 active 请求生命周期内部直接记录；若当前不存在 active 请求生命周期，系统 MUST 直接报错，而不是把 event 暂存起来等待未来 turn 绑定。

#### Scenario: 没有 active turn 时记录 tool audit 会失败
- **WHEN** 某个调用尝试在没有 active turn 的情况下记录 tool audit event
- **THEN** 系统会直接返回错误
- **THEN** thread 中不会出现任何 pending tool event buffer
