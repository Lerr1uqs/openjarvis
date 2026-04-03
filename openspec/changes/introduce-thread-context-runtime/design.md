## Context

当前线程级运行时状态已经分散在多个层级中：

- `ConversationThread` 已经持久化了 `loaded_toolsets` 和 `tool_events`
- `ToolRegistry` 内部仍然维护 thread-scoped tool runtime、visible projection 和部分条件化工具显隐
- `CompactRuntimeManager` 单独维护 `compact` / `auto_compact` 的线程级 override
- `browser` 等工具集内部还各自维护 thread-scoped live resource

这导致系统虽然在行为上已经支持“按线程隔离”，但 ownership 不清晰：

- `ToolRegistry` 同时承担“全局工具目录”和“线程事务管理器”两种角色
- 命令无法自然落到统一的线程状态宿主上
- AgentLoop 需要同时拼接 `ConversationThread`、`ToolRegistry` 和 `CompactRuntimeManager` 才能完成一次线程执行
- 后续要加入工具审批、权限策略和更多线程 feature 时，状态会继续向更多组件扩散

这次设计要解决的是宿主问题，而不是一次性删除所有旧 API。目标是先引入 `ThreadContext` 作为统一线程运行时入口，再通过 deprecated 兼容层逐步迁移现有调用点。

## Goals / Non-Goals

**Goals:**

- 引入统一的 `ThreadContext` 作为 thread-scoped runtime 宿主
- 将 `conversation` 与 `thread state` 解耦，明确谁负责历史、谁负责运行时状态
- 让 AgentLoop 以 `ThreadContext` 为中心完成工具可见性计算、工具调用和 feature 状态读取
- 让所有命令都能基于同一个线程上下文读写状态
- 将 `ToolRegistry` 收敛为全局工具池、目录和 handler 解析层
- 为旧 thread runtime API 提供 `#[deprecated]` 兼容层，降低一次性改动面
- 同步更新架构文档，让后续实现、测试和重构围绕同一套分层展开

**Non-Goals:**

- 这次不要求一次性删除所有旧 API
- 这次不要求一次性完成工具审批策略的完整实现
- 这次不改变 MCP server、builtin tool、skill registry 的全局归属
- 这次不尝试把 live runtime object 直接持久化到 thread 存储
- 这次不重新设计 Router 的并发排队语义

## Decisions

### 1. 用 `ThreadContext` 作为统一线程运行时宿主

系统将引入新的 `ThreadContext`，至少包含：

- `locator`: 当前线程定位信息
- `conversation`: `ThreadConversation`
- `state`: `ThreadState`

线程身份规则同时统一为：

- `IncomingMessage.external_thread_id` 只表示上游聊天平台给出的外部线程标识
- `thread_key = user:channel:external_thread_id`
- internal thread id 由 `thread_key` 稳定派生
- 系统不再引入独立的 conversation id，线程身份统一来自 `locator.thread_id`

其中：

- `ThreadConversation` 只负责 turn/message/tool_event 等对话与审计历史
- `ThreadState` 只负责 feature 开关、工具状态、权限审批状态和其他线程级运行时元数据

这样可以把“线程是什么”和“线程现在处于什么运行态”放到同一个宿主里，但仍然保持 conversation 与 state 的职责分离。

Alternative considered:

- 继续扩展 `ConversationThread`，让它同时承担 conversation 和全部 runtime 逻辑。
  Rejected，因为这会把纯数据对象进一步变成行为容器，后续仍然容易把工具调用、审批和 feature 逻辑耦合进去。

### 2. `ThreadConversation` 只保留会话历史与审计职责

现有 `ConversationThread` 将向 `ThreadConversation` 的方向收敛。它负责：

- `turns`
- 持久化后的 `tool_events`
- active history 的 overwrite / compact 写回

它不再直接承担：

- 当前线程的工具可见性计算
- 当前线程是否开启 `auto_compact`
- 当前线程工具权限或审批状态

