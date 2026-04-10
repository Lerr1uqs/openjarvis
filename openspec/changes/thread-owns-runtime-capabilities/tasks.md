## 1. Thread 运行时宿主与消息入口

- [x] 1.1 在 `src/thread.rs` 中引入 thread runtime attachment 结构，支持为 `Thread` 挂载 `ToolRegistry`、`MemoryRepository` 和 feature provider 集合
- [x] 1.2 为 `Thread` 增加显式初始化 API，例如 `ensure_initialized()` / `is_initialized()`，替代基于空消息数组的隐式初始化判断
- [x] 1.3 收敛消息写入接口，统一到 `push_message(...)`，并明确稳定前缀、当前 turn working set、request-only message 的内部边界
- [x] 1.4 在 `tests/thread.rs` 与对应 support helper 中补充 UT，覆盖 attach runtime、初始化幂等、统一消息入口和 request-only message 边界

## 2. 初始化、Feature 与 Memory 迁移

- [x] 2.1 将 `AgentWorker` 中的线程初始化逻辑迁移为“attach runtime + 调用 `Thread::ensure_initialized()`”，移除 worker 对初始化消息的直接写入
- [x] 2.2 调整 feature prompt 构造路径，使稳定 system/feature prompt 由 `Thread` 基于 attached feature provider 生成
- [x] 2.3 调整 memory 注入路径，使 request-time memory 由 `Thread` 基于 attached `MemoryRepository` 决定并通过 `push_message(...)` 进入当前 turn
- [x] 2.4 在 `tests/agent/worker.rs`、`tests/agent/feature.rs`、`tests/agent/memory/**` 中补充 UT，覆盖初始化快照保持稳定、memory 不由 Agent 直接注入、恢复后重新 attach runtime 的场景

## 3. 工具投影与工具调用下沉到 Thread

- [x] 3.1 保持 `ToolRegistry` 为全局单例目录，收敛其 thread-scoped owner 逻辑，仅保留 catalog、toolset 注册、handler 解析与全局路由职责
- [x] 3.2 在 `Thread` 上实现基于自身 loaded toolsets / feature / budget state 的 `visible_tools()` 投影入口
- [x] 3.3 在 `Thread` 上实现通过全局 `ToolRegistry` 执行工具调用的入口，并同步更新 thread-owned tool audit 和 toolset state
- [x] 3.4 在 `tests/agent/tool/registry.rs`、`tests/agent/tool/toolset.rs`、`tests/agent/agent_loop.rs` 中补充 UT，覆盖共享全局 registry 下的线程隔离、同一 turn 内 load/unload 生效和 thread-owned tool call 审计

## 4. AgentLoop/Session 收口与回归验证

- [x] 4.1 重构 `AgentLoop`，让主循环只通过 `Thread` 暴露的 `push_message(...)`、`messages()`、`visible_tools()`、`call_tool()`、`finalize_turn(...)` 运行
- [x] 4.2 调整 `SessionManager` / restore 路径，确保线程恢复后先 attach runtime 再进入下一轮请求
- [x] 4.3 为旧的 loop/runtime 入口补充兼容转发或删除不再需要的 thread-scoped helper，避免 `Agent` 再直接管理 tool/memory/feature
- [x] 4.4 运行并补齐 `tests/thread.rs`、`tests/session.rs`、`tests/agent/worker.rs`、`tests/agent/agent_loop.rs`、`tests/agent/tool/**`、`tests/agent/memory/**` 的回归验证
