## ADDED Requirements

### Requirement: 系统 SHALL 在线程初始化时建立线程级 request context snapshot
系统 SHALL 为每个 `ThreadContext` 建立线程级 request context snapshot，作为该线程后续请求组装的稳定前缀事实来源。首版 request context snapshot SHALL 至少包含当前 system prompt 的线程级快照，并在同一线程后续轮次中保持稳定，直到被显式迁移或重建。

#### Scenario: 新线程创建时初始化 request context
- **WHEN** Session 或 Router 首次为某个 internal thread 创建 `ThreadContext`
- **THEN** 该线程会同步建立自己的 request context snapshot
- **THEN** 首版 snapshot 中包含当前 system prompt 的线程级快照

### Requirement: Agent loop SHALL 基于 `ThreadContext + current user input` 组装请求
系统 SHALL 让 AgentLoop 主链路接收 `ThreadContext` 与当前轮 user input，而不是接收由外部预组装的 `MessageContext`。发送给 LLM 的 messages SHALL 通过 `ThreadContext.messages()` 从线程内部统一导出，AgentLoop SHALL 通过 `ThreadContext.push_message(...)` 注入当前轮 user input 与 turn 内正式消息，而 SHALL NOT 在普通请求轮次中自动注入 active memory 正文、摘要或其他 memory recall message。

#### Scenario: worker 只传当前 user input
- **WHEN** worker 准备把某个线程请求交给 AgentLoop
- **THEN** worker 不需要先构造完整的 `MessageContext`
- **THEN** AgentLoop 会基于目标 `ThreadContext` 和当前 user input 自行组装本轮 LLM request
- **THEN** Router 不会在转发过程中主动操控 memory 或其他 message 上下文

### Requirement: 线程级 request context SHALL 与 conversation history 分层
线程级 request context SHALL NOT 作为普通 `ConversationTurn` 消息落盘，也 SHALL NOT 被 `ThreadContext.load_messages()` 视为 chat history 的一部分。线程恢复后，系统 SHALL 可以分别恢复 request context snapshot 与 conversation history，而不是把二者混成一条扁平消息序列。

#### Scenario: request context 不进入 turn 历史
- **WHEN** 某个线程完成一轮普通 user / assistant 对话并被持久化
- **THEN** 落盘的 `ConversationTurn` 中只包含该轮 conversation messages
- **THEN** 线程级 request context 不会作为重复前缀被写入每个 turn

### Requirement: request context 与 active memory catalog SHALL NOT 成为 compact source history
系统 SHALL 继续只对 thread conversation 的 chat history 执行 compact。线程级 request context 以及其中稳定持久化的 active memory catalog SHALL NOT 被当作 compact source chat history，也 SHALL NOT 被 compact 结果替换。

#### Scenario: runtime compact 只替换 conversation chat history
- **WHEN** 某个线程触发 runtime compact 或模型主动调用 `compact`
- **THEN** compact 输入只包含该线程 conversation 中的 chat history
- **THEN** 线程级 request context 和 active memory catalog 不会出现在 compact source 或 compact replacement turn 中

### Requirement: memory SHALL NOT 使用 request-time 动态注入正文
系统 SHALL 将 active memory 视为线程初始化阶段固化进 request context snapshot 的稳定 catalog，而不是普通请求轮次中的动态正文注入。模型若需要记忆详情，SHALL 通过显式加载 `memory` toolset 并调用 `memory_get`、`memory_search`、`memory_list` 等工具渐进式读取。

#### Scenario: 命中 active memory keyword 时不会自动追加正文
- **WHEN** 某一轮用户输入命中 active memory keyword
- **THEN** AgentLoop 不会自动向请求中追加对应 memory 正文或摘要
- **THEN** 模型只能通过 memory tool 渐进式读取详情
