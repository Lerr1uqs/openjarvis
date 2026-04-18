## Context

当前代码已经有几块可以直接复用的基础设施：

- `SessionManager` 已经提供显式 `create_thread / load_thread / lock_thread` 生命周期入口；
- `ThreadRuntime::initialize_thread(thread, thread_agent_kind)` 已经能根据 `ThreadAgentKind` 写入稳定 prompt 和默认工具绑定；
- `AgentLoop` 已经可以在持有 live `Thread` 的前提下完成完整的一轮执行；
- thread-scoped toolsets 已经是正式模型，工具可见性和工具调用都以 `Thread` 自身状态为真相。

但系统当前仍然是“一个主线程消息 -> 一个主 worker -> 一个主线程 agent loop”模型。要支持 subagent，不能只是加几个工具名，还必须把下面这些边界正式设计清楚：

- 子线程身份不能继续复用 channel 的 `external_thread_id`
- 主线程调用 subagent 时不能复用当前主 worker 请求队列同步等待
- subagent 的 prompt/toolsets 应继续由既有 `ThreadAgentKind` 驱动，而不是再造一套重叠概念
- subagent 的事件默认只用于调试记录，不应直接进入外部 channel 发送面
- `yolo / persist` 需要在现有 thread-owned 持久化边界上定义出明确生命周期

## Goals / Non-Goals

**Goals:**

- 复用现有 `Thread`、`ThreadRuntime` 和 `AgentLoop` 模型实现 subagent，而不是为子代理重建第二套执行框架。
- 为 subagent 引入独立的 child-thread identity，使其与 channel `external_thread_id` 彻底解耦。
- 让同一父线程下的 subagent 具备可恢复、可列出、可关闭的正式状态模型。
- 让主线程同步阻塞地调用 subagent，并以单次 `ToolCallResult` 聚合返回结果。
- 通过独立 subagent worker 池避免主线程在同一 worker 请求队列上自锁。
- 让 subagent 线程继续通过 `ThreadAgentKind` 选择 prompt 和默认工具绑定。
- 为 `persist` 和 `yolo` 两种生命周期提供清晰且一致的持久化语义。

**Non-Goals:**

- 本次不支持异步后台 subagent 任务。
- 本次不支持流式子结果直接对用户可见。
- 本次不支持同一父线程下同一种 subagent profile 并存多个实例。
- 本次不引入新的外部 channel 协议，不让 channel 直接感知 subagent 概念。
- 本次不重做现有主线程 Router 主链路；subagent 首版只作为 agent 工具能力被主线程调用。

## Decisions

### 1. subagent 继续复用现有 `ThreadAgentKind`，不新增第二套 `AgentKind`

当前系统里，线程自己的稳定角色、prompt 模板和默认工具绑定已经由 `ThreadAgentKind` / `ThreadAgent` 驱动。subagent 本质上也是一种线程 profile，因此继续复用这套模型即可。

首版设计中：

- 主线程继续使用 `ThreadAgentKind::Main`
- 浏览器 subagent 使用 `ThreadAgentKind::Browser`
- 后续新增其他 subagent profile 时，也直接扩展 `ThreadAgentKind`

“这个线程是不是 subagent”不再由 `kind == sub` 之类的二值枚举判断，而由 child-thread identity 判断。

Alternative considered:

- 新增 `AgentKind::{Main, Sub}`，再让 subagent 内部自己带 profile
  Rejected，因为这会和现有 `ThreadAgentKind` / `ThreadAgent` 形成双重真相，导致 prompt 选择、默认工具绑定和线程角色判断再次分裂。

### 2. 子线程身份通过独立 child-thread identity 表达，不改写 `external_thread_id`

channel `external_thread_id` 的语义必须继续保持为“上游聊天软件提供的外部会话标识”。subagent 是 OpenJarvis 内部派生线程，不能把它再拼回 `user:channel:external_thread_id` 这一层语义里。

因此本次为线程新增独立 child-thread identity 模型。该 identity 至少包含：

- `parent_thread_id`
- `subagent_key`
- `spawn_mode`

其中：

- `parent_thread_id` 表示这个 child thread 属于哪个父线程
- `subagent_key` 表示具体 subagent profile 的稳定名字，例如 `browser`
- `spawn_mode` 表示当前实例以 `persist` 还是 `yolo` 方式运行

对应的内部 thread identity 解析改为：

- 主线程仍然沿用现有 `channel + user + external_thread_id -> thread_id`
- 子线程通过“父线程真相 + child-thread identity”派生独立的内部 `thread_id`

这样可以保持：

- channel 字段语义不变
- 父线程和子线程都有稳定、可恢复的内部 thread id
- 后续若需要增加更多子线程状态，不需要再污染 external locator

Alternative considered:

- 直接把 locator 改成 `user:channel:external_thread_id:subagent:browser`
  Rejected，因为这会把外部 channel 字段和内部 child-thread 语义混在一起，也让 `external_thread_id` 失去原有边界。

### 3. 同一父线程下，同一种 subagent profile 同时只允许一个实例

