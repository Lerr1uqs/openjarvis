# agent/tool/mcp 模块总览

## 作用

`mcp/` 负责把外部 MCP server 纳入 OpenJarvis 的工具体系。它解决的是“怎么连接远端工具、怎么探活、怎么把远端工具映射成本地可见工具名”。

## 子模块

- `demo.rs`
  演示与测试用 MCP server。用于本地协议验证，不是生产工具实现。

## 核心概念

- `MCP Server`
  提供一组远端工具的服务端实例。
- `McpServerDefinition`
  MCP server 的静态配置定义，描述它怎么连接、是否启用、入口在哪。
- `McpTransport`
  连接方式。当前支持 `stdio` 和 `streamable_http`。
- `McpServerState`
  运行健康状态。核心状态是 `disabled`、`healthy`、`unhealthy`。
- `McpToolSnapshot`
  当前已暴露的 MCP 工具快照，用于查询与诊断。
- `McpManager`
  MCP 子系统运行时管理器，负责 server 注册、探活、连接、工具发现与调用转发。

## 命名规则

- 远端 MCP 工具不会直接裸露原名，而是会被映射到带命名空间的本地工具名。
- 当前约定形态是 `mcp__<server>__<tool>`。
- 这样做的目的，是避免不同 server 的工具名冲突，同时保留来源信息。

## 运行语义

- 只有健康且启用的 server，才会把工具暴露给 `ToolRegistry`。
- MCP 在这里被视为一种特殊工具集来源，而不是独立于工具系统之外的平行体系。

## 边界

- 本模块不定义业务工具本身的语义，只负责托管和转发。
- demo server 只为验证协议与本地集成链路，不代表正式能力设计。
