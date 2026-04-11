## MODIFIED Requirements

### Requirement: Agent runtime SHALL expose a compact toolset catalog and explicit load/unload tools
系统 SHALL 通过当前 `Thread` 对模型暴露 toolset catalog 以及始终可见的 `load_toolset` / `unload_toolset` 工具。toolset catalog 的构造与当前线程可见工具集合 SHALL 由 `Thread` 基于自己的 tool state 驱动，再借助全局 `ToolRegistry` 解析全局 catalog 信息；系统 SHALL NOT 为每个 thread 构造独立 registry。

#### Scenario: 模型在空线程中先看到 catalog 和 load/unload
- **WHEN** 某个线程当前没有加载任何非基础 toolset
- **THEN** 该线程导出的 visible tool list 中包含 toolset catalog 信息以及 `load_toolset`、`unload_toolset`
- **THEN** 未加载 toolset 的非基础工具 schema 不会提前暴露

### Requirement: System SHALL isolate loaded toolsets per internal thread
系统 SHALL 将 loaded toolsets 视为 `Thread` 自己的持久化状态，而不是全局 `ToolRegistry` 的线程内部 map。加载或卸载某个 toolset 只会改动当前线程自己的 tool state，且 SHALL NOT 影响其他线程通过同一个全局 `ToolRegistry` 看到的工具可见性。

#### Scenario: 两个线程共享一个全局 registry 但状态隔离
- **WHEN** thread A 加载了 `browser` toolset，而 thread B 没有加载
- **THEN** thread A 通过自己的 `Thread` 状态可以看到并调用 `browser` 工具
- **THEN** thread B 不会因为共享同一个全局 `ToolRegistry` 就看到这些工具

### Requirement: Agent loop SHALL refresh visible tools before each generation step
系统 SHALL 在每次模型生成前，通过当前 `Thread.visible_tools()` 重新计算该线程的 visible tool projection。这个 projection SHALL 同时考虑当前线程已加载的 toolset、条件化工具显隐和上下文预算；AgentLoop SHALL NOT 直接对 `ToolRegistry` 做 thread-scoped 的可见性计算。

#### Scenario: 同一 turn 内 load_toolset 后下次生成可见
- **WHEN** 模型在当前 turn 中调用 `load_toolset` 成功加载某个 toolset
- **THEN** 下一次模型生成前，AgentLoop 会通过 `Thread.visible_tools()` 刷新 visible tools
- **THEN** 新加载的 toolset 工具会在同一线程后续步骤中出现

### Requirement: System SHALL support explicit agent-driven unload of toolsets
系统 SHALL 让 agent 继续可以在当前线程中显式调用 `unload_toolset`。该调用修改的是当前 `Thread` 的 loaded toolset state；卸载成功后，后续工具可见性刷新 SHALL 基于这个新的 thread state 生效。

#### Scenario: unload 只影响当前 thread 后续可见工具
- **WHEN** 当前线程成功执行 `unload_toolset(browser)`
- **THEN** 该线程下一次刷新 visible tools 时不再包含 `browser` 工具
- **THEN** 其他线程已经加载的 `browser` toolset 状态不受影响

### Requirement: System SHALL persist thread toolset state and tool call records
系统 SHALL 将 loaded toolset state 和结构化 tool call audit 持久化为 `Thread` 自己的状态。线程恢复后，`Thread` SHALL 基于持久化 state 继续驱动工具可见性与工具调用，而不是要求 `ToolRegistry` 先恢复 thread-scoped runtime map。

#### Scenario: 线程恢复后仍能按已持久化 state 重建工具视图
- **WHEN** 某个线程持久化时已经加载了 `memory` toolset，并保存了对应 tool call audit
- **THEN** 线程恢复并重新 attach 全局 `ToolRegistry` 后
- **THEN** `Thread` 可以基于自己的持久化 tool state 恢复可见工具和审计上下文

### Requirement: 系统 SHALL 支持线程运行时的条件化工具可见性
系统 SHALL 在 thread-scoped tool projection 阶段支持条件化工具可见性。某个工具即使已经注册在全局 `ToolRegistry` 中，也 SHALL 由当前 `Thread` 根据自己的运行时状态、配置开关、上下文预算和 loaded toolsets 决定当前是否可见。

#### Scenario: 注册在全局 registry 的工具仍可对某线程隐藏
- **WHEN** 某个工具已经注册到全局 `ToolRegistry`，但当前线程的可见性条件不满足
- **THEN** 该工具不会出现在该线程导出的 visible tool list 中
- **THEN** 其他满足条件的线程仍然可以独立看到该工具

### Requirement: Agent loop SHALL 在每次生成前刷新工具可见性投影
系统 SHALL 在每次模型生成前重新计算当前线程的 visible tool projection，而不是复用 Agent 持有的静态工具列表。该刷新 SHALL 由 `Thread` 触发并通过全局 `ToolRegistry` 解析工具定义，确保 Agent 只是消费 thread-owned projection。

#### Scenario: 上下文预算变化由 thread 触发工具显隐变化
- **WHEN** 当前线程的上下文预算、feature 开关或 loaded toolsets 在一个 turn 内发生变化
- **THEN** 下一次模型生成前会由 `Thread` 重新计算该线程的 visible tool list
- **THEN** Agent 不需要自己维护第二份 thread-scoped tool projection 缓存

## ADDED Requirements

### Requirement: Thread SHALL 通过全局 `ToolRegistry` 执行线程工具调用
系统 SHALL 让 `Thread` 通过全局 `ToolRegistry` 解析并执行当前线程的工具调用。AgentLoop SHALL 将工具调用请求交给 `Thread`，再由 `Thread` 基于自己的 tool state、thread identity 和全局 registry 完成 handler 解析、调用执行和审计记录。

#### Scenario: 线程调用已加载 toolset 的 routed tool
- **WHEN** 当前线程已经加载某个 toolset，且模型在 turn 中调用该 toolset 暴露出的 routed tool name
- **THEN** AgentLoop 会把该请求交给 `Thread`
- **THEN** `Thread` 会通过全局 `ToolRegistry` 找到正确 handler 并完成调用，同时把结果记录到当前线程审计状态
