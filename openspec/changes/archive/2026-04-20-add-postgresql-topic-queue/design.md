## Context

当前普通消息的主链路是：

`Channel -> ChannelRouter -> SessionManager.create_thread/lock_thread -> AgentWorker(mpsc request queue) -> AgentLoop`

其中真正承担“同线程串行”和“消息排队”的，不是数据库事实，而是：

- `ChannelRouter.pending_threads`
- `ChannelRouter.queued_messages`
- `AgentWorkerHandle.request_tx`

这带来三个直接问题：

- 普通消息没有 durable queue，进程重启后内存队列丢失
- 没有正式的 domain worker lease，无法严谨表达“某个 thread_key 当前由谁处理”
- Router、Worker 和线程串行事实耦合在一起，不利于后续继续演化

同时，当前项目已经明确了另一条边界：thread 正式事实由 `SessionManager + SessionStore(sqlite/memory)` 管理，`Thread` 自己通过 CAS 持久化消息和状态；这部分不应该被 PostgreSQL queue 接管。

## Goals / Non-Goals

**Goals:**

- 为普通消息引入独立的 PostgreSQL topic queue，提供原子 `add / claim / complete` 传递语义。
- 保持 command 继续在 queue 前拦截和执行，不改变当前命令边界。
- 让 router 在普通消息入队前先解析 `ThreadLocator`，并仅在目标 thread 当前进程尚未加载时执行线程准备动作，再把 `ThreadLocator + IncomingMessage` 作为 queue payload 入库。
- 让 `AgentWorker` 收敛为按 `thread_key` 懒创建的 domain worker，并以 worker lease 保证同一 domain 同时只有一个活跃 worker。
- 通过过期清理与日志恢复 stranded worker/message，使系统具备最小可接受的 crash recovery。
- 明确 queue 与 thread sqlite 的职责边界，不做跨库联合事务。

**Non-Goals:**

- 不把 thread sqlite 快照、thread message history、LLM/tool 执行过程迁入 PostgreSQL queue。
- 不让 queue 负责 outbound message 分发；现有 worker -> router dispatch 回传路径暂时继续保留。
- 不把 command 改成 queue message。
- 不把 worker 改成全局常驻 topic poller。
- 不在本次变更里解决跨库 crash window 带来的“精确一次”问题；本次显式接受至少一次交付语义。

## Decisions

### 1. Queue 只拥有“传递事实”，不拥有 thread 正式事实

PostgreSQL queue 只保存：

- queue message payload
- queue message delivery state
- worker lease / heartbeat / expire

它不保存也不协同这些事实：

- thread sqlite snapshot
- thread message history
- thread state / revision
- tool / LLM 运行时状态

这样可以保持边界稳定：queue 只回答“消息是否已入队、谁拿走了、是否完成”，thread sqlite 继续回答“线程正式事实是什么”。

备选方案：

- 把 thread snapshot 也放进 PostgreSQL queue 库。  
  Rejected，因为这会把消息传递边界和 thread 正式事实边界重新耦合。
- 做 PostgreSQL queue 和 thread sqlite 的跨库联合事务。  
  Rejected，因为复杂度过高，且不符合当前需求收敛方向。

### 2. Router 对普通消息采用“先解析 locator，按需准备 thread，再入队”的顺序

普通消息进入 router 后，先完成：

- 解析稳定 `ThreadLocator`
- 检查当前进程是否已经加载该 thread
- 仅在未加载时执行 `SessionManager.create_thread(...)` 或等价准备动作

再执行 queue 入库。queue payload 只保存：

- `ThreadLocator`
- `IncomingMessage`

不保存 live thread handle。

这样做的原因是：

- queue message 一旦入库，就能保证对应 thread identity 已解析完成，且未加载 thread 已被提前准备
- 同进程内 worker 后续通过 `SessionManager.lock_thread(...)` 会命中已有 cache handle，不需要额外传递 live handle
- 已加载 thread 不需要重复走 create 路径，避免放大 create 的初始化副作用
- 不会把内存锁对象耦合进 queue payload

代价是如果“未加载 thread 的准备动作”成功而 `queue.add(...)` 失败，可能留下一个已加载但未入队的 thread；这一点作为已知 trade-off 接受。

备选方案：

- 先 queue.add，再由 worker 首次 claim 时创建 thread。  
  Rejected，因为会让“消息已入队但 thread 尚未准备好”的失败路径更难收口。
- 把 `Arc<Mutex<Thread>>` 直接作为 queue payload 传递。  
  Rejected，因为 queue payload 必须是 durable 事实，不能依赖进程内对象。

### 3. Domain 固定使用 `thread_key`，而不是 `channel:user`

worker domain 统一使用 `thread_key`。

原因：

- 当前系统的串行粒度本来就是 thread
- `thread_key` 已经是稳定归一化键
- 同一个 user 下不同 external thread 不应被强行串行化

备选方案：

- 一个 worker 对应一个 `channel:user`。  
  Rejected，因为串行粒度过粗，会把本来独立的 thread 压成一条线。

### 4. `queue_message` 与 `queue_worker` 分表建模

