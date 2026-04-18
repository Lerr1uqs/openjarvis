## MODIFIED Requirements

### Requirement: Agent runtime SHALL expose a compact toolset catalog and explicit load/unload tools
The system SHALL present available program-defined toolsets to the model through a compact catalog prompt and SHALL expose `load_toolset` and `unload_toolset` only when the current thread agent kind has optional toolsets available in its capability profile. The system SHALL NOT expose every non-basic tool schema before its toolset is loaded, and SHALL NOT list or allow loading toolsets that fall outside the current thread agent kind's allowed toolset range.

#### Scenario: Main thread sees only kind-allowed optional toolsets
- **WHEN** a `Main` thread starts with no optional toolsets loaded
- **THEN** the model request includes only the toolsets allowed by the `Main` kind profile in the toolset catalog
- **THEN** the thread can see `load_toolset` and `unload_toolset` only if that kind profile exposes optional toolsets
- **THEN** non-basic tool schemas from unloaded toolsets are not included in the visible tool list

#### Scenario: Main thread cannot directly load browser toolset
- **WHEN** a `Main` thread needs browser capability
- **THEN** the toolset catalog does not list `browser` as one directly loadable optional toolset
- **THEN** calling `load_toolset` with `browser` is rejected
- **THEN** the supported path is to use the `subagent` capability to delegate the work to a `Browser` kind child thread

### Requirement: System SHALL support explicit agent-driven unload of toolsets
The system SHALL let the agent call `unload_toolset` only for toolsets that are both loaded in the current thread and allowed to be optional under the current thread agent kind's capability profile. The system SHALL NOT let one thread unload or remove a tool capability that belongs to that kind's default bound capability truth.

#### Scenario: Default bound capability is not treated as optional unload target
- **WHEN** a thread owns one tool capability because its `ThreadAgentKind` profile binds it by default
- **THEN** that capability is not treated as a normal optional unload target
- **THEN** later model generation steps continue to enforce the kind-owned capability boundary

### Requirement: 系统 SHALL 支持线程运行时的条件化工具可见性
系统 SHALL 在 thread-scoped tool projection 阶段支持条件化工具可见性。某个工具即使已经注册，也 SHALL 可以根据线程运行时状态、配置开关、上下文预算以及当前线程 agent kind capability profile 决定当前是否对模型可见。

#### Scenario: 工具已注册但超出当前 kind 允许范围
- **WHEN** 某个工具已经注册到工具运行时，但它不在当前线程 agent kind capability profile 允许范围内
- **THEN** 该工具不会出现在当前线程发送给模型的 visible tool list 中
- **THEN** 其他允许该工具的 kind 线程仍然可以独立看到它
