## MODIFIED Requirements

### Requirement: 系统 SHALL 在线程初始化时建立线程级 request context snapshot
系统 SHALL 在 `Thread` runtime attachment 已就绪后，由 `Thread.ensure_initialized()` 为目标线程建立稳定的线程级 request context snapshot。外部模块可以负责 attach runtime service 并触发初始化，但 SHALL NOT 直接拼装并写入初始化消息数组。该 snapshot SHALL 在同一线程后续 turn 中保持稳定，直到被显式清空或迁移。

#### Scenario: 新线程由 `Thread` 自己完成初始化
- **WHEN** Session 或 Worker 首次解析出某个 internal thread，并为该 thread attach 了 feature provider、memory repository 等 runtime service
- **THEN** 外部只需要调用 `Thread.ensure_initialized()`
- **THEN** 稳定 system prompt 与 feature prompt snapshot 由 `Thread` 自己写入其 request context

#### Scenario: 已初始化线程不会被外部重复覆盖前缀
- **WHEN** 某个线程已经存在稳定 request context snapshot，且后续请求再次 attach runtime service
- **THEN** `Thread.ensure_initialized()` 会识别该线程已初始化
- **THEN** Worker 或 AgentLoop 不会重新覆盖这个线程已有的稳定前缀

### Requirement: Agent loop SHALL 基于 `ThreadContext + current user input` 组装请求
系统 SHALL 让 AgentLoop 主链路接收已经 attach runtime 且已初始化的 `Thread` 与当前轮 user input。AgentLoop SHALL 只通过 `Thread` 对外暴露的接口组装请求，包括 `push_message(...)`、`messages()`、`visible_tools()` 和 `call_tool(...)`；AgentLoop SHALL NOT 直接管理 thread-scoped 的 tool registry、memory repository 或 feature provider。

#### Scenario: worker 只传 thread 和当前输入
- **WHEN** worker 准备把某个线程请求交给 AgentLoop
- **THEN** worker 只需要传入目标 `Thread` 与当前轮 user input
- **THEN** AgentLoop 会通过 `Thread.push_message(...)` 写入当前输入并组装本轮请求

#### Scenario: loop 不直接操作 thread-scoped runtime service
- **WHEN** AgentLoop 在一轮 turn 内连续执行 generate、tool call 与 compact
- **THEN** loop 会通过 `Thread.visible_tools()` 获取当前线程可见工具
- **THEN** loop 会通过 `Thread.call_tool(...)` 触发线程工具调用，而不是直接调用全局 runtime service

### Requirement: 线程级 request context SHALL 与 conversation history 分层
系统 SHALL 在 `Thread` 内同时持有稳定 request context、已持久化 conversation history、当前 turn working set 和 runtime-only attachment，但这些边界的 ownership SHALL 全部归 `Thread`。`Thread.messages()` SHALL 返回当前线程完整请求视图；持久化 history、system prefix 与当前 turn request-only messages 的导出边界 MUST 由 `Thread` 显式控制，而不是由外部模块拼接消息向量。

#### Scenario: 请求视图由 thread 统一导出
- **WHEN** AgentLoop 需要为当前 turn 构造一次 LLM request
- **THEN** `Thread.messages()` 会统一导出稳定前缀、已持久化 history 和当前 turn request-visible messages
- **THEN** Worker 或 AgentLoop 不需要也不能额外维护第二份消息 truth

#### Scenario: runtime attachment 不进入持久化快照
- **WHEN** Session store 保存或恢复某个线程
- **THEN** 持久化快照中只包含 declarative thread state
- **THEN** tool registry、memory repository 和 feature provider 这类 runtime attachment 不会被直接持久化

### Requirement: request context 与 request-time memory SHALL NOT 成为 compact source history
系统 SHALL 继续保证稳定 request context 不进入 compact source history。通过 `Thread.push_message(...)` 注入的 request-time memory 或其他 request-only runtime messages，只有在被 `Thread` 显式物化为 finalized turn history 后，才可以成为后续 compact source；未物化前 SHALL NOT 被 compact。

#### Scenario: request-only memory 不会被提前 compact
- **WHEN** 当前线程在一次请求中通过 attached memory repository 注入了 request-only memory message
- **THEN** 该 memory message 会出现在当前 turn 的请求视图中
- **THEN** 在它未被物化为 finalized history 前，不会进入 compact source

### Requirement: request-time memory SHALL 保持动态注入而非线程初始化固化
系统 SHALL 将 memory 视为 `Thread` 当前 turn 的动态注入内容，而不是线程初始化阶段的固定 snapshot。即使 future memory provider 接入，memory 的查询、选择和注入也 SHALL 由 `Thread` 基于其 attached `MemoryRepository` 决定，并通过 `push_message(...)` 进入当前 turn 请求视图；AgentLoop SHALL NOT 直接查询 memory repository。

#### Scenario: 命中 memory 时由 thread 自行注入
- **WHEN** 某一轮请求命中 memory repository 并需要向模型注入 memory
- **THEN** 由 `Thread` 自己从 attached repository 读取并注入这些 messages
- **THEN** AgentLoop 不会绕过 `Thread` 直接把 memory message 拼进请求

## ADDED Requirements

### Requirement: Thread SHALL 暴露统一的 `push_message(...)` 写入入口
系统 SHALL 要求任何进入 thread 请求视图的消息都通过统一的 `Thread.push_message(...)` 入口写入。外部模块 SHALL NOT 再按 user/tool/memory/system 分别调用多套 message mutation API。

#### Scenario: turn 内所有消息都走统一入口
- **WHEN** 当前 turn 先后写入 user message、assistant text、tool result 和 request-time memory
- **THEN** 这些消息都会通过 `Thread.push_message(...)` 进入 thread 当前视图
- **THEN** 它们后续是否持久化、是否参与 compact 由 `Thread` 自己决定

### Requirement: Thread SHALL 挂载 thread-scoped runtime service 并自行使用
系统 SHALL 允许 `Thread` 在运行时挂载 thread-scoped runtime service，包括全局 `ToolRegistry`、`MemoryRepository` 和 feature provider 集合。一旦挂载完成，线程初始化、memory 注入、工具可见性计算和工具调用 SHALL 由 `Thread` 自己通过这些 service 执行，而不是由 AgentLoop 或 Worker 直接调用这些 service。

#### Scenario: 线程恢复后重新挂载 runtime service
- **WHEN** 某个线程从持久化 store 恢复到内存并准备处理下一轮请求
- **THEN** Worker 或 Session 会先为该 thread attach runtime service
- **THEN** 后续 thread 初始化、request 组装和工具调用都由 `Thread` 自己驱动
