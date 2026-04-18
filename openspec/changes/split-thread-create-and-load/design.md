## Context

当前 `SessionManager` 的线程入口通过同一个 `ensure_thread_handle(...)` 内部流程同时承担了四种职责：cache 命中复用、store 恢复、空线程创建、线程初始化。结果是：

- `load_or_create_thread(...)`、`load_thread_context(...)`、`lock_thread_context(...)` 三个公开入口都可能隐式初始化线程。
- 读取路径和写入路径都可能追加稳定 `System` 前缀、更新 lifecycle state、回写 store。
- 调用方无法从接口名判断“这次调用到底只是读，还是会改线程正式状态”。
- thread 初始化入口散落后，后续 feature/toolset/memory 初始化变更都必须同时检查多条链路。
- 当前初始化只接受单一 `system_prompt`，没有显式 thread agent 类型；一旦主线程、浏览器线程等需要不同角色 prompt 和默认工具绑定，初始化规则会进一步分散到不同调用点。

这与“线程初始化必须可管理、可预期、可收口”的目标相反，也是当前 thread 初始化维护成本高的根因。

## Goals / Non-Goals

**Goals:**

- 让线程创建/初始化与线程加载/加锁拥有明确且互不重叠的语义。
- 让“准备一个可直接服务的线程”只有一个正式入口，避免读路径产生持久化副作用。
- 让线程初始化唯一收口到 `initialize_thread(thread, thread_agent_kind)`，并允许不同 thread agent 类型选择不同预定义 system prompt 和默认工具绑定。
- 让 cache miss、store 恢复、进程重启后首次访问等冷启动场景遵循统一规则。
- 保留现有 `ThreadRuntime::initialize_thread(...)` 的初始化内容，但收紧其触发时机。
- 为 Router、Worker、Command 和测试提供一致的调用约束，降低 thread 初始化管理成本。

**Non-Goals:**

- 这次不重做 `Thread` 的持久化结构，不调整 system prefix、message history、feature/tool state 的数据模型。
- 这次不改 feature prompt、memory catalog、toolset catalog 的生成内容，只调整它们的触发入口。
- 这次不修改外部 channel/router 协议，不引入新的线程 identity 规则。
- 这次不自动修改 `model/*.md` 组件文档；若后续实现与现有组件文档冲突，需要单独和用户对齐。

## Decisions

### 1. 对外入口拆分为 `create_thread`、`load_thread`、`lock_thread`

`SessionManager` 的线程访问接口改为三类：

- `create_thread(...)`：显式准备一个可直接服务的线程。它负责解析 thread identity、命中 cache 时复用句柄、cache miss 时从 store 恢复或创建空线程，并在返回前完成初始化。
- `load_thread(...)`：纯读取已有线程快照，只负责缓存命中或从 store 恢复，不触发创建和初始化。
- `lock_thread(...)`：纯获取已有线程的可变句柄，只负责缓存命中或从 store 恢复，不触发创建和初始化。

这样调用方可以从接口名直接判断是否可能产生持久化副作用。

Alternative considered:

- 保留 `load_or_create_thread(...)`，仅通过注释声明其副作用
  Rejected，因为问题不是注释不足，而是创建/加载/初始化三种职责已经物理耦合，继续保留混合接口只会让语义继续模糊。

### 2. `create_thread(...)` 采用幂等的“创建或恢复并完成初始化”语义

虽然命名叫 `create_thread(...)`，但它对调用方的真实承诺是“拿到一个已经可以对外服务的线程”。因此它必须是幂等的：

- 如果线程已经在 cache 中，直接复用并确认初始化状态。
- 如果线程只存在于 store 中，先恢复，再确认初始化状态。
- 如果线程不存在，创建空线程并初始化。

这满足用户所说的“多用一个 if 判断有没有加载到内存”，但把这个判断限制在显式 create 路径内部，不再让所有读路径都背负这套逻辑。

Alternative considered:

- 新增 `prepare_thread(...)` 或 `ensure_thread(...)`，避免 `create_thread(...)` 名称看起来像“只能创建新线程”
  Rejected，本次方案优先采用用户认可的 `create_thread + load_thread` 分工。若实现阶段觉得命名仍有歧义，再单独讨论命名调整。