首版明确约束：对同一个父线程来说，同一个 `subagent_key` 只能有一个 child thread。

例如：

- 一个父线程只能有一个 `browser` subagent
- 若再次请求 `spawn_subagent(browser)`，系统返回已存在实例或直接复用已有实例

这样做有两个好处：

- 避免首版在 locator、状态管理和 `list/close/send` 语义上引入“多实例索引”
- 与当前工具型 subagent 的目标一致，先支持稳定复用一个专用线程，而不是扩展到任意数量会话

Alternative considered:

- 从第一版开始支持同 profile 多实例
  Rejected，因为会把 identity、恢复、工具参数和 `list_subagent` 协议复杂度全部抬高，但首版没有对应收益。

### 4. `SessionManager` 扩展 child-thread create/load/lock，但继续沿用显式访问生命周期

subagent 线程仍然是正式 `Thread`，因此继续复用 `SessionManager` 的显式线程访问生命周期，而不是在工具层自己偷偷维护第二套线程 map。

推荐模型是：

- 主线程工具先解析父线程 locator
- 基于父线程 locator + child-thread identity 派生 child thread locator
- 继续通过 `create_thread`、`load_thread`、`lock_thread` 操作该 child thread

也就是说，child thread 只是“另一种来源的线程 identity”，不是另一种线程容器。

同时，`create_thread(..., thread_agent_kind)` 的现有约束继续保留：

- 子线程初始化仍然只能通过显式 create 路径触发
- 子线程已有持久化 `ThreadAgent` 时，后续重复 create 不允许偷偷改写它

Alternative considered:

- 在 `SubagentRunner` 内维护完全独立于 `SessionManager` 的临时线程仓库
  Rejected，因为这会绕过现有 thread-owned 持久化边界，也会让恢复、工具状态和生命周期再次分裂。

### 5. subagent 调度使用独立 worker 池，不复用主 worker 请求队列

主线程在 tool 调用中同步等待 subagent 结果。如果继续复用当前主 worker 请求队列，主线程会在同一消费者上等待自己的下游任务完成，存在结构性自锁风险。

因此本次新增独立 `SubagentRunner` 和 subagent worker 池：

- 主线程仍由现有主 worker 执行
- `send_subagent` 在工具调用中把任务交给独立 subagent worker 池
- subagent worker 池内部复用现有 `AgentLoop` 运行 child thread

这样可以保证：

- 主线程可以同步阻塞等待 subagent 结果
- 不会和主 worker 的请求队列互相卡死
- 未来如需扩容，也能单独控制 subagent 执行并发度

Alternative considered:

- 在主 worker 内直接递归复用同一个 request queue
  Rejected，因为同步等待会让当前执行单元等待自己，结构上不安全。

### 6. `SubagentRunner` 的调用模型是同步阻塞 RPC，而不是异步任务系统

首版 subagent 只支持同步阻塞语义：

1. 主线程调用 `send_subagent`
2. `SubagentRunner` 在独立 worker 池中运行 child thread
3. child thread 完成后返回聚合结果
4. 主线程把该结果作为一次普通 tool result 继续参与本轮推理

因此首版工具职责约束为：

- `spawn_subagent`: 创建或确保某个 subagent child thread 已存在
- `send_subagent`: 同步执行一次子线程请求并返回结果
- `close_subagent`: 关闭一个已存在的 subagent 线程
- `list_subagent`: 列出当前父线程下已存在的 subagent

其中不支持：

- 后台异步执行
- 先启动再订阅结果
- 子线程中间输出直接流式返还给用户

Alternative considered:

- 首版就做成后台任务 + 轮询结果
  Rejected，因为这需要额外的状态机、事件模型和用户可见协议，复杂度超出当前变更目标。

### 7. `AgentLoop` 通过 `AgentEventSender::for_subagent_thread` 进入“只记录不发送”的内部事件模式

当前 `AgentLoop` 运行时要求提供 `IncomingMessage` 和 `AgentEventSender`。subagent 继续复用 `AgentLoop`，但它没有真实的 channel reply 目标，也不应该把 committed 事件直接发到 Router。

因此新增：

- `AgentEventSender::for_subagent_thread(...)`

它的语义是：

- 仍然生成完整的 dispatch metadata，方便日志和调试
- committed event 默认只交给内部记录器
- 不进入 Router，也不进入外部 channel 发送链路

对应执行路径：

- `SubagentRunner` 构造一个内部 `IncomingMessage` 兼容层对象
- 使用 `for_subagent_thread(...)` 创建 sender
- committed handler 只做日志记录和必要的调试聚合

这样可以在尽量少改 `AgentLoop` 入参模型的前提下，复用现有执行链路。

Alternative considered:

- 为 subagent 单独写一个不需要 sender 的第二套 agent loop 入口
  Rejected，因为这会让执行路径再次分叉，不利于后续维护。

### 8. `persist` 和 `yolo` 都复用现有 thread-owned 持久化边界，但回收策略不同

首版不为 `yolo` 引入特殊“纯内存线程”模型。无论 `persist` 还是 `yolo`，执行时都继续复用现有正式线程持久化语义：

