## ADDED Requirements

### Requirement: 系统 SHALL 在线程初始化时将稳定 `System` messages 直接写入 `ThreadContext`
系统 SHALL 在新线程初始化或重初始化时，将基础 system prompt 与其他稳定初始化提示直接写入 `ThreadContext.messages()` 的开头前缀并持久化。系统 SHALL NOT 为这些稳定前缀再维护独立的 request context 成员、snapshot 字段或同类单独存储结构。

#### Scenario: 新线程创建时写入稳定 `System` 前缀
- **WHEN** Session 或 Router 首次为某个 internal thread 创建 `ThreadContext`
- **THEN** 系统会先构造该线程的稳定 `System` messages
- **THEN** 这些消息会直接写入 `ThreadContext.messages()` 的开头前缀
- **THEN** 后续同一线程的普通 user / assistant / tool 消息都会位于该前缀之后

### Requirement: Agent loop SHALL 基于 `ThreadContext + current user input` 组装请求
系统 SHALL 让 AgentLoop 主链路接收 `ThreadContext` 与当前轮 user input，而不是接收由外部预组装的 `MessageContext`。发送给 LLM 的 messages SHALL 通过 `ThreadContext.messages()` 从线程内部统一导出，AgentLoop SHALL 通过 `ThreadContext.push_message(...)` 注入当前轮 user input 与 turn 内正式消息，而 SHALL NOT 在普通请求轮次中自动注入 active memory 正文、摘要或其他 memory recall message。

#### Scenario: worker 只传当前 user input
- **WHEN** worker 准备把某个线程请求交给 AgentLoop
- **THEN** worker 不需要先构造完整的 `MessageContext`
- **THEN** AgentLoop 会基于目标 `ThreadContext` 和当前 user input 自行组装本轮 LLM request
- **THEN** Router 不会在转发过程中主动操控 memory 或其他 message 上下文

### Requirement: `ThreadContext` SHALL 只维护单一持久化消息序列
系统 SHALL 将稳定 `System` 前缀与后续 conversation messages 保存在同一条持久化消息序列中。系统 MAY 在导出、可见性判断或 compact 时按 message role 区分它们，但 SHALL NOT 额外要求一份独立的 request context 持久化结构。

#### Scenario: 线程恢复后直接得到完整消息序列
- **WHEN** 某个线程从持久化层恢复
- **THEN** 调用方会直接得到一条包含稳定 `System` 前缀和后续 conversation history 的持久化消息序列
- **THEN** 调用方不需要再从额外 prefix 字段或 request context 结构重新拼装消息

### Requirement: 稳定 `System` 前缀 SHALL NOT 成为 compact source history
系统 SHALL 继续只对 thread conversation 中 role 不是 `System` 的消息执行 compact。初始化阶段写入的稳定 `System` 前缀 SHALL NOT 被当作 compact source history，也 SHALL NOT 被 compact 结果替换。

#### Scenario: runtime compact 只替换非 `System` 历史
- **WHEN** 某个线程触发 runtime compact 或模型主动调用 `compact`
- **THEN** compact 输入只包含该线程中 role 不是 `System` 的 conversation history
- **THEN** 稳定 `System` 前缀保持不变

### Requirement: memory SHALL NOT 使用 request-time 动态注入正文
系统 SHALL 将 active memory 视为线程初始化阶段写入稳定 `System` 前缀的 catalog，而不是普通请求轮次中的动态正文注入。模型若需要记忆详情，SHALL 通过显式加载 `memory` toolset 并调用 `memory_get`、`memory_search`、`memory_list` 等工具渐进式读取。

#### Scenario: 命中 active memory keyword 时不会自动追加正文
- **WHEN** 某一轮用户输入命中 active memory keyword
- **THEN** AgentLoop 不会自动向请求中追加对应 memory 正文或摘要
- **THEN** 模型只能通过 memory tool 渐进式读取详情