### 3. `initialize_thread(...)` 必须是唯一线程初始化入口，并显式接收 `ThreadAgentKind`

线程初始化的稳定角色 prompt、预绑定工具和 feature 物化必须都收口到一个入口：

- `ThreadRuntime::initialize_thread(thread, thread_agent_kind)`

其中 `thread_agent_kind` 是闭集枚举，例如：

- `Main`
- `Browser`

初始化实现会基于这个枚举构造一个持久化的 `ThreadAgent` 真相，至少包含：

- `kind`
- `bound_toolsets`

然后统一完成三件事：

- 选择该 agent 类型对应的预定义 system prompt
- 合并该 agent 类型对应的默认工具绑定
- 继续执行已有的环境感知与 feature 初始化逻辑

这些预定义 prompt 模板必须来自随程序打包的 markdown 文件，并通过编译期资源加载进入二进制，而不是直接散落成 Rust 长字符串常量。这样 prompt 维护可以和代码逻辑解耦，同时保持单文件可审计。

同时系统不再接受 runtime/config 传入自定义 thread system prompt。`ThreadRuntime`、worker builder 和配置层都只能选择预定义 `ThreadAgentKind`，不能绕过模板文件直接改写线程稳定角色前缀。

这样主线程、浏览器线程等差异不会再散落到 Router、Worker 或测试调用点。

对已初始化线程，持久化的 `ThreadAgent` 是真相；如果调用方再次以不同的 `thread_agent_kind` 访问同一线程，系统只允许记录告警并继续使用该线程已持久化的 agent 类型，而不是偷偷重写稳定前缀。

Alternative considered:

- 继续只传一个 `system_prompt` 字符串，让不同调用方各自拼装 browser/main prompt 和工具绑定  
  Rejected，因为这会把初始化差异继续散落到调用方，违背“初始化唯一入口”的目标。

### 4. 内部 helper 要拆成“纯加载”和“带初始化准备”两层

当前 `ensure_thread_handle(...)` 会在读路径中隐式完成全部工作。重构后内部流程拆成两层：

- `load_existing_thread_handle(...)`：负责 cache/store 命中与恢复，miss 时返回不存在，不做空线程创建，不做初始化。
- `create_or_restore_thread_handle(...)`：在 `create_thread(...)` 路径中复用 `load_existing_thread_handle(...)`；若 miss 则创建空线程，再统一执行初始化。

这样读取链路和初始化链路共享底层恢复逻辑，但不会共享副作用。

Alternative considered:

- 保留单一 `ensure_thread_handle(...)`，通过布尔参数控制是否初始化
  Rejected，因为一个 helper 同时负责“存在性保证”和“读取恢复”仍然容易在后续调用点被误用，布尔开关也会继续弱化语义。

### 5. 线程初始化 ownership 收口到显式 create/reinitialize 路径

`ThreadRuntime::initialize_thread(thread, thread_agent_kind)` 可以继续保留为具体初始化实现，但只能由两类路径触发：

- `create_thread(...)`
- 显式的重初始化路径，例如 reset 之后的 reinitialize

`load_thread(...)`、`lock_thread(...)`、调试读取路径和普通恢复辅助路径不得再调用它。这样“何时允许写稳定前缀、何时允许更新 initialized state”会有唯一入口。

Alternative considered:

- 继续允许 `load_thread(...)` / `lock_thread(...)` 在发现未初始化线程时自动补初始化
  Rejected，因为这会把“读取一个线程”重新变成“也许会改写一个线程”，直接违背本次重构目标。

### 6. 缺失线程在 load/lock 路径中必须显式暴露，不允许偷偷创建

`load_thread(...)` 在 miss 时返回“线程不存在”；`lock_thread(...)` 在 miss 时也必须显式暴露缺失，而不是像现在一样偷偷创建一个空线程并继续流程。这样调用方如果遗漏了 `create_thread(...)`，问题会在接口边界立即暴露，而不是进入更深层后才表现为初始化混乱。

Alternative considered:

