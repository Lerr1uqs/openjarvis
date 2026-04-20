## ADDED Requirements

### Requirement: Command SHALL 绕过 PostgreSQL topic queue
系统 SHALL 在 router 入队前拦截 `/xxx` 命令并直接执行。命令消息 SHALL NOT 作为 queue message 写入 PostgreSQL topic queue。

#### Scenario: slash command 不进入 queue
- **WHEN** router 收到一条会被 `CommandRegistry` 识别为命令的 incoming message
- **THEN** 系统直接执行对应命令处理路径
- **THEN** 该消息不会写入 PostgreSQL topic queue

### Requirement: 普通消息 SHALL 在解析 locator 后按需准备 thread 再以 `thread_key` 入队
系统 SHALL 在普通消息入队前先解析 `ThreadLocator`，并检查目标 thread 在当前进程中是否已经加载。若该 thread 已经加载，系统 SHALL 直接复用现有 thread identity；若该 thread 尚未加载，系统 SHALL 先执行线程准备动作，再以该消息解析出的 `thread_key` 作为 queue `topic` 入队。queue payload SHALL 至少包含已解析的 `ThreadLocator` 与原始 `IncomingMessage`，而 SHALL NOT 依赖 live thread handle。

#### Scenario: 已加载 thread 直接复用后入队
- **WHEN** router 收到一条非命令普通消息
- **AND** 该消息对应的 thread 当前进程已经加载
- **THEN** 系统直接复用该 thread 的既有 identity
- **THEN** 随后把 `ThreadLocator + IncomingMessage` 作为 queue payload 写入 PostgreSQL topic queue

#### Scenario: 未加载 thread 先准备再入队
- **WHEN** router 收到一条非命令普通消息
- **AND** 该消息对应的 thread 当前进程尚未加载
- **THEN** 系统先解析 locator 并完成该 thread 的准备动作
- **THEN** 随后把 `ThreadLocator + IncomingMessage` 作为 queue payload 写入 PostgreSQL topic queue

#### Scenario: 同一 thread 的多条消息共享同一个 topic
- **WHEN** 两条普通消息解析后得到相同的 `thread_key`
- **THEN** 它们会写入同一个 queue `topic`
- **THEN** 后续消费串行维度与该 thread 的串行维度保持一致

### Requirement: PostgreSQL topic queue SHALL 只管理消息传递状态
PostgreSQL topic queue SHALL 只保存普通消息的传递 payload、传递状态和 worker lease。queue SHALL NOT 保存 thread sqlite snapshot、thread message history、tool/LLM 运行时状态，也 SHALL NOT 要求与 thread sqlite 做跨库联合事务。

#### Scenario: queue payload 不包含 thread 正式事实
- **WHEN** 一条普通消息被写入 PostgreSQL topic queue
- **THEN** queue 记录中只包含消息传递所需的 payload 与传递状态
- **THEN** 该记录不要求持有 thread snapshot、thread revision 或其他 thread 正式事实字段

### Requirement: 系统 SHALL 为每个 `thread_key` 保持至多一个活跃 domain worker
系统 SHALL 通过独立 worker lease 事实协调 domain worker 生命周期。对于同一个 `thread_key`，系统同一时刻 SHALL 只允许一个活跃 worker 持有该 domain；当该 domain 出现待处理消息且当前没有活跃 worker 时，系统 SHALL 懒创建一个新的 domain worker。

#### Scenario: 首条待处理消息触发 domain worker 创建
- **WHEN** 某个 `thread_key` 首次出现待处理 queue message，且当前没有活跃 worker lease
- **THEN** 系统会为该 domain 创建一个新的 worker
- **THEN** 该 worker 会持有对应的 domain lease

#### Scenario: 已有活跃 worker 时不会重复创建同 domain worker
- **WHEN** 某个 `thread_key` 已经有活跃 worker lease
- **THEN** 后续同 domain 的新消息不会再触发第二个 worker 创建
- **THEN** 系统继续复用该 domain 的现有活跃 worker

### Requirement: Domain worker SHALL 只 claim 自己 domain 的消息并完成传递
每个活跃 domain worker SHALL 只 claim 自己 `thread_key` 下的 `pending` queue message，并按可见顺序逐条处理。处理结束后，worker SHALL 将该消息标记为 `complete`，而 SHALL NOT 再依赖 router 内存态 `pending_threads/queued_messages` 或全局 request `mpsc` 队列来表达传递完成。

#### Scenario: 同一 domain 的消息按顺序逐条完成
- **WHEN** 同一个 `thread_key` 下已经存在多条待处理 queue message
- **THEN** 对应 domain worker 会逐条 claim 并处理这些消息
- **THEN** 在前一条消息完成前，后一条消息不会被同 domain 的另一个 worker 并发处理

#### Scenario: 已知业务失败路径仍以 complete 收口传递
- **WHEN** worker 已经为某条消息完成 thread 收口，并确定该条传递不需要继续重投
- **THEN** queue 会把该消息标记为 `complete`
- **THEN** queue 不要求额外保存 thread 业务成功或失败的内部细节

### Requirement: 系统 SHALL 回收过期 worker lease 与 stranded active message
系统 SHALL 为 worker lease 和 active queue message 提供过期恢复机制。若 worker 异常退出或租约超时，系统 SHALL 记录清理日志、回收过期 worker lease，并把 stranded active message 恢复到可重试状态。由于 queue 不与 thread sqlite 做跨库联合事务，恢复路径 SHALL 显式采用至少一次交付语义。

#### Scenario: 过期 worker 被清理并释放 domain
- **WHEN** 某个 domain worker 的 lease 超时且未续租
- **THEN** 系统会记录该次过期清理日志
- **THEN** 系统会回收该 worker 的 domain lease，使后续同 domain 消息可以再次被处理

#### Scenario: stranded active message 会被恢复为可重试
- **WHEN** 某条 queue message 处于 `active`，但其所属 worker lease 已经过期
- **THEN** 系统会把该消息恢复为可重试状态
- **THEN** 后续新的 domain worker 可以重新 claim 这条消息
