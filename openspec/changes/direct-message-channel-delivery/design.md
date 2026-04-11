## Context

当前 `message-level-event-dispatch` 的实现把消息级外发做成了 thread 内部的 dispatch 账本：

- `Thread` 保存待发送 item、发送顺序号、ack cursor 和 finalized pending dispatch turn。
- `AgentLoop`/`Worker` 在 commit 消息后把 dispatch item 发给 router。
- router 发送成功后再通过 session 回写 `Thread::mark_dispatch_item_dispatched(...)`。

这个模型的问题不是“消息不能逐条发”，而是“消息 commit、持久化、发事件这三件事没有被收敛成一个明确边界”。结果是：

- thread 锁被扩展成执行期共享锁，worker 和 router 都会争抢它。
- router 为了 ack 需要 mutate thread，边界从“发送组件”泄漏到了“线程内容聚合根”。
- session 被拉进了 dispatch 管理，而不是只做 thread commit persistence。

本 change 需要把这些语义收敛成一条主路径：`agent.commit_message(...)` 调用 `thread.push_message(...)` 完成消息事实写入和持久化，然后由 agent 立即发出 committed message event。router 只消费事件并向外发送，不再承担任何 thread/session 回写职责。

## Goals / Non-Goals

**Goals:**

- 让 `Thread` 只保存线程内容事实、当前 turn working set、finalized turn 和 dedup 边界，不再保存 router ack / dispatch 进度。
- 让 `Thread` 自己拥有 commit persistence 语义，`push_message(...)` 在返回前就完成持久化。
- 把 thread 锁收缩到初始化、消息 commit、turn finalization 这些事实提交点，而不是整轮请求生命周期。
- 让每条 committed assistant 文本、tool call、tool result、terminal failure 在 commit 成功后立即由 agent 发到消息信道。
- 让 `Session` 退回为 thread commit persistence 的基础设施，不再拥有 dispatch/checkpoint 概念。
- 明确放弃中断后的自动补发/恢复链路：若某条中间 event 没有成功发出，后续重新请求只依赖已经持久化的消息上下文继续执行。

**Non-Goals:**

- 本次不引入 token streaming；发送粒度仍然是完整消息或结构化事件。
- 本次不调整 memory/tool/compact 的业务含义，只调整消息提交与发送边界。
- 本次不修改 `model/**` 架构文档。
- 本次不要求 channel 必须支持消息撤回、编辑或聚合展示。
- 本次不引入 session 级精确发送游标、自动补发机制或 exact-once 发送承诺。

## Decisions

### 1. `Thread::push_message(...)` 直接拥有 commit persistence 语义

`Thread` 不再把“先改内存，再由外部补一层 persist”作为默认提交模型。系统改为让 `push_message(...)` 成为完整的 commit 边界：

- `push_message(...)` 在 thread 内写入消息事实；
- 同一调用内完成持久化；
- 返回一个稳定的 committed message 结果，供 agent 继续发事件。

`Thread` 不需要直接依赖 `SessionManager` 具体类型，但需要挂一个抽象的 thread persistence runtime attachment。这样可以保持 `Thread` 对 commit 语义的 ownership，同时不把 store/session 细节硬编码进领域对象。

Alternative considered:

- 让 `Thread` 直接持有 `SessionManager` 引用。  
  Rejected，因为这会让领域对象直接依赖具体基础设施实现，耦合过高；这里需要的是抽象的 persistence capability，而不是具体 manager。

### 2. `agent.commit_message(...) = thread.push_message(...) + send event`

发送事件的职责属于 agent，不属于 thread/session/router。`agent.commit_message(...)` 的职责固定为两步：

1. 调 `thread.push_message(...)` 完成消息 commit 和持久化；
2. 在成功返回后，把 `CommittedMessageEvent` 发到消息信道。

其中：

- 若 `push_message(...)` 失败，则不得发事件；
- 若 `push_message(...)` 成功，则事件一定代表一个已经持久化的 committed message；
- router 只消费事件，不反向决定 thread 状态。

Alternative considered:

- 让 thread 自己在 `push_message(...)` 内直接把消息发到 router。  
  Rejected，因为消息发送属于 agent 执行框架职责，不是 thread 领域职责。

### 3. 即时持久化边界只保留在消息 commit

worker 不再在整轮请求期间长期持有 thread 锁。系统只在“正式消息 commit”这一类事实写入点做即时持久化：

- `push_message(...)` 写入正式消息后立即持久化；
- `open_turn(...)` / `begin_turn(...)` 只建立本地 turn 生命周期状态；
- `finalize_turn_success(...)` / `finalize_turn_failure(...)` 只结束本地 turn 生命周期并生成完成态；
- finalized turn 的最终提交仍由 session 在回合结束时统一完成。

