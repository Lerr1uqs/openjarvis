## ADDED Requirements

### Requirement: 系统 SHALL 暴露显式的 subagent 管理工具
系统 SHALL 向主线程 agent 暴露显式的 subagent 管理工具：`spawn_subagent`、`send_subagent`、`close_subagent`、`list_subagent`。系统 SHALL NOT 在首版中把删除已落盘 child thread 的 `remove` 能力作为普通 agent 工具直接暴露。

#### Scenario: 主线程可以看到 subagent 管理工具
- **WHEN** 主线程进入正常 agent 执行流程
- **THEN** 模型可见工具中包含 `spawn_subagent`
- **THEN** 模型可见工具中包含 `send_subagent`
- **THEN** 模型可见工具中包含 `close_subagent`
- **THEN** 模型可见工具中包含 `list_subagent`

#### Scenario: `remove` 不作为首版对外工具暴露
- **WHEN** 主线程查看可用的 subagent 管理工具
- **THEN** 系统不会暴露 `remove_subagent` 或等价的“删除已落盘线程记录”工具
- **THEN** child thread 的物理删除仍然属于底层线程/存储能力

### Requirement: `spawn_subagent` SHALL 准备或复用当前父线程下的目标 subagent
系统 SHALL 允许主线程通过 `spawn_subagent` 显式准备某个 subagent child thread。若当前父线程下对应 profile 的 child thread 已存在，系统 SHALL 复用该线程；若不存在，系统 SHALL 创建并初始化该线程。

#### Scenario: 首次创建一个 subagent child thread
- **WHEN** 主线程首次调用 `spawn_subagent(browser)`
- **THEN** 系统会创建并初始化当前父线程下的 `browser` child thread
- **THEN** 返回结果会标明该 subagent 已经可继续被 `send_subagent` 调用

#### Scenario: 重复 `spawn_subagent` 复用既有 child thread
- **WHEN** 当前父线程下已经存在一个 `browser` child thread
- **AND** 主线程再次调用 `spawn_subagent(browser)`
- **THEN** 系统会复用该既有 child thread
- **THEN** 系统不会再创建第二个同 profile child thread

### Requirement: `send_subagent` SHALL 同步阻塞地执行 subagent 并返回单次聚合结果
系统 SHALL 让主线程通过 `send_subagent` 同步阻塞地执行一次 subagent 请求。`send_subagent` 的返回结果 SHALL 作为一次普通工具调用结果返回给主线程，而 SHALL NOT 在首版中作为后台任务、异步任务或流式子结果暴露。

#### Scenario: `send_subagent` 返回一次聚合结果
- **WHEN** 主线程调用 `send_subagent(browser, "...")`
- **THEN** 系统会同步等待该 `browser` child thread 完成这次请求
- **THEN** 工具调用返回一个聚合后的单次结果
- **THEN** 主线程后续推理继续把这个结果当作普通 `ToolResult` 使用

#### Scenario: 首版不支持异步后台 subagent
- **WHEN** 主线程调用 `send_subagent`
- **THEN** 系统不会返回后台任务句柄、订阅令牌或轮询 token
- **THEN** 系统不会要求主线程再通过第二个异步接口查询该次 subagent 结果

### Requirement: subagent 执行 SHALL 使用独立 worker 池，而不是复用主 worker 请求队列
系统 SHALL 通过独立的 subagent worker 池执行 `send_subagent` 请求。主线程在工具调用中同步等待子线程结果时，系统 SHALL NOT 把该请求重新投递回当前主 worker 的同一请求队列。

#### Scenario: 主线程同步等待 subagent 时不会复用主 worker 队列
- **WHEN** 主线程正在一个主 worker 执行单元里处理 `send_subagent`
- **THEN** subagent 请求会被投递到独立的 subagent worker 池
- **THEN** 系统不会要求当前主 worker 在同一请求队列上等待自己后续消费该请求