- 子线程消息通过 `push_message(...)` 成为正式历史
- 子线程工具状态、loaded toolsets 和其他 thread-owned state 继续按现有规则持久化

两者区别只在回收时机：

- `persist`: child thread 保持存在，可多次 `send_subagent`
- `yolo`: 当前这次 `send_subagent` 成功返回 `ToolCallResult` 后，立刻对 child thread 做 best-effort `remove`

这里明确接受一个首版权衡：

- `yolo` 的 remove 不与父线程后续 `ToolResult` commit 做事务绑定
- 如果极低概率下父线程在拿到 tool result 后提交失败，允许丢失这次 yolo child thread 的调试现场

Alternative considered:

- 为 `yolo` 单独做纯内存线程
  Rejected，因为这会引入第二套执行语义，而且当前用户明确接受继续复用持久化边界。

### 9. `close_subagent` 和 `remove` 分开建模，且 `remove` 下沉到底层线程/存储能力

首版需要同时支持两类回收动作：

- `close_subagent`: 面向 `persist` 线程，结束其后续使用并清空线程状态
- `remove`: 面向底层回收，彻底移除 child thread 的已落盘记录

其中：

- `yolo` 默认在一次成功调用后直接执行 best-effort `remove`
- `persist` 可以先 `close_subagent`，必要时再由内部维护路径执行 `remove`

这里的关键约束是：

- `close_subagent` 是面向主线程工具层暴露的高层生命周期动作
- `remove` 不作为首版对外 subagent 工具接口暴露
- `remove` 应该由 `Thread` 或 `SessionStore`/同等底层线程存储层提供正式删除能力，供 `yolo` 自动回收或后续内部维护路径复用

这样可以把“逻辑上结束使用”和“物理上彻底移除线程记录”分开，也避免把“删除已落盘线程”错误暴露成普通 agent 可随意调用的高层能力。

Alternative considered:

- 只保留 `close_subagent`，不区分 close 和 remove
  Rejected，因为 `yolo` 的自动回收语义本质上需要物理删除已落盘记录，而 `persist` 的人工关闭更接近逻辑结束，合在一起会让行为模糊。

Alternative considered:

- 把 `remove` 也作为对外 subagent 工具暴露
  Rejected，因为这会把“删除正式线程记录”的底层能力直接暴露给高层 agent，边界过重，也不利于后续统一收口线程删除语义。

## Risks / Trade-offs

- [Risk] child-thread identity 一旦设计不稳，后续扩展更多 subagent profile 会反复返工
  Mitigation: 首版就显式区分 parent thread identity 与 child-thread identity，不再复用 `external_thread_id`。

- [Risk] 新增独立 worker 池会让启动装配与测试接线变复杂
  Mitigation: 让 subagent worker 池继续复用既有 `AgentLoop` / `ThreadRuntime` / `SessionManager`，只新增最薄的一层调度器。

- [Risk] `yolo` 的 best-effort remove 在极低概率失败场景下会丢失调试现场
  Mitigation: 首版接受这个窗口，后续若真实出现问题，再补延迟删除或 GC 机制。

- [Risk] `for_subagent_thread` 若实现过轻，可能让内部事件调试信息不足
  Mitigation: 要求该 sender 继续保留完整 metadata，只是默认不进入外部发送链路。

- [Risk] 一个 profile 只允许一个实例会限制某些未来场景
  Mitigation: 这是首版有意收敛 scope；后续若要放开多实例，再在 child-thread identity 上增加 instance 维度。

## Migration Plan

1. 为线程状态与 locator 增加 child-thread identity 所需的最小字段，并定义父线程下唯一 profile 的约束。
2. 扩展 `ThreadAgentKind` 与线程初始化链路，使 subagent profile 继续走既有 prompt / toolset 绑定模型。
3. 在 `Thread`、`SessionStore` 或同等底层线程删除路径中新增正式 `remove` 能力，供 `yolo` 回收已落盘 child thread 记录。
4. 新增 `SubagentRunner` 和独立 worker 池，打通“主线程工具调用 -> 子线程执行 -> 同步结果返回”闭环。
5. 为 `AgentEventSender` 增加 `for_subagent_thread(...)`，并接上只记录不发送的 committed handler。
6. 新增 `spawn_subagent`、`send_subagent`、`close_subagent`、`list_subagent` 工具，并补齐同步阻塞语义和生命周期处理。
7. 增加 `yolo/persist` 回归测试、child-thread 恢复测试、底层 remove 测试、单 profile 唯一实例测试，以及主线程同步等待 subagent 结果的集成测试。

Rollback strategy:

- 若引入 subagent runtime 后出现较大行为风险，可以先移除 subagent 工具入口与 `SubagentRunner`，保留新增的 thread identity 与 sender 内部兼容层作为非活跃基础设施，不必回滚现有主线程执行模型。

## Open Questions

- `close_subagent` 是否只允许作用于 `persist` 实例，还是允许对 `yolo` 实例做幂等关闭；这会影响工具返回文案，但不阻塞当前 design 落地。
