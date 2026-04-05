# Tool

## 定位

- `agent/tool` 是 Agent 的能力目录层。
- 它负责定义“有什么工具、怎么暴露、怎么执行”，但线程自己的工具状态仍归 `ThreadContext`。

## 边界

- 负责工具定义、handler、全局注册表、MCP 接入、skill 暴露、线程内工具调用入口。
- 不负责持久化线程级 loaded toolsets、compact 开关、审批状态。

## 关键概念

- `ToolDefinition`
  暴露给模型的工具声明。
- `ToolHandler`
  工具执行接口。
- `ToolCallContext`
  一次调用附带的线程上下文。
- `ToolRegistry`
  全局工具目录。
- `ToolSource`
  工具来源，当前区分 builtin 和 MCP。

## 核心能力

- 注册四个基础工具：`read / write / edit / bash`。
- 注册程序内 toolset、MCP toolset、skill 入口。
- 按 `ThreadContext` 计算当前线程可见工具。
- 在当前线程内执行 `load_toolset / unload_toolset` 和普通工具调用。

## 使用方式

- 全局可见工具放在 `always_visible_handlers`。
- 按需能力放进 toolset，再由线程自己加载。
- 任何线程级可见性判断都应以 `ThreadContext` 为输入，不要在 `ToolRegistry` 自己维护线程真相。

## 继续阅读

- `tool/toolset.md`
- `tool/browser.md`
- `tool/command-session-manual.md`
- `tool/mcp.md`
- `tool/skill.md`
