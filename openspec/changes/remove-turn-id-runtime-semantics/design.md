## 设计

### 1. turn 只保留执行期本地状态

`Thread` 仍可在内存中保留一个 active turn working set，用于：

- 约束一次请求只能有一个活跃执行期；
- 暂存该请求的 `external_message_id`；
- 暂存该请求内的 tool audit event；
- 打日志。

但是这份状态 SHALL NOT：

- 拥有稳定 `turn_id`；
- 被持久化到 thread snapshot；
- 被 session/store/router 作为跨请求语义引用。

### 2. 删除所有 turn identity 字段

以下字段全部移除：

- `ThreadToolEvent.turn_id`
- `ThreadCurrentTurn.turn_id`
- `ThreadFinalizedTurn.turn_id`
- `ExternalMessageDedupRecord.turn_id`
- `CommittedAgentDispatchItem.turn_id`

所有依赖这些字段的日志、metadata、测试断言同步改写。

### 3. 删除 pending tool event buffer

`record_tool_event(...)` 不再允许“当前没有 active turn 也先塞进 pending buffer”。

新语义：

- 如果当前存在 active turn，则直接写入 active turn 的 tool audit working set；
- 如果当前不存在 active turn，则直接报错。

因为主链路里的 tool audit 发生点本来就在 `begin_turn(...)` 之后，所以不需要 buffer。

### 4. thread 持久化时剔除本地 turn 状态

`push_message(...)` 仍然是正式消息的唯一持久化边界，但它持久化的是：

- 正式消息历史；
- thread state；
- revision；
- dedup 所需字段；

而不是 active turn working set。

实现上，session/store 写入 thread snapshot 前必须剔除本地 turn 状态；写入成功后再把 live state 回填到运行中的 thread 对象。

### 5. dedup 只以 external message 为单位

`ExternalMessageDedupRecord` 只保留：

- `thread_id`
- `external_message_id`
- `completed_at`

系统不再尝试表达“某条 external message 对应哪个 turn”。

## 风险

- 删除 `turn_id` 后，日志排查时少了一个 UUID 锚点。
  处理：统一改用 `thread_id + external_message_id + completed_at` 组合打印。

- active turn 不再持久化后，请求中断后无法恢复“执行到哪一步”。
  处理：这是显式接受的结果；系统只依赖已持久化消息上下文继续执行。
