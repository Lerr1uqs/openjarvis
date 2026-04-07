# Agent Loop Message 驱动重构方案

更新时间：2026-04-07

## 1. 背景

当前主链路里的核心矛盾不是 `Thread` 能不能管理消息，而是系统同时保留了两套边界模型：

- 一套是 turn-driven 模型：
  `AgentLoop` 在 turn 结束时统一 finalize、统一持久化、统一派发。
- 一套是 message-driven 诉求：
  每产生一条正式消息，就应该立即进入线程事实，并按 message 触发持久化和外发。

这两套模型同时存在，直接带来了这些问题：

- `Thread` 已经有 current turn 能力，但消息事实仍然没有完全收口到 `Thread`。
- `AgentLoop` 还在为 turn 边界服务，内部保留了多处兼容性状态。
- `Router` 和 `Session` 的主提交边界仍然偏 turn，不是 message。
- `compact` 的行为与普通 message append 的行为混在一起，边界不清晰。

这次重构的目标，是把系统收敛到一条明确的共识：

- 正式消息按 message 为单位落盘和发送。
- `turn` 只保留为日志、调试、审计、事件聚合和完成状态的作用域。
- `Thread` 成为线程消息与线程状态的唯一 owner。

## 2. 当前共识

本方案基于以下前提：

- 当前没有命中 memory 后动态注入正文的需求。
- runtime instruction 已经在 `init_thread()` 阶段转成稳定的 system prompt，并持久化落盘。
- auto compactor 相关提示是作为user的role注入进上下文的，不是 request-only 临时注入，需要落盘。

这意味着当前系统不需要 request-only runtime message overlay。

换句话说，在当前范围内：

- LLM 能看到的正式消息
- 线程里保存的正式消息
- 后续轮次会继续带上的正式消息

可以被定义为同一份 `Thread.messages` 真相。

## 3. 目标

- `Thread` 成为线程消息与线程状态的唯一宿主。
- `AgentLoop` 不再维护 loop-local message source of truth。
- system prompt 全部在 `init_thread()` 中一次性注入，并始终位于消息序列开头。
- 正式消息按 message append 持久化。
- 正式外发按 message 触发，不再等待整个 turn 完成。
- `turn` 仅承担日志、调试、审计、完成状态、dedup 绑定等元信息职责。
- `compact` 作为唯一允许 rewrite 历史的特殊路径，被单独建模。

## 4. 非目标

- 本次不设计 request-only runtime message。
- 本次不引入 memory recall 注入层。
- 本次不把 turn 恢复成正式消息提交边界。
- 本次不要求第一阶段就替换所有存储实现，但要先把目标存储模型说清楚。

## 5. 新定义

### 5.1 Message

`Message` 是线程中的正式消息单位。

它有三个属性：

- 进入 `Thread.messages`
- 可以被持久化
- 可以驱动一次对外发送或一次对外可见事件

这里的“按 message 发送”不要求所有角色都原样映射到聊天平台文本，但要求发送边界由单条新提交 message 驱动，而不是由 turn batch 驱动。

### 5.2 Turn

`Turn` 不再是正式消息提交边界。

`Turn` 的新职责只有：

- 标识一次用户输入驱动的一次执行会话
- 记录开始时间、结束时间、状态、错误、统计信息
- 作为日志、调试、审计、事件归档的分组维度
- 为 external message dedup 提供绑定目标

`Turn` 不负责：

- 批量提交消息
- 批量发送消息
- 暂存正式消息内容

### 5.3 Thread

`Thread` 是唯一消息 owner。

它负责：

- 保存稳定消息序列
- 保证 system message 永远位于开头前缀
- 管理线程级 tool state / feature state / approval state
- 管理 active turn 的元信息

它不再依赖外部模块手工组装第二份消息向量。

## 6. 目标数据模型

### 6.1 Thread 内存模型

建议将 `Thread` 收敛成下面这类结构：

```rust
pub struct Thread {
    pub locator: ThreadContextLocator,
    pub thread: ThreadContext,
    pub state: ThreadState,
    revision: u64,
    current_turn: Option<ThreadTurnMeta>,
}

pub struct ThreadContext {
    pub messages: Vec<ChatMessage>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub struct ThreadTurnMeta {
    pub turn_id: Uuid,
    pub external_message_id: Option<String>,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub status: ThreadTurnStatus,
    pub emitted_message_ids: Vec<Uuid>,
    pub tool_event_ids: Vec<Uuid>,
}
```

