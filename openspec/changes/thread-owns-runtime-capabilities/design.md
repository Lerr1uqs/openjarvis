## Context

当前实现虽然已经把一部分 thread-scoped state 放进了 `Thread`，但 ownership 仍然没有真正收口：

- `AgentWorker` 仍然负责 `initialize_thread()`，由外部判断线程何时初始化、如何写入稳定前缀。
- `AgentLoop` 仍然直接接触 runtime tool/memory/feature 入口，自己拼 request messages、自己拿 visible tools、自己调用工具。
- `ToolRegistry` 既是全局工具目录，又隐含承担了部分线程运行时行为的入口。
- memory repository、feature provider 这些本质上按 thread 生效的能力，还没有成为 `Thread` 自己的运行时依赖。

这会导致一个根问题：线程相关能力虽然按 thread 隔离，但并不是由 `Thread` 自己拥有。只要后续继续增加 tool policy、memory catalog、approval 或 feature prompt，职责就还会继续散在 worker/loop/runtime 之间。

## Goals / Non-Goals

**Goals:**

- 让 `Thread` 成为 thread-scoped runtime capability 的唯一宿主。
- 让线程初始化由 `Thread` 自己负责，外部模块只负责 attach runtime 和触发初始化。
- 收敛消息写入入口，统一使用 `push_message(...)`。
- 保持 `ToolRegistry` 为全局单例工具目录和 handler 解析层，而不是按 thread 复制 registry。
- 让 per-thread loaded toolsets、可见工具投影、tool audit、memory catalog 接线和 feature prompt 构造都通过 `Thread` 驱动。
- 让 `AgentLoop` 只保留执行框架职责，不再直接管理 thread-scoped tool/memory/feature。

**Non-Goals:**

- 这次不把 `ToolRegistry`、`MemoryRepository` 或 feature provider 直接持久化到 thread store。
- 这次不重做 tool 名称路由、toolset catalog 文案或 memory 工具协议。
- 这次不改 channel/router 的外部交互协议，只调整内部 ownership。
- 这次不要求一次性重写所有具体工具的内部 live resource 管理。

## Decisions

### 1. `Thread` 拆分为持久化状态和运行时 attachment，但 ownership 统一归 `Thread`

`Thread` 将继续作为聚合根存在，但内部边界拆成两层：

- 持久化层：thread locator、稳定 `System` 前缀、conversation history、feature/tool/audit state
- 运行时层：当前 turn working set、全局 runtime service attachment

运行时 attachment 首版至少包含：

- 全局 `ToolRegistry`
- `MemoryRepository`
- feature 初始化辅助逻辑

这些对象由外部注入到 `Thread`，但一旦 attach 完成，后续如何使用它们由 `Thread` 自己决定。`AgentLoop` 和 `AgentWorker` 不再直接用这些 service 处理 thread-scoped 逻辑。

Alternative considered:

- 直接把 `Arc<ToolRegistry>`、`MemoryRepository` 等持久化进 `Thread`
  Rejected，因为这些对象属于运行时基础设施，不适合进入 store snapshot，也会让线程恢复和测试边界变脏。

### 2. 线程初始化下沉为 `Thread` 自己的职责

初始化边界改为：

1. Session/Worker 取出 `Thread`
2. 外部为该 thread attach runtime services
3. 外部调用 `thread.ensure_initialized()`
4. `Thread` 自己判断是否已经初始化，并在需要时构造稳定 `System` 前缀

初始化所需的 system prompt、feature prompt 和 memory catalog prompt 不再由 worker 直接写消息数组，而是由 `Thread` 基于已 attach 的 feature provider / memory repository 生成并落入自身状态。

这会替代当前基于“`messages().is_empty()`”的隐式初始化判断，改成显式、幂等的 thread-owned 初始化流程。

Alternative considered:

- 保留 `AgentWorker::initialize_thread()` 作为唯一初始化入口
  Rejected，因为 worker 是 orchestration 层，不应该拥有“线程何时算初始化完成”的领域语义。

### 3. 所有进入线程请求视图的消息统一经由 `push_message(...)`

消息入口统一为 `push_message(...)`。任何进入 thread 请求视图的消息，包括：

- 当前轮 user message
- assistant text
- tool call message
- tool result message
- 初始化阶段物化出的稳定 system message

都应收敛到同一个 message mutation 入口。由 `Thread` 根据当前生命周期和 message scope 决定该消息进入：

- 稳定 `System` 前缀
- 当前 turn working set
- 或最终持久化 history

这样可以避免 `inject_user_message(...)`、`append_message(...)`、`inject_memory(...)` 这类分散接口重新把 ownership 拆回外部。

Alternative considered:

- 保留按消息类型命名的多个写入接口，只把底层实现共用
  Rejected，因为调用点仍然会按消息来源分流，边界约束还是弱的。

### 4. `ToolRegistry` 保持全局单例，thread 只拥有自己的 tool state 与投影

`ToolRegistry` 的定位保持为全局 registry：

- 注册 builtin tools
- 注册 program-defined toolsets
- 管理 routed tool name 与 handler 解析
- 提供全局 catalog / manifest / handler lookup

但与 thread 有关的内容必须收口到 `Thread`：

