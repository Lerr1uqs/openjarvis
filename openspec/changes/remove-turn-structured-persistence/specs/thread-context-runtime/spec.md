## REMOVED Requirements

### Requirement: Agent loop SHALL 基于 `ThreadContext + current user input` 组装请求
**Reason**: 主运行时不再以 detached `ThreadContext` 快照作为核心输入，而是直接围绕 live `Thread` handle 工作；旧 requirement 同时把正式消息写入描述成 turn 内消息收集，已经不符合新的 message-atomic 持久化边界。
**Migration**: worker 改为把 live `Thread` handle 与当前 user input 交给 AgentLoop；正式消息与状态只通过 thread-owned mutator 写入并持久化。

### Requirement: 线程级 request context SHALL 与 conversation history 分层
**Reason**: 原 requirement 仍以 `ConversationTurn` 作为 conversation history 的持久化载体，与本次“只保留 flat message 序列，不再持久化 turn 结构”的目标冲突；同时“独立 request context”表述会误导为额外成员。
**Migration**: 将正式 conversation history 迁移为线程内的 flat persisted message sequence；稳定前缀直接表现为消息序列开头的 `System` messages，不再引入独立 request context 成员。

## ADDED Requirements

### Requirement: 系统 SHALL 通过独立的 `ThreadRuntime` 提供线程运行时能力
系统 SHALL 用独立的 `ThreadRuntime` 向 live `Thread` 提供线程创建期初始化、工具可见性计算、工具调用、memory 访问和 feature prompt 重建等运行时能力。`Thread` 自身 SHALL NOT 保存 `ThreadRuntimeAttachment` 或其他挂载式运行时对象。

#### Scenario: 运行时能力不再挂在 Thread 内部
- **WHEN** worker 或 AgentLoop 需要为某个 live `Thread` 提供创建期初始化或工具运行时能力
- **THEN** 它们会显式使用独立的 `ThreadRuntime`
- **THEN** `Thread` 本体不会因为这些运行时能力而持有 attachment 字段

### Requirement: `SessionManager` 派生 Thread 时 SHALL 完成初始化消息持久化
系统 SHALL 在 `SessionManager` 首次解析并派生某个 `Thread` handle 时，完成 feature/system 初始化消息的生成与持久化。`ThreadRuntime` SHALL 在 thread handle 返回给 worker 或 AgentLoop 之前，把这些初始化消息作为正式消息写入线程开头前缀，并保证写入成功后线程才进入后续执行。

#### Scenario: SessionManager 返回的 thread 已完成初始化
- **WHEN** `SessionManager` 为新的用户输入首次解析并派生某个线程
- **THEN** `ThreadRuntime` 会先基于 feature、tool registry 和稳定 `System` 前缀规则生成初始化消息
- **THEN** 这些初始化消息会在 thread handle 返回前写入线程正式消息序列并完成持久化
- **THEN** 后续 worker 与 AgentLoop 不需要再补做初始化

### Requirement: Agent loop SHALL 基于 live `Thread` 与当前 user input 组装请求
系统 SHALL 让 AgentLoop 主链路接收 live `Thread` 与当前 user input，而不是接收 detached thread snapshot 或外部预组装的消息容器。发送给 LLM 的 messages SHALL 通过 `Thread.messages()` 从线程内部统一导出；正式消息只能通过 `Thread.push_message(...)` 或其他 thread-owned mutator 注入，并在成功返回前完成持久化。

#### Scenario: worker 只传 live Thread 与当前 user input
- **WHEN** worker 准备把某个线程请求交给 AgentLoop
- **THEN** worker 只需要提供 live `Thread` handle 与当前 user input
- **THEN** AgentLoop 会基于该线程内部状态自行组装本轮 LLM request
- **THEN** Router 不会在转发过程中主动拼接消息、补做提交或回写线程正式状态

### Requirement: AgentLoop SHALL NOT 依赖运行中补初始化机制
系统 SHALL NOT 在 AgentLoop 或普通请求主链路中保留 `ensure_initialized()`、`request_context_initialized_at` 或其他“如果尚未初始化则现场补写”的机制。初始化完成状态 SHALL 仅由已持久化的初始化消息前缀和正式线程状态表达。

#### Scenario: 恢复后的线程不会在 loop 内补初始化
- **WHEN** 某个线程从持久化层恢复并准备进入一次新的 AgentLoop
- **THEN** AgentLoop 直接消费该线程已持久化的正式消息与线程状态
- **THEN** 系统不会检查或回填 `request_context_initialized_at`
- **THEN** 系统不会调用 `ensure_initialized()` 之类接口为本轮请求补写初始化消息

### Requirement: 线程正式快照 SHALL 以稳定 `System` 前缀与 flat message history 分层表达
系统 SHALL 继续让稳定 `System` 前缀与 conversation history 分层，但 conversation history SHALL 改为 flat persisted message sequence，而不是 `ConversationTurn` 或其他 turn 结构。线程恢复后，系统 SHALL 能分别恢复稳定前缀、正式消息序列和线程状态。

#### Scenario: 恢复结果只包含稳定前缀与正式消息序列
- **WHEN** 某个线程在重启后从持久化层恢复
- **THEN** 系统可以分别读出稳定 `System` 前缀、正式消息序列和线程状态
- **THEN** 恢复结果中不会出现 conversation turn、finalized turn 或 turn working set

### Requirement: 当前请求执行期状态 SHALL 仅作为 live-only 内部状态存在
系统 MAY 在 `Thread` 内部保留当前请求执行期状态，用于日志、串行约束和临时工具审计；但这些状态 SHALL NOT 形成公共 turn 结构体，SHALL NOT 写入持久化快照，也 SHALL NOT 作为跨请求恢复的一部分暴露。

#### Scenario: 中途重启后不会恢复未完成请求期状态
- **WHEN** 某个请求执行到中途时进程重启
- **THEN** 系统恢复的是该线程已经正式持久化的消息与状态
- **THEN** 未完成请求的临时执行期状态不会作为 turn 结构被恢复