这版模型的重点是：

- `messages` 只有一份正式序列
- 不再让 `current_turn` 持有 `working_messages`
- 不再让 `current_turn` 持有 `history_override`
- 不再把“消息提交”延后到 turn finalize

### 6.2 Message 顺序规则

线程消息顺序规则固定为：

1. 开头连续 `System`
2. 后续全部正式 non-system message

普通执行过程中，系统不允许在中间位置插入新的 system message。

## 7. 主链路

### 7.1 标准执行流

```mermaid
sequenceDiagram
    participant User
    participant Router
    participant Session
    participant Worker
    participant Loop as AgentLoop
    participant LLM
    participant Tool

    User->>Router: IncomingMessage
    Router->>Session: resolve_thread()
    Session-->>Router: ThreadLocator + Thread
    Router->>Worker: AgentRequest(Thread, IncomingMessage)
    Worker->>Loop: run(Thread, IncomingMessage)
    Loop->>Loop: init_thread_if_needed()
    Loop->>Loop: begin_turn()
    Loop->>Session: append user message
    Loop->>LLM: generate(Thread.messages())
    LLM-->>Loop: assistant / tool_calls
    Loop->>Session: append assistant message
    Loop->>Router: dispatch assistant event
    Loop->>Tool: execute tool
    Tool-->>Loop: tool result
    Loop->>Session: append toolcall/tool result
    Loop->>Router: dispatch tool event
    Loop->>Session: complete_turn()
    Loop-->>Worker: TurnCompleted
```

### 7.2 伪代码

```rust
fn run_one_turn(thread: &mut Thread, incoming: IncomingMessage) -> Result<TurnCompletion> {
    init_thread_if_needed(thread)?;

    let turn_id = thread.begin_turn(incoming.external_message_id.clone(), incoming.received_at)?;

    let user_message = build_user_message(&incoming);
    append_and_persist_message(thread, turn_id, user_message, false)?;

    loop {
        maybe_compact_thread(thread, turn_id)?;

        let request_messages = thread.messages();
        let visible_tools = runtime.list_tools(thread)?;
        let response = llm.generate(request_messages, visible_tools)?;

        if let Some(assistant_message) = build_assistant_message(&response) {
            append_and_persist_message(thread, turn_id, assistant_message, true)?;
        }

        if response.tool_calls.is_empty() {
            break;
        }

        for tool_call in response.tool_calls {
            let tool_call_message = build_toolcall_message(&tool_call);
            append_and_persist_message(thread, turn_id, tool_call_message, true)?;

            let tool_result = runtime.call_tool(thread, &tool_call)?;
            let tool_result_message = build_tool_result_message(&tool_call, &tool_result);
            append_and_persist_message(thread, turn_id, tool_result_message, true)?;
        }
    }

    complete_turn(thread, turn_id)?;
    Ok(build_turn_completion(thread, turn_id))
}
```

这版流程里最关键的一点是：

- `push user input` 在内部 ReAct loop 外
- 但必须发生在 `begin_turn()` 之后

## 8. 模块职责重定义

### 8.1 Thread

`Thread` 负责：

- 持有唯一正式消息序列
- 提供只读的 `messages()` 导出
- 提供 append message API
- 提供 compact rewrite API
- 提供 turn meta 生命周期 API

建议新增或保留以下接口：

- `begin_turn(...)`
- `append_message(...)`
- `replace_non_system_messages(...)`
- `record_tool_event(...)`
- `complete_turn_success(...)`
- `complete_turn_failure(...)`

建议删除或废弃以下旧接口：

- `push_turn_message(...)`
- `apply_turn_compaction(...)`
- `finalize_turn_success(...)`
- `finalize_turn_failure(...)`
- `store_turn(...)`
- `store_turn_state(...)`

这些旧接口的共同问题，是都建立在“turn 是消息提交边界”的假设上。

### 8.2 AgentLoop

`AgentLoop` 负责：

- 读取 `Thread.messages()`
- 驱动 `generate -> tool -> generate`
- 生成新正式 message
- 调用 Session 的 message append / compact rewrite / turn complete API

`AgentLoop` 不再负责：

