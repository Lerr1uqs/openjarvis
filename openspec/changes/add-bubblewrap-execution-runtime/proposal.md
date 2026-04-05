## Why

当前 OpenJarvis 的工具调用链已经统一收口到 `ToolRegistry -> ToolHandler`，但真正发生副作用的部分仍然散落在具体实现里：`read/write/edit` 直接访问宿主文件系统，`bash`、browser sidecar 和 stdio MCP server 直接在宿主环境启动子进程，而 `sandbox` 仍然只是占位对象。继续在具体工具里零散补沙箱，会让边界失控，也无法形成“本地环境 / 沙箱环境”二选一的统一执行模型。

这次变更需要先把“工具执行层”抽象补齐，明确所有工具副作用都应经过统一执行入口，再以 Bubblewrap 作为首个沙箱后端落地，从而为后续真正把工具运行约束在受控环境里提供可实现的方案。

## What Changes

- 在 `agent.tool` 体系下新增统一的工具执行层抽象，执行环境枚举固定为 `Local` 和 `Sandbox` 两种。
- 让工具子系统把文件访问和工具自有子进程启动收口到执行层，而不是在各个 tool handler 中直接操作宿主文件系统或直接 `spawn`。
- 为 `Sandbox` 执行环境定义 Bubblewrap 后端方案，使用长期存活的内部 helper 进程承接沙箱内文件与进程动作。
- 为工具运行时增加执行层配置入口，并明确当配置为 `Sandbox` 但当前环境不支持 Bubblewrap 时的失败语义。
- 将 builtin 文件工具、shell 工具、memory 工具，以及工具自有 sidecar / stdio server 生命周期纳入统一执行层迁移范围。

## Capabilities

### New Capabilities
- `tool-execution-runtime`: 为工具子系统提供本地环境 / 沙箱环境二选一的统一执行层，并用 Bubblewrap 作为首个沙箱后端。

### Modified Capabilities

## Impact

- Affected code: `src/agent/runtime.rs`、`src/agent/tool/**`、`src/agent/sandbox.rs`、`src/agent/worker.rs`、`src/config.rs`，以及新增的内部 helper 入口与对应测试。
- Affected systems: builtin 文件工具、`bash`、memory toolset、browser toolset、stdio MCP server 启动链路。
- Runtime impact: Linux 上可按配置切换到 Bubblewrap 沙箱执行；当配置选择 `Sandbox` 但后端不可用时，运行时将显式失败而不是静默回退。
- Dependency impact: 需要把 `bwrap` 作为宿主环境运行依赖；Rust 侧不要求直接引入容器编排依赖，但会新增 helper 协议与进程管理逻辑。
