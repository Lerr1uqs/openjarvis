## Why

当前线程相关能力虽然大多已经是 thread-scoped，但 ownership 仍然分散在 `AgentWorker`、`AgentLoop`、`ToolRegistry` 和 memory/feature 入口之间。继续沿着这条路径叠功能，会让线程初始化、工具可见性、memory 注入和 turn 消息边界继续漂移，因此现在需要把这些 runtime capability 收口到 `Thread` 自己。

## What Changes

- **BREAKING** 将线程初始化 ownership 从 `AgentWorker` 下沉到 `Thread`，由 `Thread` 自己对外暴露显式初始化入口与初始化状态判断。
- **BREAKING** 收敛当前 turn 的消息写入入口，统一改为 `Thread::push_message(...)`，不再保留按消息类型分散命名的外部写入接口。
- 让 `Thread` 直接持有 thread-scoped runtime attachment，并通过这些 attachment 管理 feature prompt 构造、memory 注入、工具可见性投影和工具调用。
- 保持 `ToolRegistry` 为全局单例工具目录与 handler 解析层；每个 thread 只持有自己的 loaded/unloaded tool state、tool audit 和运行时投影结果。
- 让 `AgentLoop` 只作为执行框架，围绕 `Thread` 提供的 `push_message(...)`、`messages()`、`visible_tools()`、`call_tool()` 等接口运行，而不再直接管理 thread-scoped tool/memory/feature 状态。

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `thread-context-runtime`: 线程初始化、request 组装、memory 注入和 turn working set 的 ownership 改为由 `Thread` 自己统一管理。
- `thread-managed-toolsets`: 工具集加载状态继续按 thread 隔离，但可见工具投影与工具调用入口改为由 `Thread` 驱动，并通过全局 `ToolRegistry` 解析与执行。

## Impact

- Affected code: `src/thread.rs`、`src/session.rs`、`src/agent/worker.rs`、`src/agent/agent_loop.rs`、`src/agent/feature/mod.rs`、`src/agent/tool/mod.rs`、`src/agent/memory/**` 及对应测试。
- API impact: `AgentLoop` 不再直接持有 thread-scoped tool/memory/feature 管理职责；`Thread` 将新增统一消息写入、初始化和工具调用接口。
- Runtime impact: `ToolRegistry` 继续作为全局 registry 存活，但 thread-scoped loaded toolsets、memory request state 和 feature 初始化快照改由 `Thread` own 住。
