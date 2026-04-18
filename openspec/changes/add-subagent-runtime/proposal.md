## Why

当前 OpenJarvis 已经具备稳定的线程模型、线程级 toolset 装载能力，以及可复用的 `AgentLoop` 执行框架，但系统仍然只支持单一主线程 agent 直接完成任务。对于浏览器、检索等更偏专用职责的能力，继续把所有规划、上下文和工具执行都堆在主线程里，会让 thread prompt、工具可见性和上下文管理持续膨胀，也缺少“主线程调用专用子代理并回收结果”这一正式运行模型。

当前代码也已经具备实现 subagent 的几个前提：线程创建与初始化被显式收口到 `create_thread(...) + ThreadAgentKind`，线程自己的 prompt/toolset 真相由 `ThreadAgent` 驱动，`AgentLoop` 可以在持有 live `Thread` 的前提下完成一次完整执行。但还没有一套正式设计去回答这些关键问题：subagent 的线程身份如何表达、如何避免和主 worker 队列自锁、`yolo/persist` 生命周期如何管理、以及子线程事件如何与主 channel 发送面解耦。

因此需要新增一轮 subagent runtime 设计，把“复用现有 thread/agent loop 模型实现多代理协作”正式收口，而不是继续通过临时工具逻辑或特殊分支拼接。

## What Changes

- 新增 subagent runtime 模型，允许主线程通过显式工具调用创建、发送、关闭和列出当前线程下的 subagent。
- 新增 child-thread identity 模型，用独立字段表达 subagent 子线程身份，而不是继续复用 channel 侧 `external_thread_id` 语义。
- 新增 subagent 线程管理约束：同一父线程下，同一种 subagent profile 同时只允许存在一个实例；例如同一父线程只允许一个 `browser` subagent。
- 复用现有 `ThreadAgentKind` / `ThreadAgent` 模型承载 subagent profile，不再额外引入与之重叠的第二套 agent kind 概念。
- 新增 `SubagentRunner` 及其独立 worker 池，使主线程调用 subagent 时不会复用主 worker 的同一请求队列同步等待，从而避免自锁。
- 新增 `spawn_subagent`、`send_subagent`、`close_subagent`、`list_subagent` 四个工具，并约束首版只支持同步阻塞结果返回，不支持后台异步任务或流式子结果转发。
- 为 subagent 定义两种生命周期：
  - `persist`: 子线程上下文持久化保存，可多次 `send_subagent`
  - `yolo`: 子线程同样复用持久化边界执行，但在本次 `send_subagent` 成功返回后立即做 best-effort `remove`
- 为 `AgentLoop` 增加 subagent 内部事件兼容层，允许系统通过 `AgentEventSender::for_subagent_thread` 构造只记录日志、不向 channel 发送的子线程事件接收端。
- 补充主线程与子线程之间的职责边界：主线程只通过工具结果拿到 subagent 聚合输出；subagent 的 committed 事件默认只用于调试记录，不直接面向外部 channel 分发。

## Capabilities

### New Capabilities

- `subagent-runtime`: 定义主线程创建、调用、关闭、列出 subagent 的正式运行模型，以及 subagent 的独立 worker 执行与生命周期管理。
- `subagent-thread-identity`: 定义父线程下 child thread 的身份字段、唯一性约束，以及与现有 channel `external_thread_id` 的解耦边界。

### Modified Capabilities

- `thread-context-runtime`: 扩展线程运行时模型，使线程除了主线程角色外，也能承载 subagent profile，并允许内部子线程走无 channel 发送的事件记录路径。
- `thread-access-lifecycle`: 扩展显式 create/load/lock 访问模型，允许系统在父线程上下文中显式创建和恢复 child thread。
- `thread-managed-toolsets`: 允许不同 `ThreadAgentKind` 继续通过既有线程初始化机制选择各自稳定 prompt 与默认工具绑定，并让 subagent 线程按自己的 thread-scoped toolset truth 参与可见性投影。

## Impact

- Affected code: `src/thread.rs`、`src/thread/agent.rs`、`src/session.rs`、`src/agent/worker.rs`、`src/agent/agent_loop.rs`、`src/agent/tool/**`、`src/router.rs` 及对应测试。
- API impact: 会新增 subagent 工具接口、child-thread identity 字段，以及 `AgentEventSender::for_subagent_thread` 这类内部事件入口；现有 `ThreadAgentKind` 会扩展到更多 profile，而不是只保留 `Main/Browser`。
- Runtime impact: 系统将引入独立 subagent worker 池；主线程调用 subagent 不再复用当前主 worker 请求队列同步等待。
- Persistence impact: subagent 继续复用现有 thread-owned 持久化边界；`persist` 子线程会长期保留，`yolo` 子线程会在调用成功返回后做 best-effort remove。
- Behavior impact: subagent 首版只支持同步阻塞调用，不提供异步后台任务、流式用户可见子结果或一个 profile 多实例并存。
