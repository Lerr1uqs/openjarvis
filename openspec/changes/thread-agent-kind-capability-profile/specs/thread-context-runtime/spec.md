## MODIFIED Requirements

### Requirement: 系统 SHALL 在线程初始化时将稳定 `System` messages 直接写入 `ThreadContext`
系统 SHALL 在新线程初始化或重初始化时，将基础 system prompt 与其他稳定初始化提示直接写入 `ThreadContext.messages()` 的开头前缀并持久化。系统 SHALL NOT 为这些稳定前缀再维护独立的 request context 成员、snapshot 字段或同类单独存储结构。线程初始化时使用的基础 system prompt、默认工具能力和 feature prompt 注入范围 SHALL 由当前 `ThreadAgentKind` 对应的 capability profile 决定。

#### Scenario: 新线程创建时写入稳定 `System` 前缀
- **WHEN** Session 或 Router 首次为某个 internal thread 创建 `ThreadContext`
- **THEN** 系统会先基于该线程的 `ThreadAgentKind` capability profile 构造稳定 `System` messages
- **THEN** 这些消息会直接写入 `ThreadContext.messages()` 的开头前缀
- **THEN** 后续同一线程的普通 user / assistant / tool 消息都会位于该前缀之后

### Requirement: `ThreadContext` SHALL 只维护单一持久化消息序列
系统 SHALL 将稳定 `System` 前缀与后续 conversation messages 保存在同一条持久化消息序列中。系统 MAY 在导出、可见性判断或 compact 时按 message role 区分它们，但 SHALL NOT 额外要求一份独立的 request context 持久化结构。初始化时写入哪些稳定前缀 SHALL 受当前 `ThreadAgentKind` capability profile 约束。

#### Scenario: 线程恢复后直接得到完整消息序列
- **WHEN** 某个线程从持久化层恢复
- **THEN** 调用方会直接得到一条包含稳定 `System` 前缀和后续 conversation history 的持久化消息序列
- **THEN** 调用方不需要再从额外 prefix 字段或 request context 结构重新拼装消息

### Requirement: 系统 SHALL 在线程初始化时按 kind profile 约束 feature 注入边界
系统 SHALL 在解析线程初始化 feature 时，同时考虑 `ThreadAgentKind` capability profile 的允许范围。对于 main thread，配置驱动或 resolver 产生的 feature 集合 SHALL 与该 kind 允许范围求交；对于 subagent 或其他非主线程 kind，系统 SHALL NOT 让不在其 capability profile 中的 feature 被初始化。

#### Scenario: `Browser` 线程不会初始化不被允许的 feature
- **WHEN** 系统初始化一个 `Browser` 线程
- **THEN** 该线程只会获得其 kind profile 允许的 feature
- **THEN** 即使全局默认或其他线程启用了 `memory`、`skill` 或 `subagent`
- **THEN** 该 `Browser` 线程也不会初始化这些不被其 profile 允许的 feature