消息真正发往 channel 的动作发生在 `push_message(...)` 成功返回之后，由 agent 在锁外完成。

Alternative considered:

- 让 `open_turn(...)` 和 `finalize_turn_*` 也承担即时持久化。  
  Rejected，因为它们不是正式消息 commit 边界；把持久化扩散到这些接口只会重新放大线程执行期开销。

### 4. Session 不再拥有 dispatch / checkpoint 概念

`Session` 的职责收缩为：

- 解析和定位 thread；
- 提供 thread commit 所需的持久化能力；
- 提交 finalized turn 与 dedup 记录。

`Session` 不再负责：

- 维护 dispatch cursor；
- 记录 ack 状态；
- 管理 pending dispatch ledger。

Alternative considered:

- 把 dispatch/checkpoint 从 `Thread` 挪到 `Session`。  
  Rejected，因为这只是换个地方继续维护发送账本，没有解决边界问题。

### 5. 不提供中断后的自动补发或重放

系统不再维护 session/thread 级 dispatch cursor，也不再为 committed event 提供自动恢复、补发或重放机制。若请求在中途被打断：

- 已经持久化的正式消息仍然留在 thread 中；
- 未成功发出的中间 event 不会被系统自动补发；
- 后续新的请求直接基于已有消息上下文继续执行。

Alternative considered:

- 继续为 committed event 设计恢复/重放 identity。  
  Rejected，因为用户已经明确不需要这套语义，保留它只会把 dispatch 复杂度换个名字重新带回来。

### 6. turn finalization 与消息发送彻底解耦

turn finalization 只保留这些职责：

- 标记 turn 成功或失败；
- 生成最终 reply / audit 结果；
- 提交 external-message dedup；
- 结束 active turn 生命周期。

它不再负责：

- 聚合待发送消息；
- 追踪消息送达；
- 等待 router ack。

`open_turn(...)` 和 `finalize_turn_success(...)` / `finalize_turn_failure(...)` 都只属于 turn 生命周期管理接口：

- `open_turn(...)` 只开启 turn、绑定基础元数据并记录日志；
- `finalize_turn_*` 只关闭 turn、记录完成态并记录日志；
- 它们 SHALL NOT 缓冲、补齐、伪造或隐式追加任何用户可见消息/event。

如果某条失败消息需要对用户可见，它必须在 finalize 之前由 agent 显式 `commit_message(...)`，而不是由 `finalize_turn_failure(...)` 在内部兜底生成。

这意味着“消息能否发送”只取决于消息是否已经 commit，而不是 turn 是否已经 finalized。

Alternative considered:

- 仍然保留 finalized turn 后统一补齐未发消息的兜底语义。  
  Rejected，因为这会重新把发送边界拉回 turn，和本 change 目标冲突。

## Risks / Trade-offs

- [Risk] commit 频率上升后，thread snapshot 持久化次数会增加。  
  Mitigation: 后续实现可优化 snapshot 写入成本，但语义上仍以单条 commit 为原子边界。

- [Risk] “thread commit 成功但 router 发送失败”会留下已持久化但未真正外发的中间消息。  
  Mitigation: 显式接受这种情况；后续新的请求基于已持久化消息上下文继续执行，而不是自动补发旧 event。

- [Risk] 如果 `Thread` 的持久化能力直接绑定具体 `SessionManager`，会把领域对象和基础设施重新耦合。  
  Mitigation: 使用 trait/attachment 形式提供 persistence capability，而不是直接注入具体 manager。

- [Risk] 取消自动恢复后，用户可能看不到某些中途失败前未发出的 event。  
  Mitigation: 保证正式消息已持久化；下一轮请求仍能基于完整消息上下文继续，不影响线程事实一致性。

## Migration Plan

1. 新增 `message-channel-delivery` capability，并修改 `thread-context-runtime` 对 thread 锁和所有权的约束。
2. 从 `Thread` 和 `Session` 中删除 dispatch ledger、ack cursor、pending dispatch 等发送账本结构。
3. 为 `Thread` 引入抽象的 commit persistence attachment，让 `push_message(...)` 在返回前完成持久化。
4. 把 `AgentLoop`/`Worker` 调整为 `agent.commit_message(...) = thread.push_message(...) + send event`。
5. 重写 router 为纯事件消费者，删除 thread/session 回写路径。
6. 删除旧的 dispatch 相关测试，新增 commit 边界、agent 发事件、router 纯发送、无自动恢复等 UT。

Rollback strategy:

- 如果实现中需要分步迁移，可以先保留旧 dispatch 字段的只读兼容层，但所有新写入路径都必须改成 `push_message(...)` 自持久化 + agent 发事件，不再新增任何 dispatch/checkpoint 状态。

## Open Questions

- tool call / tool result 是否最终也要进入正式消息历史，还是只作为即时外发 event 保留在执行期。