- 持有第二份消息事实
- 在 loop 内维护多份 `xxx_messages`
- 按 turn 批量提交消息
- 按 turn 批量输出用户可见结果

### 8.3 Session

`SessionManager` 负责：

- resolve thread identity
- 提供线程级串行写入口
- 按 message append 持久化
- 在 compact 时执行原子 rewrite
- 在 turn 结束时保存 turn completion 和 dedup

`SessionManager` 不再负责：

- 外部组装 commit message 列表
- 把一整个 finalized turn snapshot 当作唯一正式提交单元

### 8.4 Router

`Router` 负责：

- 接收已持久化 message 对应的 dispatch event
- 按 message 发送外部可见结果
- 在请求结束时处理 turn complete 事件

`Router` 不再负责：

- 等待 turn batch 再统一发送正式消息
- 参与 thread message 序列处理
- 以 turn snapshot 作为主发送边界

## 9. 持久化方案

### 9.1 不推荐方案

继续沿用“整份 `snapshot_json` 每条消息重写一次”的模型，不推荐作为最终方案。

原因：

- 每追加一条消息都重写整个 thread snapshot，写放大过高
- message-driven 语义与 whole-snapshot 写法天然别扭
- compact 和普通 append 都挤在同一条 snapshot save 逻辑里，边界不清晰

### 9.2 推荐方案

推荐把 sqlite 持久化模型重构成消息归一化结构：

#### `thread_metadata`

- 线程身份
- revision
- created_at / updated_at
- 非消息状态快照

#### `thread_messages`

- `message_id`
- `thread_id`
- `turn_id`
- `seq`
- `role`
- `content`
- `tool_calls_json`
- `tool_call_id`
- `created_at`
- `is_dispatched`

#### `thread_turns`

- `turn_id`
- `thread_id`
- `external_message_id`
- `started_at`
- `completed_at`
- `status`
- `error_message`

#### `external_message_dedup`

- `thread_id`
- `external_message_id`
- `turn_id`
- `completed_at`

### 9.3 SessionStore 新接口建议

建议新增以下主接口：

- `append_thread_message(...)`
- `mark_message_dispatched(...)`
- `replace_thread_non_system_messages(...)`
- `save_thread_state(...)`
- `complete_turn(...)`
- `load_thread_messages(...)`
- `load_turns(...)`

建议删除或降级为兼容层的接口：

- `save_thread_context(...)`
- `commit_finalized_turn(...)`
- `commit_messages(...)`
- `commit_messages_with_state(...)`
- `commit_messages_with_thread_context(...)`

## 10. 外发语义

### 10.1 新规则

正式外发边界改成：

- 一条正式 message append 并持久化成功后
- 再由 router 派发与这条 message 对应的外部事件

也就是说，外发顺序必须满足：

1. thread 内存更新
2. store 持久化成功
3. router 发出这条 message 对应事件

### 10.2 不再使用 turn batch 作为正式发送边界

turn event batch 仍然可以保留，但只能用于：

- 调试记录
- 观测与 tracing
- turn 内事件归档

它不再是正式消息发送边界。

## 11. compact 的特殊处理

`compact` 是这条 message-driven 主链里的唯一特殊情况。

普通消息是 append。

`compact` 不是 append，而是 rewrite：

- 保留 system 前缀
- 原子替换全部 non-system history
- 写入新的 compact summary message
- 写入后续继续执行所需的稳定消息

因此 `compact` 需要单独提供事务型接口，而不能复用普通 append。

建议语义如下：

1. 锁定目标 thread
2. 读取当前 non-system message 视图
3. 生成 compact replacement
4. 在单事务中替换历史
5. 更新 thread revision
6. 记录 compact tool event / compact audit event

compact 是否需要对外发消息，应该单独由角色映射决定，而不是由 turn 边界决定。

## 12. 失败语义

改成按 message 持久化和发送后，失败语义也必须一起变化。

### 12.1 旧语义

旧语义允许：

- turn 中途产生一些 working message
- 如果 turn 最终失败，可以整体丢弃未提交内容

### 12.2 新语义

新语义下，已经 append 并发送出去的 message 不允许回滚。

因此失败处理必须改成：

- 保留已经提交的消息
- 再追加一条明确的 failure message 或 failure event
- turn 状态标记为 failed

这条规则必须提前讲清楚，否则协作者很容易误以为 message-driven 模型还能保留 turn rollback。

