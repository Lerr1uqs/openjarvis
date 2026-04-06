## MODIFIED Requirements

### Requirement: 系统 SHALL 对线程 `chat` 历史执行 token 感知的 runtime compact
系统 SHALL 在每次发送 LLM 请求前基于 thread-owned 完整请求视图估算上下文占用，并在达到 compact 运行阈值时对当前线程的 active non-system message view 执行 compact。compact source SHALL 包含 thread 中已持久化的 non-system history 与当前 turn request-visible non-system messages；稳定 system prefix SHALL NOT 被 compact。

#### Scenario: 到达 runtime 阈值时压缩 thread active non-system view
- **WHEN** 当前线程的完整请求估算 token 占用达到 compact 运行阈值
- **THEN** 系统会在发送下一次 LLM 请求前对当前线程的 active non-system messages 执行 compact
- **THEN** 稳定 system prefix 保持不变

#### Scenario: 当前 turn 消息也参与 compact 输入
- **WHEN** 当前 turn 中已经产生 assistant/tool 相关 non-system messages，且此时触发 runtime compact
- **THEN** 这些 thread-owned current-turn messages 会与已持久化 non-system history 一起参与 compact
- **THEN** compact 后的 active view 继续由 `Thread` 统一持有

### Requirement: `auto_compact` 关闭时 SHALL NOT 暴露 compact tool 或容量信息给模型
当 `auto_compact` 未开启时，系统 SHALL 继续保留 runtime compact，但 SHALL NOT 向模型暴露 compact tool，也 SHALL NOT 通过 loop-local transient system messages 向模型注入容量信息。

#### Scenario: 未开启 auto-compact 时模型无感知
- **WHEN** 当前配置中 `auto_compact` 为关闭状态
- **THEN** runtime compact 仍可在需要时执行
- **THEN** 当前模型请求中看不到 compact tool
- **THEN** 当前模型请求中不存在额外的 transient 容量 system prompt

### Requirement: `auto_compact` 开启时 SHALL 允许模型主动触发 compact
当 `auto_compact` 开启且当前线程预算到达 compact tool 的可见阈值时，系统 SHALL 向模型暴露 compact tool，让模型可以自行选择压缩时机。该可见性 SHALL 基于 thread-owned request view 决定，而 SHALL NOT 依赖 `AgentLoop` 额外维护 transient request system messages。

#### Scenario: 开启 auto-compact 后模型可见 compact tool
- **WHEN** `auto_compact` 已开启且当前线程预算达到 compact tool 的可见阈值
- **THEN** 当前模型请求中可见 compact tool
- **THEN** 当前模型请求是否可见该工具只由 thread-owned request view 与预算判断决定
- **THEN** loop 不会再为此单独注入 transient request system prompt