- 当前 thread 已加载哪些 toolset
- 当前 thread 当前时刻哪些工具可见
- 当前 thread 的工具审计记录
- 当前 thread 发起工具调用时的调用上下文

执行路径改为：

1. `AgentLoop` 调用 `thread.visible_tools()`
2. `Thread` 读取自己的 loaded toolsets / feature / budget state
3. `Thread` 借助全局 `ToolRegistry` 计算 visible tools
4. `AgentLoop` 调用 `thread.call_tool(...)`
5. `Thread` 借助全局 `ToolRegistry` 解析并执行 handler，同时更新 thread-owned tool state

Alternative considered:

- 为每个 thread 构造一个独立 `ToolRegistry`
  Rejected，因为 registry 的职责是全局能力目录，不应按 thread 复制。
- 保持 `AgentLoop` 直接调用 `ToolRegistry::list_tools/call_tool`
  Rejected，因为 thread-scoped tool state 会继续留在 loop 之外。

### 5. memory repository 和 feature provider 也变成 `Thread` 的运行时依赖

memory 与 feature prompt 虽然依赖全局基础设施，但行为上是 thread-scoped：

- 某个线程初始化时看到什么 feature/system prompt，属于该线程自己的初始化快照
- 某个线程初始化时看到什么 active memory catalog，也属于该线程自己的稳定 snapshot

因此：

- `Thread` 自己调用 attached feature 初始化辅助逻辑生成初始化前缀
- `Thread` 自己调用 attached `MemoryRepository` 构造初始化阶段的 active memory catalog prompt
- `AgentLoop` 不再直接查 memory repository，也不直接重建 feature prompt，更不会自动注入 memory 正文

Alternative considered:

- 继续让 worker 负责 feature prompt，loop 负责 memory 相关接线
  Rejected，因为这是按执行阶段拆职责，不是按线程 ownership 拆职责。

### 6. `AgentLoop` 收缩为纯执行框架

重构后 `AgentLoop` 只围绕 `Thread` 暴露的接口执行：

1. `thread.ensure_initialized()`
2. `thread.begin_turn(...)`
3. `thread.push_message(...)`
4. `thread.messages()`
5. `thread.visible_tools()`
6. `thread.call_tool(...)`
7. `thread.finalize_turn(...)`

也就是说，loop 负责时序，不负责 thread-scoped runtime 管理。

Alternative considered:

- 在 loop 内保留 runtime helper，并只做轻量封装
  Rejected，因为 helper 的 owner 仍然是 loop，本质问题没有解决。

### 7. Session/恢复链路只持久化 declarative thread state

Session/store 仍然只保存可持久化的 thread snapshot：

- 稳定 `System` 前缀
- history
- feature/tool/audit state

运行时 attachment 必须在 load/restore 后重新 attach，不能直接从 store 反序列化出来。

这保证：

- store 结构保持干净
- thread 恢复后 runtime 依赖可替换
- 测试可为 thread 挂不同假的 registry/repository/provider

Alternative considered:

- 在 session cache 里偷偷保留带 attachment 的 thread 并默认长期复用
  Rejected，因为跨进程恢复和 cache miss 路径仍然需要显式 attachment 语义。

## Risks / Trade-offs

- [Risk] `Thread` 容易变成新的大对象 → Mitigation: 明确把持久化 state、current turn working set 和 runtime attachment 分层，禁止把 orchestration 逻辑塞回 `Thread`
- [Risk] 旧的 request-time memory 注入残留路径会和新的渐进式披露模型冲突 → Mitigation: 显式删除 runtime memory recall 入口，只保留初始化 catalog 和 memory tool 渐进式读取
- [Risk] ToolRegistry 去线程 owner 化后会牵动较多调用点和测试 → Mitigation: 先保留兼容转发层，分批迁移到 `Thread` API
- [Risk] memory/feature 下沉到 `Thread` 后，初始化和请求期流程会更依赖 runtime attachment 完整性 → Mitigation: 为未 attach 状态提供显式错误，而不是静默降级

## Migration Plan

1. 在 `Thread` 上引入 runtime attachment 结构，并提供 attach/ensure_initialized 基础 API。
2. 将 worker 中的初始化逻辑迁移为 `Thread::ensure_initialized()` 调用，移除 worker 对稳定前缀的直接写入。
3. 收敛消息写入路径，统一迁移到 `push_message(...)`。
4. 将可见工具投影与工具调用入口从 `AgentLoop`/runtime 下沉到 `Thread`，保留 `ToolRegistry` 全局目录职责。
5. 将 memory catalog 构造和 feature prompt 构造从 worker/loop 迁移到 `Thread` runtime attachment，并删除 request-time memory 注入残留路径。
6. 让 `AgentLoop` 只通过 `Thread` API 驱动一次 turn。
7. 更新 session restore、thread、worker、agent loop、tool registry 和 memory 相关测试。

Rollback strategy:

- 若迁移过程中出现行为风险，可保留 `Thread` 新接口，同时让旧 `AgentLoop`/worker/runtime 路径转发到兼容层，不需要回滚持久化模型。

## Open Questions

- `push_message(...)` 是否需要显式 scope 参数，还是由当前生命周期和 message role 推导即可
- `Thread` 未 attach runtime 时，`messages()` 是否允许只返回持久化视图，还是一律对需要 runtime 的入口报错
