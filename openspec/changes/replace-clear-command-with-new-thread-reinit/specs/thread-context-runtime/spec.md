## MODIFIED Requirements

### Requirement: 系统 SHALL 在线程初始化时将稳定 `System` messages 直接写入 `ThreadContext`
系统 SHALL 在新线程初始化或显式重初始化时，将基础 system prompt 与其他稳定初始化提示直接写入 `ThreadContext.messages()` 的开头前缀并持久化。系统 SHALL NOT 为这些稳定前缀再维护独立的 request context 成员、snapshot 字段或同类单独存储结构。

#### Scenario: 新线程创建时写入稳定 `System` 前缀
- **WHEN** Session 或 Router 首次为某个 internal thread 创建 `ThreadContext`
- **THEN** 系统会先构造该线程的稳定 `System` messages
- **THEN** 这些消息会直接写入 `ThreadContext.messages()` 的开头前缀
- **THEN** 后续同一线程的普通 user / assistant / tool 消息都会位于该前缀之后

#### Scenario: 显式重初始化后立即恢复稳定 `System` 前缀
- **WHEN** 一个已初始化线程通过显式重初始化命令开启新的当前 thread
- **THEN** 系统会先清空旧的普通历史消息和线程级运行态
- **THEN** 系统会在命令返回前重新写入与当前 `ThreadAgentKind` 一致的稳定 `System` 前缀
- **THEN** 持久化层里不会留下一个没有稳定前缀的空 thread 作为命令执行结果
