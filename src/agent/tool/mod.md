# agent/tool 模块总览

## 作用

`agent/tool/` 是 Agent 的能力层，负责把“可供模型调用的动作”组织成统一工具体系。这里统一定义工具协议、工具注册、线程级可见性，以及动态工具集加载。

## 子模块

- `browser/`
  浏览器工具集。负责通过 Node Playwright sidecar 提供页面浏览与交互能力。
- `edit.rs`
  内建 `edit` 工具。负责对文件做精确文本替换。
- `mcp/`
  MCP 运行时。负责托管外部 MCP server，并把远端工具映射进本地工具注册表。
- `read.rs`
  内建 `read` 工具。负责读取 UTF-8 文件及可选行区间。
- `shell.rs`
  内建 `bash` 工具。负责执行一次性 shell 命令。
- `skill/`
  本地 skill 子系统。负责发现、加载、展开技能文档。
- `toolset.rs`
  线程级工具集状态层。负责记录某个 thread 当前加载了哪些工具集。
- `write.rs`
  内建 `write` 工具。负责整文件覆盖写入。

## 核心概念

- `ToolDefinition`
  工具对外暴露给模型的声明，包含名字、描述、参数 schema、来源。
- `ToolHandler`
  工具执行器接口。定义“这个工具收到参数后该怎么执行”。
- `ToolRegistry`
  工具总注册表。负责汇总内建工具、工具集、MCP 工具、skill 能力。
- `Toolset`
  一组成套出现的工具。它比单个工具更高一层，适合做渐进式加载。
- `ToolCallContext`
  一次工具调用附带的线程上下文，主要用于线程级资源隔离。
- `Always Visible Tool`
  始终可见的基础工具，不需要额外加载即可给模型使用。
- `Thread-scoped Toolset`
  只对当前 thread 生效的工具集，可按需加载、按需卸载。
- `compact request visibility`
  `compact` 是否对模型可见由当前 request state 决定，不再持久化在线程状态里。
  当 `auto_compact` 开启时，当前 request 会暴露 `compact`，并在每次 generate 时配合预算提示一起提供给模型。

## 工具分层

- 第一层是内建基础工具，如 `read`、`write`、`edit`、`bash`。
- 第二层是程序内定义的工具集，如 `browser`。
- 第三层是外部 MCP server 提供的远端工具。
- 第四层是本地 skill，用于按需加载额外知识或操作说明。

## 设计意图

- 不是把所有工具一次性都暴露给模型，而是尽量按线程、按任务、按需可见。
- 重点目标是控制上下文膨胀，而不是单纯追求“工具越多越强”。
