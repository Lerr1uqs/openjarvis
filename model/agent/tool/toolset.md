# Toolset

## 定位

- `Toolset` 是比单个工具更高一层的能力包。
- 它存在的目的不是分组好看，而是支持线程级渐进式加载，控制上下文膨胀。

## 边界

- 负责描述一组工具如何作为整体被发现、加载、卸载。
- 不负责 ReAct 循环，不负责线程持久化。
- 当前线程是否已加载某个 toolset，以 `ThreadContext.state.tools.loaded_toolsets` 为准。

## 关键概念

- `ToolsetCatalogEntry`
  toolset 的最小目录信息，只保留名字和描述。
- `load_toolset / unload_toolset`
  模型可调用的线程级控制工具。
- `ToolsetRuntime`
  toolset 卸载时的清理扩展点。

## 核心能力

- 先向模型暴露 toolset 目录，而不是一次性暴露全部工具。
- 按线程加载指定 toolset，让其中工具在后续步骤可见。
- 卸载后把工具从当前线程的可见集合中移除。
- 静态 toolset 和 MCP server 都能被统一看成 toolset。

## 使用方式

- 适合做成 toolset 的是成套能力，比如 `browser` 或某个 MCP server。
- 不适合做成 toolset 的是始终都要可见的基础工具。
