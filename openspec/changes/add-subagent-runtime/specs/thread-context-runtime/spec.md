## MODIFIED Requirements

### Requirement: 系统 SHALL 根据 `ThreadAgentKind` 初始化主线程与 subagent 子线程
系统 SHALL 继续通过统一的 `initialize_thread(thread, thread_agent_kind)` 入口初始化线程，并 SHALL 允许该入口同时用于主线程和 subagent child thread。无论线程是否属于 subagent，只要它拥有自己的 `ThreadAgentKind`，系统都 SHALL 根据该 kind 选择稳定 prompt 和默认工具绑定。

#### Scenario: child thread 按自己的 `ThreadAgentKind` 完成初始化
- **WHEN** 系统为某个 `browser` child thread 执行初始化
- **THEN** 系统会根据 `ThreadAgentKind::Browser` 写入该 child thread 自己的稳定 prompt
- **THEN** 系统会为该 child thread 绑定属于 `Browser` profile 的默认工具集合
- **THEN** 这些初始化结果只属于这个 child thread 自己

### Requirement: Agent loop SHALL 支持不依赖外部 channel 的 subagent 内部执行
系统 SHALL 允许 `AgentLoop` 在 subagent 场景中由内部兼容层触发执行，而不要求一定存在真实外部 channel 消息。subagent 执行时，系统 MAY 构造兼容层 `IncomingMessage` 作为当前输入，但 subagent 的请求组装、消息提交和工具调用 SHALL 仍然以 child thread 自己的 `Thread` 为真相。

#### Scenario: subagent 通过兼容层 `IncomingMessage` 驱动一次内部执行
- **WHEN** 主线程通过 `send_subagent` 请求系统执行某个 child thread
- **THEN** 系统可以为这次执行构造一个内部兼容层 `IncomingMessage`
- **THEN** `AgentLoop` 仍然只通过该 child thread 的消息历史和当前输入组装请求
- **THEN** subagent 的正式消息仍然提交到这个 child thread 自己的线程历史

### Requirement: subagent 执行 SHALL 通过 `AgentEventSender::for_subagent_thread` 记录内部事件
系统 SHALL 为 subagent child thread 提供 `AgentEventSender::for_subagent_thread(...)`。通过该入口创建的 sender SHALL 继续携带完整调试 metadata，但 subagent committed event 默认 SHALL NOT 转发到 Router，也 SHALL NOT 发往外部 channel。

#### Scenario: subagent committed event 不进入外部发送面
- **WHEN** subagent 在执行过程中提交了一条 assistant 文本消息
- **THEN** 系统会为这条消息生成对应的内部 dispatch metadata
- **THEN** 该 metadata 只用于日志、调试或内部聚合
- **THEN** 该消息不会作为用户可见消息发往 channel

### Requirement: subagent child thread 的正式消息 SHALL 与父线程正式消息分离持有
系统 SHALL 让 subagent child thread 持有自己独立的正式消息序列。主线程通过 `send_subagent` 拿到的只是一次聚合工具结果；系统 SHALL NOT 把 child thread 的内部消息直接混入父线程自己的正式历史，除非它们被主线程后续作为普通 `ToolResult` 或 assistant 消息显式提交。

#### Scenario: child thread 内部历史不会自动混入父线程
- **WHEN** subagent child thread 在执行中产生了多条 assistant / tool 消息
- **THEN** 这些消息会保留在 child thread 自己的线程历史中
- **THEN** 父线程只会看到 `send_subagent` 返回的聚合工具结果
- **THEN** 系统不会自动把 child thread 全量消息回填到父线程正式消息序列

