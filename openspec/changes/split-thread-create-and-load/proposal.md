## Why

当前 `SessionManager` 中的 `load_or_create_thread`、`load_thread_context`、`lock_thread_context` 都可能在 cache miss 或恢复路径里隐式触发线程初始化，导致“创建线程”“加载线程”“初始化线程”三种语义混在一起。线程稳定前缀、feature/toolset 初始化和恢复链路的副作用入口过多，已经让调用方难以判断何时会真正改写线程状态，也让后续管理 thread 初始化变得越来越难。

另一个缺口是当前初始化只围绕单一 `system_prompt` 展开，没有显式的 thread agent 类型。后续如果主线程、浏览器线程等需要不同的稳定角色 prompt 和默认工具绑定，调用方只能在多处手工拼接，进一步放大了初始化分散的问题。

## What Changes

- **BREAKING** 拆分线程入口语义，移除 `load_or_create_thread` 这种混合职责入口，改为显式区分“创建并初始化线程”和“纯加载线程”。
- 新增一套线程访问生命周期约束，明确 `create_thread` 负责 thread identity 建立、cache/store miss 处理、稳定前缀初始化与首次持久化；`load_thread` 只负责读取已有线程；`lock_thread` 只负责拿可变句柄。
- 为线程初始化引入 `ThreadAgentKind` / `ThreadAgent`，由显式 create 路径声明线程 agent 类型，并由唯一的 `initialize_thread` 入口根据 agent 类型选择预定义 system prompt 与默认工具绑定。预定义 prompt 模板只来自随程序打包的 markdown 文件，不接受 runtime/config 自定义 thread prompt，也不再散落在 Rust 源码中的长字符串里。
- 修改线程运行时契约，禁止 `load_thread_context`、`lock_thread_context` 或其他恢复辅助路径在读取过程中隐式初始化线程。
- 为冷启动恢复、进程重启后恢复、同进程热缓存命中等场景定义统一行为，要求所有“准备对外服务”的线程先走显式 create/init 路径。
- 增加迁移约束与验收任务，收敛 Router、Worker、Command 和测试中对旧混合入口的依赖。

## Capabilities

### New Capabilities
- `thread-access-lifecycle`: 定义 SessionManager 中 create/load/lock 三类线程访问入口的职责边界、cache miss 行为和初始化触发规则。

### Modified Capabilities
- `thread-context-runtime`: 调整线程初始化 ownership，要求稳定 `System` 前缀、thread agent 角色 prompt 与线程级 feature/tool 初始化只能由显式创建/重初始化路径触发，不允许读取路径隐式补初始化。

## Impact

- Affected code: `src/session.rs`、`src/router.rs`、`src/agent/worker.rs`、`src/thread.rs`、`src/session/store/**` 及对应测试。
- API impact: 线程入口会从 `load_or_create_thread` / `load_thread_context` / `lock_thread_context` 迁移为显式 `create_thread` / `load_thread` / `lock_thread`；`initialize_thread` 会新增 thread agent 参数。
- Behavior impact: cache miss 与 store 恢复不再自动代表“已完成初始化”；准备处理消息的线程必须先经过显式 create/init 路径；不同 thread agent 类型会对应不同的稳定 system prompt 和默认工具绑定。
