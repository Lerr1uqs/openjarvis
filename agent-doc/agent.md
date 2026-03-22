# Agent 目录说明

## 当前目录结构

源码里的 agent 相关代码现在放在 [src/agent](F:/coding-workspace/openjarvis/src/agent) 目录下：

- [mod.rs](F:/coding-workspace/openjarvis/src/agent/mod.rs)
  - 统一导出 agent 相关模块和公共类型
- [worker.rs](F:/coding-workspace/openjarvis/src/agent/worker.rs)
  - `AgentWorker`
  - 对外承接 router 的消息处理入口
- [agent_loop.rs](F:/coding-workspace/openjarvis/src/agent/agent_loop.rs)
  - `AgentLoop`
  - 负责一轮 agent 执行
- [runtime.rs](F:/coding-workspace/openjarvis/src/agent/runtime.rs)
  - `AgentRuntime`
  - 聚合 `hook / tool / mcp` 三类 registry
- [hook.rs](F:/coding-workspace/openjarvis/src/agent/hook.rs)
  - hook 事件枚举
  - hook handler trait
  - hook registry
- [tool/mod.rs](F:/coding-workspace/openjarvis/src/agent/tool/mod.rs)
  - tool 定义
  - tool handler trait
  - tool registry
  - 内置工具批量注册入口
- [tool/read.rs](F:/coding-workspace/openjarvis/src/agent/tool/read.rs)
  - 读取 UTF-8 文本文件
- [tool/write.rs](F:/coding-workspace/openjarvis/src/agent/tool/write.rs)
  - 全量写入 UTF-8 文本文件
- [tool/edit.rs](F:/coding-workspace/openjarvis/src/agent/tool/edit.rs)
  - 按字符串匹配修改文件内容
- [tool/shell.rs](F:/coding-workspace/openjarvis/src/agent/tool/shell.rs)
  - 执行本地 shell 命令
- [mcp.rs](F:/coding-workspace/openjarvis/src/agent/mcp.rs)
  - MCP 服务定义
  - MCP registry

## 当前调用关系

当前消息链路在 agent 内部的调用关系是：

`AgentWorker::handle_message`

`-> SessionManager::begin_turn`

`-> AgentLoop::run`

`-> MessageContext::render_for_llm`

`-> HookRegistry::emit(UserPromptSubmit)`

`-> ToolRegistry::register_builtin_tools`

`-> LlmProvider::generate`

`-> 可选的 ToolRegistry::call`

`-> HookRegistry::emit(Notification)`

`-> SessionManager::complete_turn`

## 当前各模块职责

### AgentWorker

`AgentWorker` 是 router 唯一直接依赖的 agent 入口。

它负责：

- 接收 `IncomingMessage`
- 维护 `Session / Thread / Context`
- 调用 `AgentLoop`
- 生成统一 `OutgoingMessage`

也就是说，外层不需要知道：

- 有没有 hook
- 有没有 tool
- 有没有 mcp
- loop 内部到底怎么跑

这些都被收在 agent 目录内部。

### AgentLoop

`AgentLoop` 是后续扩展 agent 行为的主位置。

当前最小实现只做了：

1. 发 `UserPromptSubmit` hook
2. 要求模型输出 `TOOL_CALL` 或 `FINAL`
3. 最多执行一次工具调用
4. 把工具结果回填给模型
5. 发 `Notification` hook

后续往这里继续接：

- tool planning
- tool call
- tool result 回填
- mcp client 调度
- compact / stop / permission request

### AgentRuntime

`AgentRuntime` 是一个聚合容器，当前持有：

- `HookRegistry`
- `ToolRegistry`
- `McpRegistry`

这样做的目的很直接：

- `AgentWorker` 不需要自己管理一堆 registry 字段
- `AgentLoop` 可以统一访问运行时扩展点
- 后续接配置或热更新时有一个稳定挂载点

### HookRegistry

当前 `HookRegistry` 只实现了：

- 注册 handler
- 顺序 emit 事件

还没有做：

- Python/TypeScript hook loader
- 动态脚本加载
- hook 超时/隔离
- hook 失败策略配置

### ToolRegistry

当前 `ToolRegistry` 只实现了：

- 注册工具
- 列出工具定义
- 按名称调用工具
- 批量注册四个内置基础工具

当前内置工具是：

- `read`
- `write`
- `edit`
- `shell`

还没有做：

- 权限审批
- 工具 schema 暴露给模型
- 工具调用前后 hook
- 沙箱执行

### McpRegistry

当前 `McpRegistry` 只实现了：

- 注册 MCP 服务定义
- 获取和列出 MCP 服务

还没有做：

- 真正的 MCP client
- stdio/http/sse 连接管理
- tool discovery
- 认证和重连

## 为什么现在先拆目录

在单文件 `agent.rs` 阶段，继续往里面堆：

- mcp
- tool
- hook
- loop

很快会变成一个混杂入口文件。

先拆成目录的收益是：

- worker 和 loop 分层清楚
- registry 型能力有独立文件
- 后续增加真正实现时，不需要再做一次大重构
- 测试也能按目录映射拆开

## 当前状态结论

现在已经具备“开始接 agent 能力”的源码骨架了：

- 可以继续往 `hook.rs` 加脚本 hook
- 可以继续往 `tool.rs` 加真实工具注册与调用
- 可以继续往 `mcp.rs` 接 MCP client
- 可以继续往 `agent_loop.rs` 实现多轮 tool loop

但当前运行时行为仍然保持最小闭环：

- 已支持单轮 tool call
- 不连 MCP
- 只发 hook
- 当前只支持单轮 ReAct，不支持多轮规划
- loop 事件会通过 router 回发到当前群聊
