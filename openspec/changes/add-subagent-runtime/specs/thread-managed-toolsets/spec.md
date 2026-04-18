## MODIFIED Requirements

### Requirement: System SHALL isolate loaded toolsets per internal thread
系统 SHALL 继续将 loaded toolsets 视为每个内部线程自己的持久化状态；该约束同样适用于 subagent child thread。某个 child thread 的 toolset 变化 SHALL NOT 影响其父线程，也 SHALL NOT 影响同一父线程下其他 child thread 的可见工具集合。

#### Scenario: child thread 与父线程的 toolset 状态相互隔离
- **WHEN** 某个 `browser` child thread 加载或持有 `browser` toolset
- **THEN** 父线程不会因此自动获得 `browser` 工具可见性
- **THEN** 父线程后续是否能看到这些工具，仍然只取决于父线程自己的 tool state

#### Scenario: 一个 child thread 的 toolset 变化不影响另一个 child thread
- **WHEN** 同一父线程下存在两个不同 profile 的 child thread
- **AND** 其中一个 child thread 的 loaded toolsets 发生变化
- **THEN** 另一个 child thread 的 visible tools 不受影响

### Requirement: 系统 SHALL 允许 `ThreadAgentKind` 为 child thread 预绑定默认工具集合
系统 SHALL 继续允许 `ThreadAgentKind` 在初始化阶段为线程预绑定默认工具集合；该机制同样 SHALL 适用于 subagent child thread。child thread 初始化后可见的默认工具，必须由它自己的 `ThreadAgentKind` 和 thread-scoped tool state 决定，而不是由父线程继承。

#### Scenario: `browser` child thread 初始化后自带浏览器工具绑定
- **WHEN** 系统创建并初始化一个 `browser` child thread
- **THEN** 该 child thread 初始化后的默认工具状态包含 `browser` 对应的工具集合
- **THEN** 这些默认工具绑定不需要从父线程复制

### Requirement: Agent loop SHALL refresh visible tools before each generation step
系统 SHALL 在 child thread 的每次模型生成前，继续基于该 child thread 自己的 tool state 刷新 visible tools。subagent 执行期间发生的 toolset 变化 SHALL 只作用于当前 child thread 后续步骤，而 SHALL NOT 回写父线程的 visible tool projection。

#### Scenario: child thread 在本轮内的 toolset 变化只作用于自己
- **WHEN** 某个 child thread 在当前执行过程中加载或卸载了一个 toolset
- **THEN** 该 child thread 下一次模型生成前会看到更新后的 visible tools
- **THEN** 父线程当前轮的 visible tool projection 不会因此被直接修改

### Requirement: System SHALL persist thread toolset state and tool call records
系统 SHALL 继续将 child thread 的 loaded toolsets 和结构化 tool call records 持久化为该 child thread 自己的线程真相。child thread 恢复后，系统 SHALL 能独立恢复其工具视图，而不依赖父线程同步重建同一份 tool state。

#### Scenario: child thread 恢复后独立重建工具视图
- **WHEN** 某个 child thread 已经落盘并保存了自己的 loaded toolsets
- **AND** 系统在后续流程中恢复该 child thread
- **THEN** 系统可以基于这个 child thread 自己的持久化 tool state 恢复 visible tools
- **THEN** 不需要先读取或复制父线程的 tool state 才能重建该 child thread

