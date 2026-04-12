## MODIFIED Requirements

### Requirement: 系统 SHALL 在线程初始化时直接写入稳定 `System` 前缀
系统 SHALL 在新线程初始化阶段通过 `init_thread()` 为目标 `Thread` 直接写入稳定的 `System` 前缀。该前缀 SHALL 作为 `Thread.messages()` 开头的一组持久化 `System` messages 存在，并在同一线程后续 turn 中保持稳定。正常线程在产生任何 chat message 之前，MUST 已经具备该 system prefix。

#### Scenario: 新线程创建时初始化稳定前缀
- **WHEN** Session 或 Router 首次为某个 internal thread 创建 `Thread`
- **THEN** 该线程会在进入 agent loop 前写入自己的稳定 `System` 前缀
- **THEN** 该前缀中包含当前 system prompt 与 feature system messages

#### Scenario: 线程在 chat 之前已有 system prefix
- **WHEN** 某个线程开始接受第一条 user chat message
- **THEN** 该线程开头已经存在稳定 system prefix
- **THEN** 后续 chat message 都会位于该前缀之后

### Requirement: Agent loop SHALL 基于 `ThreadContext + current user input` 组装请求
系统 SHALL 让 AgentLoop 主链路接收已初始化的 `Thread` 与当前轮 user input，并 SHALL 只通过 `Thread` 持有的当前 turn state 进行消息写入与请求导出。AgentLoop SHALL NOT 在 `Thread` 之外再维护第二份 live system/chat/commit message source of truth。

#### Scenario: worker 只传当前 user input
- **WHEN** worker 准备把某个线程请求交给 AgentLoop
- **THEN** worker 只需要传入目标 `Thread` 与当前 user input
- **THEN** AgentLoop 会把当前 user input 写入 thread-owned turn state
- **THEN** 发送给 LLM 的 messages 由 `Thread.messages()` 统一导出

#### Scenario: loop 不再维护局部消息真相
- **WHEN** AgentLoop 在一轮 turn 内连续执行 generate、tool call 与 compact
- **THEN** 当前 turn 的 user / assistant / tool / compact 消息都由 `Thread` 管理
- **THEN** loop 不会再依赖独立的 `live_chat_messages`、`commit_messages` 或 `request_system_messages`

### Requirement: 稳定 `System` 前缀、conversation history 与当前 turn working set SHALL 明确分层
系统 SHALL 在 `Thread` 内明确区分稳定 `System` 前缀、已持久化 conversation history 和当前 turn working set，但它们的 ownership SHALL 全部归 `Thread`，且不要求独立的 request context 成员。`Thread.messages()` SHALL 返回当前 turn 的完整请求视图；持久化 history、system prefix 与 turn finalization MUST 通过 `Thread` 的显式边界导出，而不是由外部模块手工拼接消息向量。

#### Scenario: 请求视图由 thread 统一导出
- **WHEN** AgentLoop 需要为当前 turn 构造一次 LLM request
- **THEN** `Thread.messages()` 会导出稳定 system prefix、已持久化历史和当前 turn request-visible messages
- **THEN** Router 或 Worker 不需要也不能额外拼接第二份请求消息列表

### Requirement: 稳定 `System` 前缀与 request-time memory SHALL NOT 成为 compact source history
系统 SHALL 继续保证稳定 system prefix 不进入 compact source history。若 memory 或其他 request-time runtime messages 只属于当前 turn 的临时注入，它们 SHALL 由 `Thread` 的当前 turn state 管理，并 SHALL NOT 在未被显式物化为 finalized turn message 之前成为 compact source history。

#### Scenario: compact 只处理 thread-owned active non-system messages
- **WHEN** 当前线程触发 runtime compact 或模型主动调用 `compact`
- **THEN** compact 输入不会包含稳定 system prefix
- **THEN** 只属于当前 turn 的 request-only runtime messages 不会在未物化前被 compact 成持久化 chat history

### Requirement: request-time memory SHALL 保持动态注入而非线程初始化固化
系统 SHALL 将 memory 视为当前 turn 的动态注入内容，而不是线程初始化阶段的固定 snapshot。即使未来接入 memory provider，memory 也 MUST 进入 `Thread` 的当前 turn state，而不是由 Router 组装消息或由 `init_thread()` 一次性写入稳定前缀。

#### Scenario: 命中 memory 时只影响当前 turn 请求视图
- **WHEN** 某一轮请求命中 memory provider 并需要向模型注入 memory
- **THEN** 这些 memory messages 由 `Thread` 当前 turn state 管理
- **THEN** 它们只影响当前 turn 的请求视图，而不会变成稳定初始化前缀

## ADDED Requirements

### Requirement: Thread SHALL 保持 system messages 位于开头前缀
系统 SHALL 保证稳定 system messages 始终位于 `Thread` 消息序列的开头前缀。普通 user / assistant / tool / compact 消息 MUST 位于该前缀之后，外部模块 SHALL NOT 在消息序列中间或尾部追加新的 system message。

#### Scenario: chat message 不会出现在 system prefix 之前
- **WHEN** AgentLoop 向 thread 写入 user、assistant、tool call 或 tool result 消息
- **THEN** 这些消息都会位于已有 system prefix 之后
- **THEN** `Thread` 的开头前缀顺序保持不变

### Requirement: Router 和 Session SHALL 只消费 thread-owned turn 结果
系统 SHALL 要求 Router 与 Session 只消费由 `Thread` 最终化后的 thread snapshot 与 turn 级结果。Router 和 Session SHALL NOT 在 `Thread` 之外组装 user / assistant / tool / error 消息，也 SHALL NOT 再根据增量消息 append 历史。

#### Scenario: 成功 turn 直接保存 thread snapshot
- **WHEN** 某个 turn 成功结束并生成对应的 thread snapshot 与 turn 级结果
- **THEN** Router 会直接消费该 turn 级结果
- **THEN** Session 会保存对应的 thread snapshot，而不是重新拼接提交消息

#### Scenario: 失败 turn 不由外部组装错误消息
- **WHEN** 某个 turn 在执行过程中失败并需要对用户返回失败结果
- **THEN** 失败结果边界由 `Thread` 自己最终化
- **THEN** Router 不会在 `Thread` 之外再补 user / assistant error 消息

### Requirement: Thread SHALL 持有当前 turn 的 message ownership
系统 SHALL 要求当前 turn 的 user input、assistant 输出、assistant tool-call message、tool result 与 compact replacement 都先进入 `Thread` 的当前 turn state，随后才能参与下一次 `llm.generate(...)` 或 turn finalization。

#### Scenario: tool result 进入下一轮请求视图
- **WHEN** AgentLoop 完成一次 tool 调用并拿到 tool result
- **THEN** 该 tool result 会先写入 `Thread` 的当前 turn state
- **THEN** 下一次 `llm.generate(...)` 读取到的 messages 会包含这条 thread-owned tool result