这部分状态统一放到 `ThreadState` 中。

Alternative considered:

- 让 `ThreadConversation` 直接调用 `ToolRegistry`。
  Rejected，因为这样只是把 thread 事务从 `ToolRegistry` 移到了 conversation 对象里，分层仍然不清楚。

### 3. `ToolRegistry` 收敛为全局工具目录与 handler 解析层

重构后 `ToolRegistry` 只负责全局能力：

- 注册 builtin tools
- 注册 program-defined toolsets
- 管理 MCP server 与 skill registry
- 根据工具名或 toolset 名解析 handler / manifest / catalog entry

它不再作为 thread-scoped state 宿主保存：

- loaded toolsets map
- compact tool projection map
- 其他 thread-scoped visible tool 计算缓存

线程级工具事务改由 `ThreadContext` 驱动：

- `ThreadContext` 维护当前线程哪些 toolset 已加载
- `ThreadContext` 根据当前 feature / permission / budget 计算 visible tools
- `ThreadContext` 再委托 `ToolRegistry` 去解析全局 handler 并执行

Alternative considered:

- 保持 `ToolRegistry::list_for_thread/call_for_thread` 为主入口，只是在内部再包一层 state。
  Rejected，因为 ownership 仍然留在 registry，不符合“registry 是全局池，thread 自己管理自己”的目标。

### 4. `ThreadState` 分层管理 feature、tools 和 approval/policy

`ThreadState` 会拆成几个稳定子域：

- `ThreadFeatureState`
  - `compact_enabled`
  - `auto_compact`
  - 未来其他 thread feature flags
- `ThreadToolState`
  - `loaded_toolsets`
  - 当前线程工具显隐所需的线程级元数据
  - 后续工具权限、allowlist、审批结果
- `ThreadApprovalState` 或等价子域
  - pending approvals
  - granted / denied decisions
  - policy trace

首版不要求把这些全部实现完整，但结构要预留好，避免后续继续把新状态散落回别的模块。

Alternative considered:

- 只把 `loaded_toolsets` 和 `auto_compact` 合并，审批以后再看。
  Rejected，因为这会继续让新线程状态缺少明确归宿，后续补审批时仍然会面临重复重构。

### 5. AgentLoop 改为围绕 `ThreadContext` 运行

`AgentWorker -> AgentLoop` 的线程输入将从“拆散的 thread pieces”收敛为一个 `ThreadContext`。AgentLoop 内部通过它完成：

- 读取会话历史
- 计算当前线程 visible tools
- 调用当前线程工具
- 记录 tool event
- 读取 / 修改 compact 与 auto-compact 的线程状态

也就是说，循环内部的 thread-scoped 行为不再直接找 `ToolRegistry` 或 `CompactRuntimeManager` 拿线程态，而是先操作 `ThreadContext`，再由 `ThreadContext` 委托全局组件。

Alternative considered:

- 只在 AgentLoop 层拼装 helper，不新增上下文对象。
  Rejected，因为这会把复杂度长期留在 loop 中，Command、Session、Router 仍然无法共享同一条线程事务边界。

### 6. 所有 Command 统一通过 `ThreadContext` 读写状态

所有命令都必须先 resolve 目标线程，再读取或修改对应 `ThreadContext`。

`/auto-compact on|off|status` 只是其中一个线程级命令。这样它读写的就是和 AgentLoop 使用同一份线程状态，而不是额外的 override map。

这也为未来 `/approve`、线程级工具权限调整等命令提供统一入口。

Alternative considered:

- 保持命令直接写独立 manager，只在运行时再同步回 thread。
  Rejected，因为会出现双写和状态漂移，命令与循环无法共享同一个事实来源。

### 7. 采用 deprecated-first 迁移，而不是一步删除旧 API

本次重构不会立刻删除现有 thread-scoped API，而是先提供兼容转发层，并用 Rust `#[deprecated(note = \"...\")]` 标记旧入口。例如：