## 13. 命令与并发

如果 thread-scoped command 仍然绕过线程串行队列，直接读写 thread，会破坏新的 message-driven 一致性。

因此建议把 thread-scoped command 也纳入同一个线程串行执行入口。

原则只有一条：

- 任何会改 thread 的操作，都必须经过同一条 thread lane

## 14. 分阶段改造计划

### 第一阶段：冻结语义

- 明确 turn 只用于日志、调试、审计、dedup、完成状态
- 明确消息提交和外发都按 message
- 明确当前系统不支持 request-only runtime message

### 第二阶段：改 Thread

- 删除 current turn 内的 working message 缓冲语义
- 改为唯一正式消息序列
- 让 turn 只剩元数据

### 第三阶段：改 AgentLoop

- 删除 loop 内所有局部消息真相
- 删除 per-turn finalize message 提交路径
- 改成 append message -> persist -> dispatch

### 第四阶段：改 Session / Store

- 先补 message-level append API
- 再把 sqlite 从 snapshot_json 模型迁到 normalized message log 模型
- compact 走单独 rewrite 事务

### 第五阶段：改 Router

- `TurnFinalized` 降级
- 新增 `MessageCommittedForDispatch` 或等价事件
- 正式外发按 message 驱动

### 第六阶段：删兼容层

- 删除旧 turn-driven commit API
- 删除旧 snapshot-driven 提交路径
- 删除针对 finalized turn batch 的主流程测试

## 15. 测试要求

### Thread UT

- system prefix 始终位于开头
- append message 顺序正确
- compact rewrite 后历史正确
- complete turn 不再负责提交消息

### Session UT

- append message 可恢复
- replace non-system messages 原子生效
- complete turn 正确绑定 dedup
- revision conflict 在 message append 下可恢复

### AgentLoop UT

- user input 在 begin_turn 后、ReAct loop 前进入 thread
- 每条 assistant/tool message 都按顺序 append
- 没有 loop-local message source of truth
- 失败时追加 error message，而不是回滚已提交消息

### Router UT

- store 成功后才允许 dispatch
- append 多条消息时外发顺序正确
- compact rewrite 不会重复发送旧历史

## 16. 风险

- message-driven 后，失败不能再整体回滚已发送消息
- compact 从 append 特例变成 rewrite 特例，事务复杂度上升
- sqlite 需要从 snapshot 存储过渡到 normalized message log，迁移成本不小
- 旧测试大量默认 turn finalize 后才有正式历史，需要整体重写

## 17. 推荐结论

建议采用以下总原则推进这次重构：

- `Thread` 只保留一份正式消息真相
- 正式消息按 message 持久化与外发
- `turn` 只保留为日志、调试、审计和完成状态
- `compact` 作为唯一 rewrite 历史的特殊路径单独建模
- 当前范围内不引入 request-only runtime message

如果后续未来重新引入 memory recall 或其他 request-only runtime message，应作为新的独立设计议题处理，而不是在本次 message-driven 重构中保留半套兼容语义。

## 18. 本次 Diff 实际移除了什么

这一节不再讲“应该怎么做”，而是明确记录这次已经落地的删除项，方便协作者对照 diff 复现。

### 18.1 `Thread` 内移除的旧边界

本次在 `src/thread.rs` 里主要移除了这些旧设计：

- `ThreadCurrentTurn.working_messages`
- `ThreadCurrentTurn.history_override`
- `ThreadContext::load_messages()`
- `ThreadContext::system_prefix_messages()`
- `Thread::load_messages()`
- `Thread::system_prefix_messages()`
- `Thread::active_non_system_messages()`
- `Thread::current_turn_working_messages()`
- `Thread::push_turn_message()`
- `Thread::apply_turn_compaction()`

这些内容的共同点是：它们都在把正式消息拆成多份 view 或多段临时状态。

删除后的核心变化是：

- turn 不再持有“未提交消息缓冲区”
- turn 不再持有“本轮专用 history override”
- 线程里正式存在的消息，只剩 `Thread.messages()` 这一份真相
- compact 不再改一个 turn-local override，而是直接改写 thread 中 system 之后的正式历史

### 18.2 `SessionManager` 内移除的旧兼容读取层

本次在 `src/session.rs` 里主要移除了：

