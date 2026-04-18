## Context

当前 `src/command.rs` 中的 `/clear` 命令只是调用 `Thread::reset_to_initial_state(...)`，把持久化消息和线程级状态直接清成空初始值。这个实现的问题不是“清空不彻底”，而是“清空后停在一个不完整状态”：

- 持久化 `System` 前缀会全部消失
- `ThreadState::default()` 会把 `agent` 重置成默认值，当前 thread kind 真相暂时丢失
- child thread 若走普通 reset，也会丢掉 `child_thread` identity
- 命令执行完成后到下一次 router 再次显式初始化之前，当前 thread 在存储里并不是一个完整的 initialized thread

用户现在要求的不是保留旧 `/clear` 名字，而是把语义改成“当前 thread 开一个新的会话”，这意味着命令完成后 thread 必须立即回到 initialized 状态。

## Goals / Non-Goals

**Goals:**

- 删除 `/clear`，新增 `/new` 作为当前 thread 的重初始化命令。
- `/new` 执行完成后立即恢复当前 thread kind 对应的稳定 `System` 前缀、feature state 和默认工具 truth。
- `/new` 在 child thread 上保留当前 `child_thread` identity，不把 browser thread 弄丢。
- 复用现有 `ThreadRuntime` 初始化链路，不在命令层复制 prompt / feature / toolset 初始化逻辑。

**Non-Goals:**

- 不新增“命令执行中切换 thread kind”的能力。
- 不把 `/new` 做成跨线程命令或会话管理入口；它只作用于当前 thread。
- 不修改其他线程命令如 `/context`、`/echo` 的行为。

## Decisions

### 1. `/clear` 直接移除，`/new` 成为唯一的线程重开命令

用户问题的根因不是命令命名，而是 `/clear` 的语义允许 thread 暂时失去初始化 truth。继续保留 `/clear` 并偷偷改语义只会留下双重认知负担，因此本次直接移除 `/clear`，由 `/new` 承担“当前 thread 开新会话”的唯一入口。

备选方案：

- 保留 `/clear` 名字，只把实现改成重初始化。  
  Rejected，因为名字仍然暗示“清空到空状态”，与真实语义不符。
- 同时保留 `/clear` 和 `/new` 两个别名。  
  Rejected，因为会长期保留两套用户入口，不利于收敛。

### 2. 新增 `ThreadRuntime::reinitialize_thread(...)`，内部复用已有初始化链路

`/new` 需要的不是一次简单状态写入，而是完整的“reset + initialize”。初始化逻辑已经集中在 `ThreadRuntime::initialize_thread(...)`，其中包含：

- agent kind capability profile 解析
- stable system prompt / shell env / feature prompt 写入
- 允许 feature / toolset 的过滤与恢复

因此新增一个 runtime 级辅助入口最稳妥：先捕获当前 `thread_agent_kind()`，再调用 `reset_to_initial_state_preserving_child_thread(...)` 清空历史，最后对同一个 kind 调用 `initialize_thread(...)`。这样命令层只负责表达“我要重开线程”，不复制任何初始化细节。

备选方案：

- 在命令层手写一套 `/new` 初始化消息拼装。  
  Rejected，因为会重新引入第二套 thread truth。
- 只调用 `reset_to_initial_state(...)`，等待下一条普通消息再初始化。  
  Rejected，因为这正是当前问题来源。

### 3. Command 执行路径显式接入已安装的 `ThreadRuntime`

当前 `CommandHandler` 只拿到 `IncomingMessage` 和 `Thread`，无法直接重跑初始化。生产链路里 router 已经通过 `SessionManager` 持有 `ThreadRuntime`，所以命令执行时直接把该 runtime 作为可选依赖透传给命令 handler。`/new` 若拿不到 runtime，就显式失败；正常 router + agent 启动路径则一定可用。

备选方案：

- 给 `CommandRegistry` 自己维护一份独立 runtime。  
  Rejected，因为 runtime 真相已经挂在 router/session 主链路上，重复持有没有必要。

## Risks / Trade-offs

- [Risk] 直接调用重初始化会让 `/new` 对运行时依赖更强，不再是一个纯 `Thread` 本地命令。  
  → Mitigation: 依赖只复用现有 `ThreadRuntime`，不新增第二套服务容器。
- [Risk] child thread reset 时若误用普通 reset，会丢失 `child_thread` identity。  
  → Mitigation: 统一走 `reset_to_initial_state_preserving_child_thread(...)`。
- [Risk] 现有直接调用 `CommandRegistry::with_builtin_commands()` 的 UT 默认不带 runtime。  
  → Mitigation: 保留无 runtime 的默认路径，仅为 `/new` 测试显式安装 runtime 或通过带 agent 的 router 链路覆盖。