### Requirement: subagent 线程 SHALL 继续复用现有 `AgentLoop` 执行框架
系统 SHALL 让 subagent 线程继续复用现有 `AgentLoop` 执行框架，而不是为 subagent 另写第二套推理主循环。subagent 线程的 prompt、工具可见性和消息提交 SHALL 继续以 child thread 自身的 `Thread` 状态为真相。

#### Scenario: subagent 按 child thread 自己的 thread truth 执行
- **WHEN** 系统执行一个 `browser` subagent 请求
- **THEN** subagent 的请求消息来自该 `browser` child thread 自己的消息历史
- **THEN** subagent 的可见工具来自该 `browser` child thread 自己的 `ThreadAgentKind` 与 thread-scoped tool state
- **THEN** subagent 产生的新消息会提交到该 `browser` child thread 自己的正式线程历史

### Requirement: subagent 事件 SHALL 通过 `AgentEventSender::for_subagent_thread` 进入“只记录不发送”的内部模式
系统 SHALL 为 subagent 执行提供 `AgentEventSender::for_subagent_thread`。通过该入口创建的 sender SHALL 继续生成完整的 dispatch metadata 供日志和调试使用，但 subagent committed event 默认 SHALL NOT 进入 Router，也 SHALL NOT 直接发往外部 channel。

#### Scenario: subagent committed event 只记录不发送
- **WHEN** subagent 在执行中提交了一条 assistant 文本或 tool 事件
- **THEN** 系统会保留该事件对应的 metadata 以供日志或调试记录
- **THEN** 该事件不会被 Router 当作用户可见 channel 消息发送出去

### Requirement: 系统 SHALL 支持 `persist` 与 `yolo` 两种 subagent 生命周期
系统 SHALL 支持 `persist` 与 `yolo` 两种 subagent 生命周期。两者执行时都 SHALL 复用现有 thread-owned 持久化边界；区别只在于调用成功后的保留或回收策略。

#### Scenario: `persist` subagent 在调用后继续保留
- **WHEN** 主线程创建并调用一个 `persist` 模式的 subagent
- **THEN** subagent 在本次调用完成后仍然保留其线程历史和 thread state
- **THEN** 后续主线程可以继续对同一个 child thread 调用 `send_subagent`

#### Scenario: `yolo` subagent 在成功返回后自动回收
- **WHEN** 主线程创建并调用一个 `yolo` 模式的 subagent
- **AND** 这次 `send_subagent` 成功返回了工具结果
- **THEN** 系统会对该 child thread 执行 best-effort 底层 `remove`
- **THEN** 该 `yolo` child thread 的已落盘记录会被作为内部回收目标删除

### Requirement: `close_subagent` SHALL 结束已存在 subagent 的后续使用，但不直接替代底层 `remove`
系统 SHALL 允许主线程通过 `close_subagent` 结束某个已存在 subagent 的后续使用。`close_subagent` SHALL 作为高层生命周期动作存在，而 SHALL NOT 直接等价于底层 child thread 物理删除能力。

#### Scenario: `close_subagent` 结束一个 `persist` subagent
- **WHEN** 当前父线程下存在一个 `persist` 模式的 `browser` subagent
- **AND** 主线程调用 `close_subagent(browser)`
- **THEN** 系统会将该 subagent 视为已结束后续使用
- **THEN** 后续若需要再次长期使用该 profile，调用方需要重新准备或创建该 subagent

### Requirement: `list_subagent` SHALL 返回当前父线程下的 subagent 视图
系统 SHALL 允许主线程通过 `list_subagent` 查看当前父线程下已经存在的 subagent child thread 视图。该视图 SHALL 至少包含 `subagent_key`、当前生命周期模式以及该实例是否仍可继续被发送请求。

#### Scenario: `list_subagent` 只返回当前父线程下的 subagent
- **WHEN** 父线程 A 调用 `list_subagent`
- **THEN** 返回结果只包含父线程 A 自己名下的 subagent child thread
- **THEN** 系统不会把其他父线程名下的 subagent 混入这次结果
