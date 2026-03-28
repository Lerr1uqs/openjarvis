## ADDED Requirements

### Requirement: 系统 SHALL 由 `ThreadContext` 持有线程工具集状态
系统 SHALL 将 `loaded_toolsets`、线程工具可见性投影输入以及后续权限策略相关的线程工具状态收口到 `ThreadContext` 中。`ToolRegistry` SHALL NOT 继续作为这些线程工具状态的长期宿主。

#### Scenario: 工具集状态保留在线程上下文中
- **WHEN** 某个线程加载或卸载一个 toolset
- **THEN** 该线程的 toolset 状态会记录在自己的 `ThreadContext`
- **THEN** 其他线程即使共享同一个 `ToolRegistry` 也不会共享该状态

### Requirement: `ToolRegistry` SHALL 作为全局工具目录被 `ThreadContext` 调用
系统 SHALL 让 `ToolRegistry` 负责全局工具注册、catalog 和 handler 解析，而线程级 visible tool 计算、权限审批和工具调用入口 SHALL 先经过 `ThreadContext`，再由 `ThreadContext` 委托 `ToolRegistry` 完成全局能力解析。

#### Scenario: 线程通过上下文委托全局工具池
- **WHEN** AgentLoop 需要为某个线程计算可见工具或执行一个工具调用
- **THEN** 线程会先通过自己的 `ThreadContext` 做线程级状态判断
- **THEN** `ThreadContext` 再调用全局 `ToolRegistry` 完成对应工具或 toolset 的解析与执行
