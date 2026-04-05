## ADDED Requirements

### Requirement: active memory catalog SHALL 作为线程初始化 request context snapshot 的固定组成部分
系统 SHALL 在新线程初始化或线程被清空后的重初始化流程中，从工作区下 `./.openjarvis/memory` 中的 active memory 仓库派生当前可用的 active memory catalog，并将其作为线程初始化 request context snapshot 的固定组成部分持久化到该线程。该 snapshot 一旦完成初始化，在该线程后续普通轮次中 SHALL 保持稳定，直到线程被显式重初始化。

#### Scenario: 清空上下文后的线程会重新获得最新 active memory catalog
- **WHEN** 某个线程被清空上下文并重新初始化，且此时 `./.openjarvis/memory/active` 已经新增了新的关键词词表
- **THEN** 该线程新的 request context snapshot 中会包含最新的 active memory catalog
- **THEN** 之前旧 snapshot 中的 active memory catalog 不会继续沿用

### Requirement: 线程级 request context snapshot SHALL NOT 成为 compact source history
系统 SHALL 继续只对 thread conversation 的 chat history 执行 compact。线程级 request context snapshot，包括基础 system prompt 与 active memory catalog，SHALL NOT 被当作 compact source chat history，也 SHALL NOT 被 compact 结果替换。相比之下，通过 memory 工具产生的普通 toolcall / tool_result messages SHALL 继续作为 conversation history 的一部分存在。

#### Scenario: active memory catalog 不参与 compact，但 memory tool 结果会参与普通历史处理
- **WHEN** 某个线程先使用初始化注入的 active memory catalog 作为 system prompt，再调用 `memory_get` 读取正文，后续又触发一次 compact
- **THEN** compact 输入不会包含该线程的 active memory catalog system prompt
- **THEN** 由 `memory_get` 产生的普通 toolcall / tool_result 消息仍然会按普通 conversation history 处理

## MODIFIED Requirements

### Requirement: 系统 SHALL 在线程初始化时建立线程级 request context snapshot
系统 SHALL 为每个 `ThreadContext` 建立线程级 request context snapshot，作为该线程后续请求组装的稳定前缀事实来源。该 snapshot SHALL 至少包含当前基础 system prompt 的线程级快照，并在有 active memory 可用时包含由工作区下 `./.openjarvis/memory/active` 派生出的 active memory catalog。该 snapshot 在同一线程后续轮次中 SHALL 保持稳定，直到被显式迁移、重建或线程被重初始化。

#### Scenario: 新线程创建时初始化 request context
- **WHEN** Session 或 Router 首次为某个 internal thread 创建 `ThreadContext`
- **THEN** 该线程会同步建立自己的 request context snapshot
- **THEN** snapshot 中包含当前基础 system prompt 的线程级快照
- **THEN** 若 `./.openjarvis/memory/active` 中存在可用词表，snapshot 中也包含对应的 active memory catalog

### Requirement: Agent loop SHALL 基于 `ThreadContext + current user input` 组装请求
系统 SHALL 让 AgentLoop 主链路接收 `ThreadContext` 与当前轮 user input，而不是接收由外部预组装的 `MessageContext`。发送给 LLM 的 messages SHALL 通过 `ThreadContext.messages()` 从线程内部统一导出，AgentLoop SHALL 只负责在当前轮追加 user input 与其他运行期消息，而 SHALL NOT 再负责主动注入 active memory 正文、摘要或其他 request-time memory recall message。

#### Scenario: worker 只传当前 user input
- **WHEN** worker 准备把某个线程请求交给 AgentLoop
- **THEN** worker 不需要先构造完整的 `MessageContext`
- **THEN** AgentLoop 会基于目标 `ThreadContext` 和当前 user input 自行组装本轮 LLM request
- **THEN** 当前轮请求不会因为命中 active memory keyword 而由 loop 自动追加对应 memory 正文
- **THEN** Router 不会在转发过程中主动操控 memory 或其他 message 上下文

## REMOVED Requirements

### Requirement: request context 与 request-time memory SHALL NOT 成为 compact source history
**Reason**: 旧 requirement 绑定了“request-time 自动注入 memory”的模型；新的 memory 语义中，active memory 以线程初始化 catalog 的形式进入 request context，而 memory 正文只能通过工具读取。
**Migration**: 改为实现新的“线程级 request context snapshot SHALL NOT 成为 compact source history”要求；并将 `memory_get` 等工具结果视为普通 conversation history。

### Requirement: request-time memory SHALL 保持动态注入而非线程初始化固化
**Reason**: 该方案已被放弃。新的 active memory 模型不再进行 request-time 主动注入，而是在线程初始化或重初始化时固化关键词词表，并通过 memory 工具执行渐进式披露。
**Migration**: 删除 AgentLoop 中面向 active memory 的自动 recall / live memory 注入逻辑，改为在 thread 初始化阶段构建 active memory catalog，并通过 `memory_get`、`memory_search`、`memory_write`、`memory_list` 提供正文访问。
