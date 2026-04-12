## ADDED Requirements

### Requirement: 系统 SHALL 为 Feishu 入站消息提供独立的内存 TTL dedup 层
系统 SHALL 在 `Feishu` 入站消息进入主链路前提供独立的内存 TTL dedup 层。该层 SHALL 基于 `channel + external_message_id` 判定某条消息是否已经被当前进程接收处理，并 SHALL 与 `Session`、`Thread`、`ThreadStore` 持久化模型完全解耦。

#### Scenario: 重复投递在入口被拦截
- **WHEN** `Feishu` 在 TTL 窗口内重复投递同一个 `external_message_id`
- **THEN** dedup 层会在消息进入主链路前识别该重复投递
- **THEN** 系统不会因为这次重复投递再次进入正常的线程处理流程

### Requirement: Feishu dedup SHALL 维护 Processing 与 Completed 两种状态
系统 SHALL 为每条命中的 `Feishu` dedup 记录维护 `Processing` 与 `Completed` 两种状态。第一条消息命中时 SHALL 原子地创建 `Processing` 记录；请求成功完成后 SHALL 将其标记为 `Completed`；请求失败时 SHALL 删除该记录，使后续平台重试可以重新进入主链路。

#### Scenario: 请求失败后重试可重新进入
- **WHEN** 某条 `Feishu` 消息已经被登记为 `Processing`，但本次请求最终失败
- **THEN** dedup 层会删除这条记录
- **THEN** 平台后续对同一 `external_message_id` 的重试仍可以重新进入主链路

### Requirement: Feishu dedup SHALL 通过 TTL 与定期清理控制内存占用
系统 SHALL 为 `Feishu` dedup 记录设置 TTL，并提供定期清理机制移除过期记录。过期清理 SHALL 只影响入口 dedup 能力，而 SHALL NOT 影响任何线程正式消息或线程状态。

#### Scenario: 过期记录被清理但线程状态不受影响
- **WHEN** 某条 `Feishu` dedup 记录超过 TTL 并被清理
- **THEN** 该记录会从内存 dedup 层移除
- **THEN** 线程正式消息、线程状态与持久化快照不会受到影响

### Requirement: Feishu dedup SHALL 明确是 best-effort 而非 exactly-once 保证
系统 MUST 将 `Feishu` 内存 dedup 明确定义为单进程、best-effort 去重能力，而不是跨重启、跨实例的 exactly-once 保证。若进程重启、记录过期或部署为多实例，同一 `external_message_id` MAY 再次进入主链路；因此副作用路径 MUST 接受重复执行风险或自行实现幂等。

#### Scenario: 进程重启后同一消息可能再次被处理
- **WHEN** 系统进程重启后再次收到历史上已经处理过的同一个 `Feishu external_message_id`
- **THEN** 内存 dedup 层不会保证识别这条历史消息
- **THEN** 这条消息可能再次进入主链路并重复触发副作用
