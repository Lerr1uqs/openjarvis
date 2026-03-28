# MCP

## 定位

- `mcp` 是外部 MCP server 的托管层。
- 它解决的是“怎么把远端工具稳定接进本地工具系统”，不是定义业务工具本身。

## 边界

- 负责 server 定义、连接、探活、工具发现、调用转发、名字映射。
- 不负责线程级 toolset 真相，不负责业务 prompt，不负责远端工具语义设计。

## 关键概念

- `McpServerDefinition`
  一个 MCP server 的静态定义。
- `McpManager`
  MCP 运行时管理器。
- `McpServerState`
  `disabled / healthy / unhealthy` 三态健康视图。
- `McpToolHandler`
  MCP 工具在本地 registry 中的执行代理。
- `mcp__<server>__<tool>`
  远端工具映射到本地后的命名空间名称。

## 核心能力

- 支持 `stdio` 和 `streamable_http` 两种传输。
- 启用 server 时先探活，再发现工具。
- 只有 healthy 且 enabled 的 server 才能把工具暴露出来。
- 把本地工具调用转发为远端 `call_tool`。

## 使用方式

- 在 `ToolRegistry` 里，MCP server 被看作一种特殊 toolset。
- 某线程是否加载某个 MCP toolset，仍由该线程自己的 `ThreadContext` 决定。