- `lock_thread(...)` 继续在 miss 时创建空线程，减少调用方处理分支
  Rejected，因为这相当于保留了一个隐藏 create 入口，最终还是会回到现在的语义混乱。

### 7. 迁移策略采用“新接口先落地，旧接口短期兼容，随后移除”

为了降低回归风险，迁移建议分阶段执行：

1. 新增 `create_thread(...)`、`load_thread(...)`、`lock_thread(...)` 和对应内部 helper。
2. 给 `initialize_thread(...)` 增加 `ThreadAgentKind`，并落地 `ThreadAgent` 的持久化真相。
3. Router、Worker、Command 与测试先迁到新接口。
4. 保留旧 `load_or_create_thread(...)` 作为短期兼容 wrapper，并在内部转发到新 create 路径。
5. 待调用点迁完后删除旧 wrapper 与隐式初始化 helper。

这样可以先用编译错误和测试把边界固定住，再安全清理旧入口。

Alternative considered:

- 一次性删除旧入口并同步改完所有调用点
  Rejected，改动面涉及 session/router/worker/tests，多阶段迁移更稳妥。

## Risks / Trade-offs

- [Risk] 现有调用方依赖 `load_thread_context(...)` 或 `lock_thread_context(...)` 的隐式初始化行为，重构后可能直接暴露 not-found 或未初始化状态。  
  Mitigation: 在迁移阶段保留兼容 wrapper，并增加针对冷启动恢复、store miss 和重复 create 的回归测试。

- [Risk] 历史上已经持久化的“未初始化线程快照”在纯 load 路径下会原样返回，短期内看起来比现在更“严格”。  
  Mitigation: 明确规定所有准备处理消息的链路都必须先走 `create_thread(...)`；对历史脏数据增加 create-path 修复测试。

- [Risk] API 命名从 `load_or_create_thread(...)` 切到 `create_thread(...)` 后，部分人会误以为它只适用于全新线程。  
  Mitigation: 在设计和代码文档中明确其“幂等准备”语义，并通过测试覆盖“已有线程再次 create 仍能正常返回”的场景。

- [Risk] 同一个已初始化线程如果被不同调用方传入不同 `ThreadAgentKind`，可能造成角色语义冲突。  
  Mitigation: 以持久化 `ThreadAgent` 为真相；对不一致访问增加关键日志，并禁止读路径或重复 create 隐式改写稳定前缀。

- [Risk] 多阶段迁移期间新旧接口并存，可能让边界再次模糊。  
  Mitigation: 旧接口只允许转发到新 create 路径，不允许保留旧的隐式初始化实现；完成迁移后立即删除。

## Migration Plan

1. 在 `SessionManager` 中新增显式 `create_thread(...)`、`load_thread(...)`、`lock_thread(...)` 接口，并拆分内部 helper。
2. 在 `ThreadRuntime` 中新增 `ThreadAgentKind` / `ThreadAgent` 初始化链路，并让 `initialize_thread(...)` 成为唯一初始化入口。
3. 将 Router 入站消息主链路改为先 `create_thread(..., ThreadAgentKind::Main)`，再按需 `lock_thread(...)`。
4. 将 Worker、Command 和测试里的纯读/纯锁路径迁移到新 `load_thread(...)`、`lock_thread(...)`，清除对隐式初始化的依赖。
5. 保留旧 `load_or_create_thread(...)` 作为兼容 wrapper，并在日志中标注迁移期使用情况。
6. 回归验证稳定前缀初始化、store 恢复、cache hit、reset 后重初始化、线程缺失处理，以及 browser/main agent 差异化初始化等关键场景。
7. 删除旧入口与旧 helper，收口 thread 初始化 ownership。

Rollback strategy:

- 若迁移中出现较大行为回归，可以暂时保留旧 wrapper 对外暴露，但内部仍必须走新的 create-path helper，不能恢复到“load/lock 隐式初始化”的旧实现。

## Open Questions

- `lock_thread(...)` 在缺失线程时，最终 API 形式是 `Option` 还是领域化 not-found error，需要在实现阶段结合现有 `SessionStoreResult` 统一一次。