- `StoredThreadState`
- `SessionManager::load_thread_state()`
- 更早一轮已经移除的 `SessionManager::load_messages()`

这类接口的问题是：它们把同一个线程再次拆成：

- `thread_context`
- `messages`
- `loaded_toolsets`
- `tool_events`

这会让调用方继续沿着“Thread 只是原材料，外部再拼自己想要的视图”这条老路走下去。

删除后，`SessionManager` 对外只恢复完整 `Thread`，调用方自己从 `Thread.messages()` 和 `Thread.state` 读取。

### 18.3 `AgentLoop` 内移除的局部消息真相

本次在 `src/agent/agent_loop.rs` 里主要移除了这些 loop-local 兼容状态：

- `pending_turn_input_messages`
- UT probe 里的 `active_non_system_messages`
- UT probe 里的 `current_turn_working_messages`

删除后的含义很明确：

- 用户输入不再先放在 loop-local 临时数组里等下次提交
- `AgentLoop` 不再维护第二份“准备提交的消息”
- 测试观测点也只允许看 `thread.messages()`

### 18.4 这次没有删除、但语义已经变化的旧名字

下面这些接口名字还在，但职责已经变化了：

- `finalize_turn_success(...)`
- `finalize_turn_failure(...)`
- `store_turn(...)`
- `store_turn_state(...)`
- `commit_finalized_turn(...)`

它们现在不再承担“把一批 working messages 合并进正式历史”的职责，而是：

- finalize 只负责结束 turn、绑定 tool event、产出 turn 审计快照
- `store_turn*` 只是兼容入口，内部也改成逐条 append 正式消息
- `commit_finalized_turn(...)` 只负责 turn 元数据 / dedup 这条线，不再是主消息提交边界

## 19. 关键结构体现在长什么样

下面不是逐字段穷举源码，而是用于复现的“真实骨架”。

### 19.1 `ThreadCurrentTurn`

当前 `ThreadCurrentTurn` 可以理解成下面这样：

```rust
struct ThreadCurrentTurn {
    turn_id: Uuid,
    external_message_id: Option<String>,
    started_at: DateTime<Utc>,
    buffered_events: Vec<ThreadTurnEvent>,
    tool_events: Vec<ThreadToolEvent>,
}
```

重点变化：

- 它只保留 turn 元信息和事件缓冲
- 它不再持有任何正式消息
- 它不再持有 compaction 后的局部历史替身

### 19.2 `Thread`

当前 `Thread` 的核心形态可以理解成：

```rust
pub struct Thread {
    pub locator: ThreadContextLocator,
    pub thread: ThreadContext,
    pub state: ThreadState,
    revision: u64,
    pending_tool_events: Vec<ThreadToolEvent>,
    current_turn: Option<ThreadCurrentTurn>,
}

pub struct ThreadContext {
    pub messages: Vec<ChatMessage>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

重点变化：

- 正式消息只存在于 `ThreadContext.messages`
- `Thread.state` 只存 feature/tool/approval 这类非消息状态
- `current_turn` 只记录当前执行期的元信息，不再参与消息拼装

### 19.3 `AgentLoopOutput`

`AgentLoop` 现在不是只吐一个 turn 结果，而是同时吐：

```rust
pub struct AgentLoopOutput {
    pub reply: String,
    pub metadata: Value,
    pub messages: Vec<CompletedAgentMessage>,
    pub turns: Vec<CompletedAgentTurn>,
}

