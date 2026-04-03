## ADDED Requirements

### Requirement: 系统 SHALL 通过外部 `ContextCompactor` component 执行 thread compact
系统 SHALL 通过可外部初始化并串联调用的 `ContextCompactor` component 执行 thread compact，而不是要求 `AgentLoop` 长期持有 compact manager 作为内部成员。`AgentLoop` 中的 compact 行为 SHALL 通过显式调用该 component 完成。

#### Scenario: AgentLoop 显式调用 compactor 完成 compact
- **WHEN** AgentLoop 在某一轮中决定对当前线程执行 compact
- **THEN** loop 会显式构造或获得一个 `ContextCompactor` 并调用它
- **THEN** compact 的实际执行不再依赖 `AgentLoop` 长期持有的 compact manager 成员

### Requirement: runtime compact 与模型触发 compact SHALL 共用同一 compactor execution contract
系统 SHALL 让 runtime 阈值触发的 compact 与模型主动调用 `compact` tool 触发的 compact 共用同一套 compactor execution contract。两条路径可以保留各自的事件包装和后处理差异，但实际 compact 执行 SHALL 复用同一个 component 入口。

#### Scenario: 两条 compact 路径复用同一执行入口
- **WHEN** 某次 compact 由 runtime threshold 触发，或由模型主动调用 `compact` 触发
- **THEN** 两条路径都会调用同一个 `ContextCompactor` execution contract
- **THEN** compact summary、replacement turn 和 compact outcome 的语义保持一致