本次最小模型使用两张表：

- `queue_message`
  - `message_id`
  - `topic`
  - `payload_json`
  - `status`
  - `claim_token`
  - `leased_until`
  - `created_at`
  - `claimed_at`
  - `completed_at`
- `queue_worker`
  - `worker_id`
  - `domain`
  - `lease_token`
  - `lease_expires_at`
  - `last_heartbeat_at`
  - `started_at`
  - `stopped_at`

其中：

- `queue_message` 只表达消息传递状态，最小状态机为 `pending -> active -> complete`
- `queue_worker` 只表达 domain worker 协调事实

只用 message 表而不建 worker 表无法可靠表达“同一 domain 同时只有一个活跃 worker”，因此 worker 表是必须的。

### 5. `AgentWorker` 改成按 domain 懒创建的 worker task

`AgentWorker` 不再消费全局 `mpsc::Receiver<AgentRequest>`。新的工作方式是：

- router 在普通消息成功入队后执行 `ensure_worker(thread_key)`
- 若当前 domain 没有活跃 lease，则启动一个对应 domain 的 tokio worker task
- worker 仅 claim 自己 domain 下的消息
- 没有待处理消息并达到空闲超时后主动释放 lease 并退出

这个改动替换的是“request queue 机制”，不是整个 worker 事件语义。当前 worker 对 router 的 dispatch 回传路径可以先继续保留。

备选方案：

- 一个全局常驻 worker 扫描所有 topic。  
  Rejected，因为 topic 数量无界，且不符合“来消息再建 worker”的目标。
- 继续保留 router 内存排队，再只把消息镜像写进 PG。  
  Rejected，因为 durable queue 就不再是事实来源。

### 6. Queue 事务边界使用 raw SQL transaction engine

本次 queue 模块会引入一层专用的 raw SQL transaction engine，用来统一承载：

- `add`
- `claim`
- `complete`
- worker lease acquire / heartbeat / release / reap

上层 queue repository 只依赖事务闭包和 SQL 执行入口，不在业务代码里散落 `BEGIN/COMMIT/ROLLBACK`。

这里的 engine 只服务 queue 模块，不扩展成整个仓库的通用事务基础设施。

备选方案：

- 直接在每个 repository 方法里手写事务控制。  
  Rejected，因为 lease 与 message 状态更新很容易分散失控。
- 先引入 ORM，再在 ORM 上包事务。  
  Rejected，因为当前需求更适合明确 raw SQL。

### 7. 完成语义按“传递结束”定义，不按“业务成功”定义

queue 的 `complete` 语义不是“Agent 一定成功回答了”，而是“这条 queue message 的传递处理已经结束，不再需要继续投递”。

因此：

- 如果 AgentLoop 正常结束，worker 会 `complete`
- 如果 AgentLoop 进入已知失败路径，但 worker 已经把错误回复写入 thread 并完成本轮收口，也会 `complete`
- 只有 worker 崩溃、租约超时或未完成收口的异常中断，才走 lease expiry recovery

这样 queue 就不需要知道 thread 业务成功/失败细节。

## Risks / Trade-offs

- [Risk] 未加载 thread 的准备动作成功但 `queue.add(...)` 失败，会留下一个已加载但未入队的 thread。  
  → Mitigation: 只在未加载路径执行准备动作，并通过日志记录 queue add 失败；不把它扩展成跨库事务问题。

- [Risk] worker 在 thread 已经写入正式事实后、`queue.complete(...)` 前崩溃，恢复后消息可能被再次处理。  
  → Mitigation: 显式接受至少一次交付语义，并在 spec 中写明该边界；后续如果需要更强幂等，再单开变更收敛。

- [Risk] 只做 message 表、不做 worker 表会导致同一 domain 被重复 spawn。  
  → Mitigation: 明确引入 `queue_worker` lease 表作为 domain 级协调事实来源。

- [Risk] lease 清理不彻底会导致 active message 永久卡住。  
  → Mitigation: worker 定期 heartbeat，清理器同时回收过期 lease 与 stranded active message，并打结构化日志。

- [Risk] 一次性移除 router 内存队列和 worker request mpsc，集成回归面较大。  
  → Mitigation: 先保持现有 dispatch event 回传路径不动，把本次改动范围收敛在 inbound queue 与 domain worker 生命周期。

## Migration Plan

1. 新增 PostgreSQL queue 配置、schema 初始化与 queue 模块。
2. 实现 `queue_message + queue_worker`、事务引擎、lease 清理器与最小日志。
3. 将 router 普通消息路径改成“解析 locator -> 若未加载则准备 thread -> queue.add -> ensure_worker”，command 路径保持不变。
4. 将 `AgentWorker` 改成 domain worker task，移除 request mpsc 消费路径。
5. 删除 router 内存排队状态与相关测试，补齐 queue/worker/router 新用例。

## Open Questions

- 首版 queue 配置是挂在 `session` 下还是新增独立 `queue` 配置节，更贴合当前配置模型？
- worker 空闲超时与 lease TTL 是否要分开配置，还是首版统一成一组简单超时参数？
