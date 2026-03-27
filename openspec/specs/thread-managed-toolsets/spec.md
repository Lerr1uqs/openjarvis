# thread-managed-toolsets Specification

## Purpose
Define thread-scoped toolset management so non-basic capabilities are exposed as named toolsets, loaded and unloaded explicitly per internal thread, refreshed during the agent loop, and persisted for runtime reconstruction and audit.
## Requirements
### Requirement: Agent runtime SHALL expose a compact toolset catalog and explicit load/unload tools
The system SHALL present available program-defined toolsets to the model through a compact catalog prompt and SHALL always expose `load_toolset` and `unload_toolset` as agent-callable tools. The system SHALL NOT expose every non-basic tool schema before its toolset is loaded.

#### Scenario: Model sees toolset catalog before loading
- **WHEN** a thread starts with no non-basic toolsets loaded
- **THEN** the model request includes toolset catalog information and the `load_toolset` and `unload_toolset` tools
- **THEN** non-basic tool schemas from unloaded toolsets are not included in the visible tool list

### Requirement: System SHALL isolate loaded toolsets per internal thread
The system SHALL maintain loaded toolset state per internal thread identified by the existing thread resolution flow. Loading or unloading one toolset in one internal thread SHALL NOT change visible tool availability in any other internal thread.

#### Scenario: Two threads remain isolated
- **WHEN** thread A loads the `browser` toolset and thread B does not
- **THEN** thread A can see and call the `browser` toolset tools
- **THEN** thread B does not see those `browser` toolset tools unless it loads the same toolset itself

### Requirement: Agent loop SHALL refresh visible tools before each generation step
The system SHALL rebuild the visible tool list from the current thread runtime before each model generation step within one agent loop run.

#### Scenario: Loaded toolset becomes visible in the same turn
- **WHEN** the model calls `load_toolset` during an active ReAct turn
- **THEN** the next model generation step in that same turn includes the newly loaded toolset tools in the visible tool list

### Requirement: System SHALL support explicit agent-driven unload of toolsets
The system SHALL let the agent call `unload_toolset` for a loaded toolset in the current internal thread and SHALL remove that toolset's tools from the visible tool list after the unload succeeds.

#### Scenario: Unloaded toolset disappears from visible tools
- **WHEN** the agent successfully calls `unload_toolset` for the `browser` toolset in the current thread
- **THEN** later model generation steps in that thread do not include `browser` toolset tools unless the toolset is loaded again

### Requirement: System SHALL persist thread toolset state and tool call records
The system SHALL persist loaded toolset state and structured tool call records as part of thread state so runtime reconstruction and audit are possible.

#### Scenario: Thread runtime can be reconstructed from persisted state
- **WHEN** a thread with loaded toolsets is reloaded from persisted state
- **THEN** the runtime can restore that thread's loaded toolset set before serving the next model request
- **THEN** the persisted thread record contains structured evidence of the tool load, unload, and execution history

### Requirement: Program-defined toolsets SHALL provide stable routed tool names
The system SHALL expose program-defined toolset tools under stable routed names that remain unambiguous when multiple toolsets are loaded in the same thread.

#### Scenario: Tool names do not collide across loaded toolsets
- **WHEN** two loaded toolsets contain tools with similar underlying raw names
- **THEN** the model-visible tool names remain unique and route to the correct toolset-owned handler

### Requirement: 系统 SHALL 支持线程运行时的条件化工具可见性
系统 SHALL 在 thread-scoped tool projection 阶段支持条件化工具可见性。某个工具即使已经注册，也 SHALL 可以根据线程运行时状态、配置开关或上下文预算决定当前是否对模型可见。

#### Scenario: 工具已注册但当前线程不可见
- **WHEN** 某个工具已经注册到工具运行时，但其当前线程可见性条件不满足
- **THEN** 该工具不会出现在当前线程发送给模型的 visible tool list 中
- **THEN** 其他满足条件的线程仍然可以独立看到该工具

### Requirement: Agent loop SHALL 在每次生成前刷新工具可见性投影
系统 SHALL 在每次模型生成前重新计算当前线程的 visible tool projection，而不是只基于静态注册结果。该刷新 SHALL 同时考虑已加载 toolset 和条件化工具显隐。

#### Scenario: 上下文预算变化影响工具可见性
- **WHEN** 当前线程的上下文预算或特性开关发生变化
- **THEN** 下一次模型生成前会重新计算该线程的 visible tool list
- **THEN** 条件满足的工具可以在同一线程后续步骤中出现或消失

