## Context

当前系统已经具备以下可复用基础：

- `SessionManager::list_child_threads(...)` 可以枚举 parent 名下的 persisted child thread。
- `ThreadRuntime::initialize_thread(...)` / `reinitialize_thread(...)` 已经是 thread 初始化真相。
- `SubagentRunner` 已能在独立 worker 中同步执行 child thread，并返回聚合结果。
- `persist` child thread 与 `close_subagent` 的“保留 identity、清空历史”模型已经存在。

但当前存在两个设计问题：

- subagent 工具把“建线程”和“发首个任务”拆成 `spawn` + `send`，使 `yolo` 模式必须多走一步，语义反而最不顺。
- parent `/new` 只处理自己，不处理挂载在当前 parent 之下的 `persist` child thread，导致 parent 与 child 的初始化前缀不再同步。

## Goals / Non-Goals

**Goals:**

- 让 `spawn_subagent` 成为统一入口：创建或复用 child thread，并执行首个 task。
- 明确 `send_subagent` 与 `close_subagent` 只服务 `persist` 模式。
- 让 parent `/new` 级联到当前 parent 名下全部 `persist` child thread。
- 继续复用现有 `initialize_thread(...)` / `SubagentRunner::run(...)`，不引入第二套初始化或执行主链路。

**Non-Goals:**

- 本次不改 child-thread identity 模型，不引入新的 child id 派生规则。
- 本次不增加新的 subagent 管理工具。
- 本次不支持运行时切换某个已存在 child thread 的模式策略。
- 本次不修改 `model/*.md` 中的组件定义文档。

## Decisions

### 1. `spawn_subagent` 改为“建好就执行”的首轮入口

`spawn_subagent` 新增必填 `content` 参数。工具执行时：

1. 解析 parent locator 与目标 `subagent_key`
2. 依据 `spawn_mode` 创建或复用 child thread
3. 立刻把 `content` 当作一条 incoming user message 送入 child thread
4. 同步等待 `SubagentRunner` 返回结果
5. 把 child 结果直接作为本次工具调用结果返回

这样 `yolo` 与 `persist` 的首轮入口统一，不再要求调用方先“准备”再“发第一条消息”。

Alternative considered:

- 保留 `spawn=准备`、`send=首轮执行`
  Rejected，因为这会继续让 `yolo` 模式多出无意义的第二次工具调用，也与用户要求不一致。

### 2. `send_subagent` 与 `close_subagent` 只面向已存在的 `persist` child thread

`send_subagent` 不再隐式创建 child thread。它只会：

- 查找当前 parent 名下已存在的 child thread
- 校验该 child thread 的 persisted `spawn_mode == persist`
- 校验该 child thread 当前处于已初始化可用状态
- 把后续消息同步送入该 thread 并返回结果

`close_subagent` 也只操作 `persist` child thread；若当前 child 不存在或不是 `persist`，直接返回清晰错误或未找到结果。

Alternative considered:

- 继续让 `send_subagent` 在缺失时自动创建 child thread
  Rejected，因为这会再次模糊“首轮启动”和“后续交互”的边界。

### 3. `yolo` child thread 在一次 `spawn_subagent` 完成后立即 best-effort 回收

`yolo` 模式的 child thread 只服务本次 `spawn_subagent` 调用，因此在执行结束后立即尝试 `remove_thread(...)`，不再保留给 `send_subagent` 或 `close_subagent` 后续使用。

无论 child 最终回复成功还是失败，工具层都不会把它视作后续可交互实体。

Alternative considered:

- 只在成功时回收，失败时保留调试现场
  Rejected，因为这样 `yolo` child thread 仍可能被下一次调用复用，不满足“只用一次”的语义。

### 4. parent `/new` 通过 runtime 级联 reinitialize 自己名下的全部 `persist` child thread

`ThreadRuntime::reinitialize_thread(...)` 在处理 parent thread 时，会先从请求绑定的 `SessionManager` 枚举当前 parent 名下 child threads，然后仅挑出 `spawn_mode == persist` 的记录逐个执行 reset + initialize。

实现上继续复用现有 `reset_to_initial_state_preserving_child_thread(...)` 与 `initialize_thread(...)`，避免手写第二套 child reinit 逻辑。

child thread 自己执行 `/new` 时，不会再向下枚举额外 child；它只重置自己，并保留自己的 child identity。

Alternative considered:

- 只在 `/new` 命令层做级联，`ThreadRuntime` 只负责当前 thread
  Rejected，因为级联重初始化本质上仍属于 thread runtime 的初始化语义，放在命令层会让入口和真相分裂。

### 5. parent `/new` 的级联范围只包含 `persist`

`yolo` child thread 不属于可复用会话体，理论上应在单次调用结束后被回收，因此 `/new` 不会把它纳入级联范围。即使极端情况下存在清理失败残留记录，本次也不会把它当作长期 child session 一起 reinit。

Alternative considered:

- 对所有 child thread 一律级联 `/new`
  Rejected，因为这会把一次性 `yolo` child 也错误提升为长期会话对象。

## Risks / Trade-offs

- [Risk] `spawn_subagent` 参数变化会影响现有测试和提示词假设
  Mitigation: 同步更新工具 schema、feature prompt 文案和相关 UT。
- [Risk] parent `/new` 级联 reinit 不是事务操作，若中途失败可能出现 parent 与部分 child 已重置、部分未重置的中间状态
  Mitigation: 关键路径增加日志，并在首版保持失败即返回错误，避免静默吞错。
- [Risk] `yolo` 失败后立即回收会损失 child thread 历史现场
  Mitigation: 保留 hook 与工具元数据日志，把“单次执行语义”优先级放在调试便利之上。
