## ADDED Requirements

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