- `ToolRegistry::list_for_thread(...)`
- `ToolRegistry::call_for_thread(...)`
- `ToolRegistry::catalog_prompt(thread_id)`
- `ToolRegistry::rehydrate_thread(...)`
- `ToolRegistry::loaded_toolsets_for_thread(...)`
- `CompactRuntimeManager` 中与线程 override 相关的旧入口

这些 API 在兼容期内可以转发到 `ThreadContext` 新实现，确保：

- 调用点可以逐个迁移
- 测试可以逐步改写
- 风险集中在兼容层，而不是分散在业务行为变化上

Alternative considered:

- 第一版直接删掉旧 API，强制所有调用点同时迁移。
  Rejected，因为这会让重构面过大，难以分批验证，也不利于保持主线稳定。

### 8. live resource 不直接持久化，只通过 `ThreadContext` 统一挂载

浏览器 session 这类 live resource 仍然不会直接落盘到 thread 持久化对象中，但 ownership 会从“工具内部私有 thread map”演进为“由 `ThreadContext` 所在的线程 runtime 挂载和管理”。持久化层只保留可重建的 declarative state。

这意味着：

- 持久化保存“线程需要哪些状态”
- runtime 恢复时基于 `ThreadContext` 和全局 catalog 重新创建 live resource

Alternative considered:

- 直接把 browser session 或其他 live object 持久化到 thread 记录。
  Rejected，因为 live runtime object 不适合直接进入持久化模型，也会抬高恢复复杂度。

## Risks / Trade-offs

- [兼容层会让新旧 API 并存一段时间] → 用 `#[deprecated]` 明确迁移方向，并在 tasks 中规定收敛顺序，避免长期双轨
- [ThreadContext 可能变成新的大而全对象] → 用 `conversation/state/features/tools/approval` 分层，禁止把全局 registry 和大型 service 直接塞成无边界字段
- [Command 路径调整会影响现有命令处理时序] → 明确要求所有命令都先解析线程，再统一走 `ThreadContext`
- [ToolRegistry 去线程化后会牵动较多测试] → 先保留旧 API 转发，测试分批切到新入口
- [live resource ownership 调整容易和现有工具实现冲突] → 首版只调整所有权边界，不要求同时重写所有具体工具内部实现

## Migration Plan

1. 新增 `ThreadContext`、`ThreadConversation`、`ThreadState` 及其子结构，并让 Session 层能加载/保存这组对象。
2. 提供 `ThreadContext` 到现有 `ConversationThread` / tool runtime 数据结构的兼容映射。
3. 为现有 thread-scoped API 增加 `#[deprecated]` 标记，并将其内部转发到 `ThreadContext` 路径。
4. 调整 `AgentWorker` 和 `AgentLoop`，让主循环直接接收和返回 `ThreadContext`。
5. 迁移所有 Command，使其在目标 `ThreadContext` 上读写 feature 和工具状态。
6. 将 `ToolRegistry` 内部的 thread runtime map 与 compact projection map 标记为旧实现，并逐步迁出。
7. 更新架构文档与测试，补足“新入口可用、旧入口 deprecated 仍兼容”的覆盖。
8. 当所有调用点迁移完成后，再删除旧 API 与旧线程状态容器。

Rollback strategy:

- 若迁移中发现行为风险，可保留 `ThreadContext` 数据模型，同时继续使用旧 registry/compact runtime 入口；因为本次先走兼容层，回滚不需要立即删除新增结构。

## Open Questions

- `ThreadContext` 是否需要额外区分“可持久化 state”和“仅运行时 attachment”，还是通过字段命名和模块边界约束即可
- thread-scoped approval 的首版是否只记录决策结果，还是连 pending request 也要进入 `ThreadState`
- `ConversationThread` 是直接重命名为 `ThreadConversation`，还是先保留类型并新增兼容 alias，再分阶段迁移
