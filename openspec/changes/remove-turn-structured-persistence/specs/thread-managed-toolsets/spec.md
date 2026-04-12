## MODIFIED Requirements

### Requirement: Agent loop SHALL refresh visible tools before each generation step
The system SHALL rebuild the visible tool list from the current thread runtime before each model generation step within one active request. Tool visibility refresh SHALL depend on thread-owned persisted state and current runtime conditions, and SHALL NOT require any public turn structure.

#### Scenario: Loaded toolset becomes visible in the same active request
- **WHEN** the model calls `load_toolset` during one active request on the current thread
- **THEN** the next model generation step in that same active request includes the newly loaded toolset tools in the visible tool list

### Requirement: System SHALL persist thread toolset state and tool call records
The system SHALL persist loaded toolset state and structured tool call records as part of thread state so runtime reconstruction and audit are possible. This persisted audit model SHALL NOT depend on `ConversationTurn`, `ThreadCurrentTurn`, `ThreadFinalizedTurn`, or any other turn structure.

#### Scenario: Thread runtime can be reconstructed from persisted state without turn records
- **WHEN** a thread with loaded toolsets is reloaded from persisted state
- **THEN** the runtime can restore that thread's loaded toolset set before serving the next model request
- **THEN** the persisted thread record contains structured evidence of the tool load, unload, and execution history without relying on turn structures

## ADDED Requirements

### Requirement: tool audit persistence SHALL NOT 依赖 turn finalize
系统 MUST 让工具审计事实通过 thread-owned state mutation 进入正式线程状态，而 SHALL NOT 依赖 turn finalize、finalized snapshot commit 或其他 turn 收尾接口才变成持久化事实。

#### Scenario: tool event 在记录成功后已成为正式线程状态
- **WHEN** 当前线程成功记录一条工具加载、卸载或执行审计事件
- **THEN** 该事件已经进入线程正式状态并完成持久化
- **THEN** 系统不需要等待任何 turn finalize 才能在后续恢复中看到这条审计记录
