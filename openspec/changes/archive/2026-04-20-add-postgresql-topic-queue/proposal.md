## Why

当前主链路对普通消息的串行和排队仍然依赖 `ChannelRouter` 里的内存态 `pending_threads/queued_messages` 以及 `AgentWorker` 的进程内 `mpsc` 请求队列。这个模型在单进程内够用，但它没有 durable queue 事实层，进程重启后未处理消息会丢失，也没有正式的 domain worker lease 边界。

这次变更要把“普通消息传递”收敛成一个独立的 PostgreSQL topic queue：只负责消息的事务化传递、拿走和完成，不接管 thread sqlite 的正式事实，不把 LLM/tool/thread state 拉进 queue 事务。

## What Changes

- 新增 PostgreSQL topic queue，用于承接普通入站消息的 durable `add / claim / complete` 流程。
- 规定 queue 的 `topic` 统一采用 `thread_key`，并以 `ThreadLocator + IncomingMessage` 作为入队 payload，而不是持久化 live thread handle。
- 保留 command 前置拦截；`/xxx` 命令继续由 router 直接处理，不进入 queue。
- 规定 router 在普通消息入队前先解析 `ThreadLocator`；若目标 thread 当前进程尚未加载，再执行线程准备动作，避免对已加载 thread 重复走 create 路径。
- 移除 router 侧 `pending_threads/queued_messages` 内存排队机制，并移除 `AgentWorker` 直接消费 `mpsc::Receiver<AgentRequest>` 的模型。
- 将 `AgentWorker` 改为按 `thread_key` 懒创建的 domain worker；worker 由 router 通过 tokio task 启停，并通过独立 worker lease 表协调“同一 domain 同时只有一个活跃 worker”。
- 新增 worker lease 过期与清理机制；当 worker 异常退出或租约过期时，系统会回收对应 lease、记录日志，并恢复 stranded message 的可继续处理状态。
- 明确 PostgreSQL queue 只管理消息传递状态，不管理 thread sqlite 快照、LLM 调用过程、tool 调用过程或跨库联合事务。

## Capabilities

### New Capabilities

- `postgresql-topic-queue`: 普通消息的 PostgreSQL durable queue、按 `thread_key` 的 domain worker 生命周期，以及 queue 与 thread sqlite 的职责边界。

### Modified Capabilities

<!-- None -->

## Impact

- Affected code: `src/router.rs`、`src/agent/worker.rs`、`src/main.rs`、`src/config.rs`，以及新增的 queue 模块和对应测试。
- Affected runtime data: 新增 PostgreSQL queue 库中的 message/worker 事实表；现有 thread sqlite 持久化继续保留。
- Runtime impact: 普通消息改为先解析 locator、按需准备未加载 thread、再入 PostgreSQL queue、再由按 domain 懒创建的 worker 消费；command 继续绕过 queue。
- Delivery semantics impact: 在不做跨库联合事务的前提下，queue 恢复路径采用至少一次交付语义；系统需要通过日志和恢复策略显式接受该边界。