pub struct CompletedAgentMessage {
    pub turn_id: Uuid,
    pub message: ChatMessage,
    pub snapshot: Thread,
    pub dispatch_events: Vec<AgentDispatchEvent>,
}
```

重点变化：

- `messages` 是新主线，表示“本轮执行过程中每条已经正式提交的消息”
- 每条 `CompletedAgentMessage` 都携带提交后的 thread snapshot，供后续持久化
- `turns` 仍然存在，但退化成调试 / 审计 / dedup 完成态

### 19.4 `AgentWorkerEvent`

当前 worker 发给 router 的事件流可以理解成：

```rust
enum AgentWorkerEvent {
    ThreadContextSynced(SyncedThreadContext),
    MessageCommitted(CommittedAgentMessage),
    TurnFinalized(FinalizedAgentTurn),
    RequestCompleted(CompletedAgentRequest),
}
```

重点变化：

- `MessageCommitted` 成为主事件
- `TurnFinalized` 只负责 turn 收尾
- router 看到 `MessageCommitted` 就可以先落盘、再外发，不需要等 turn 结束

## 20. 当前新增/保留的关键函数，以及它们现在做什么

这一节只用自然语言描述职责，方便别人按相同语义重写。

### 20.1 `Thread` 相关

- `messages()`
  返回 thread 当前正式消息序列的完整拷贝。它不再拼接任何 turn-local working set，也不再读取 history override。

- `append_message(message)`
  要求当前线程已经 `begin_turn()`。函数只做一件事：把新正式消息直接 push 到 `ThreadContext.messages`，并更新时间戳。它不再往 `current_turn` 里的临时数组写。

- `replace_non_system_messages(replacement, updated_at)`
  保留开头连续 system message，把 system 之后的正式历史整体替换成 `replacement`。这是 compact rewrite 的基础原语。

- `replace_messages_after_compaction(compacted_messages)`
  先确认当前确实有 active turn，再调用 `replace_non_system_messages(...)` 改写历史。它不再改 turn-local override。

- `has_system_messages()`
  只检查 `messages vec` 的开头是否已经有 system prefix，用于初始化判定和冲突合并判定。

- `has_non_system_messages()`
  判断 system prefix 之后是否已经有正式历史，用于 runtime compact 判定。

- `adopt_system_messages_from(source)`
  当目标 thread 还没有 system prefix 时，从另一个 thread 的 `messages()` 前缀里复制 system message。这个函数主要用于 store 冲突恢复和旧数据兼容恢复。

- `begin_turn(external_message_id, started_at)`
  创建当前 turn 元信息，生成 turn id。它只打开 turn 生命周期，不再暗含“开始一段 working message 缓冲”。

- `current_turn_id()`
  读取当前 active turn id，主要给 worker fallback 路径和外部事件绑定使用。

- `finalize_turn_success(reply, completed_at)`
  关闭当前 turn，把 turn 内 buffered event 和 tool event 绑定到 turn 结果，并返回当前 thread 快照。它不再负责把 working message 合并进正式历史，因为消息在 append 时已经入库。

- `finalize_turn_failure(error, completed_at)`
  关闭失败 turn，但不会回滚已经 append 的正式消息。若 turn 内还没有 buffered event，会补一条 failure event 作为 turn 级审计输出。

- `store_turn(...)` / `store_turn_state(...)`
  这两个名字虽然还在，但内部已经变成兼容包装器：先 `begin_turn()`，再把传入消息逐条 `append_message()`，最后 `finalize_turn_success(...)`。

### 20.2 `AgentLoop` 相关

- `run_live_thread(...)`
  现在的主逻辑是：
  1. 先初始化 thread runtime。
  2. 在进入 ReAct loop 前执行一次 `begin_turn()`。
  3. 立即把用户消息 `append_message()` 到正式历史。
  4. 每次 LLM 生成 assistant message / tool-call assistant message / tool result message，都立即 append 到 thread。
  5. 每成功 append 一条正式消息，就产出一个 `CompletedAgentMessage`。
  6. 只有当最终 assistant 文本已经 append 完成后，才 `finalize_turn_success(...)`。

- `prepare_dispatch_event(event, reply_to_source)`
  把单个 loop event 转成 router 可以直接发送的单条 dispatch payload。这个函数的意义是把“按 turn 一次性准备 batch”改成“每条 message 自己带自己的 dispatch 结果”。

- `should_runtime_compact(thread_context, budget_report)`
  不再依赖 `active_non_system_messages()`，而是直接看 thread 是否已经存在 non-system 正式历史。

- `execute_turn_compaction(...)`
  直接把完整 `thread.messages()` 交给 compact manager。compact 完成后，调用 `replace_messages_after_compaction(...)` 改写正式历史。

### 20.3 `CompactManager` 相关

- `compact_messages(messages, compacted_at)`
  现在接收的是完整消息序列，而不是外部提前裁好的 active history。函数内部自己过滤掉开头 system message，只把 non-system 正式历史送去做 compact。也就是说，“过滤 system prefix”是 compact 内部细节，不再是公共消息视图 API。

### 20.4 `AgentWorker` 相关

- `initialize_thread(thread_context)`
  只在 thread 还没有 system prefix 时执行初始化。它会把 system prompt 和 feature prompt 一次性构造成稳定 system messages，并写入 thread。

- `build_committed_agent_message(locator, completed_message)`
  把 loop 内部的 `CompletedAgentMessage` 转成 router 能消费的 `CommittedAgentMessage`。转换后仍然保留：turn_id、正式 message、提交后的完整 thread snapshot、该 message 对应的 dispatch 事件。

- `spawn` 后的主处理流程
  worker 现在先把所有 `MessageCommitted` 事件顺序发给 router，再发 `TurnFinalized`，最后发 `RequestCompleted`。顺序不能反，否则 router 无法保证“先按 message 落盘，再收 turn 完成态”。

### 20.5 `Router` 相关

- `store_and_dispatch_committed_message(committed)`
  先调用 `SessionManager::store_thread_context(...)` 持久化这条 message 提交后的完整 thread snapshot；持久化成功后，再遍历 `dispatch_batch` 对外发送。它是“先落盘，后外发”的关键实现点。

- `store_completed_turn(turn)`
  只负责把 finalized turn 元数据写入 session/store，用于 turn 审计和 external message dedup。它不是正式聊天消息的主提交入口。

### 20.6 `SessionManager` 相关

- `load_thread_context(locator)`
  当前唯一推荐的线程恢复入口。恢复结果就是完整 `Thread`，调用方自己从中读取 `messages()` 和 `state`。

- `store_thread_context(locator, thread_context, updated_at)`
  把某个时刻的完整 thread snapshot 写回 store。当前实现依然是 snapshot 持久化，但调用语义已经服务于“每条 message 提交一次 snapshot”。

- `commit_finalized_turn(locator, finalized_turn)`
  把 turn 完成态和 dedup 绑定持久化。它保留是因为 turn 仍然承担审计和幂等绑定职责，不是因为 turn 还是消息提交边界。

## 21. 想让别人复现这次改造，建议按这个顺序做

### 21.1 先改 `Thread`

1. 删掉 turn 内任何正式消息缓存字段。
2. 让 `Thread.messages()` 直接返回唯一正式消息序列。
3. 增加 `append_message()`，要求 begin_turn 之后才能调用。
4. 增加 `replace_messages_after_compaction()`，让 compact 直接改正式历史。
5. 调整 finalize，让它不再做消息合并，只做 turn 收尾。

### 21.2 再改 `AgentLoop`

1. 删除 loop 内所有 `pending_*` / `*_working_messages` / `active_*` 临时变量。
2. 把 `begin_turn()` 挪到整个 loop 外，只调用一次。
3. 把用户输入放在 `begin_turn()` 之后、ReAct loop 之前，直接 append。
4. 每生成一条正式 assistant/tool-result message，就立刻 append。
5. 每 append 成功一条 message，就立刻产出一条 `CompletedAgentMessage`。
6. 最终 assistant 文本出现后，再 finalize turn。

### 21.3 再改 `Worker` 和 `Router`

1. worker 先把 `CompletedAgentMessage` 转成 `MessageCommitted`。
2. router 收到 `MessageCommitted` 后先持久化 thread snapshot。
3. 持久化成功后再发送这条 message 对应的 dispatch event。
4. 全部消息处理完后，worker 再发 `TurnFinalized`。

### 21.4 最后砍兼容读取接口

1. 删除 `load_messages()` 这类“只取 non-system history”的公共读取口。
2. 删除 `load_thread_state()` 这类“把 Thread 再拆成 messages/state”的公共读取口。
3. 若测试仍然需要这些断言形式，只允许在 `tests/support` 下做 test-only trait 包装，不能把兼容接口重新加回生产代码。

### 21.5 当前版本仍然保留的现实约束

为了避免协作者误判，这里把尚未继续推进的点也写明：

- store 目前仍然是 snapshot 持久化模型，还没有迁到 normalized message log
- `commit_finalized_turn(...)` 仍然存在，因为 turn 元数据和 dedup 仍要持久化
- `ensure_system_prefix_messages(...)` 仍然存在，因为初始化 system prompt 和 legacy 数据恢复还需要它

也就是说，这次已经做到的是：

- 消息真相统一到 `Thread.messages()`
- 主链路改成按 message 提交和外发
- turn 降级成日志 / 调试 / 审计 / dedup 元信息

但还没有做到的是：

- 底层 store 真正按 message 行式落盘
