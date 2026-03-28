## Context

当前 `SessionManager` 以进程内 `RwLock<HashMap<SessionKey, Session>>` 作为唯一存储，`ThreadContext` 虽然已经具备比较完整的可序列化结构，但系统启动时仍然总是创建全新的 session runtime。结果是：

- 服务重启后历史 `turns`、线程级 feature 状态、loaded toolsets 和 tool audit records 全部丢失
- `/auto-compact` 等线程命令修改的是内存状态，无法跨重启延续
- `ThreadContext` 已经被定义为线程事实来源，但事实仍然只存在内存里
- 后续若直接把 SQLite 细节写进 `SessionManager`，将来再切 PostgreSQL 会再次拆分一遍

同时，当前代码还存在几个约束：

- `ThreadContext / ThreadConversation / ThreadState` 已可序列化，适合直接作为持久化聚合根
- `ConversationTurn` 显式保存 `turn_id`、`external_message_id`、`started_at`、`completed_at` 和消息边界
- compact 产物是一个 turn，内部顺序是 `assistant summary + user continue`，无法可靠地从扁平 messages 逆推出通用 turn 边界
- `ToolRegistry`、`CompactRuntimeManager` 仍有兼容缓存，但这些缓存不应成为重启恢复的事实来源

这次设计的目标不是一次性完成 PostgreSQL 支持，而是先建立统一的 store 抽象和 SQLite 首版实现，让上层运行时以后可以无感切换不同数据库后端。

## Goals / Non-Goals

**Goals:**

- 定义统一的线程持久化 store trait，隔离 `SessionManager` 与数据库实现细节
- 以 `ThreadContext` 作为持久化聚合根，持久化线程 conversation 与 thread state
- 明确会话历史以 `Vec<ConversationTurn>` 保存，而不是只保存扁平 messages 再运行时反推 turn
- 首版提供 SQLite 持久化实现，使线程历史和线程状态可以跨进程重启恢复
- 让 `SessionManager` 升级为“内存缓存 + 持久化 store”的运行模型
- 为 `external_message_id` 相关幂等恢复预留持久化去重记录
- 为未来 PostgreSQL 实现保留兼容接口和数据语义

**Non-Goals:**

- 本次不要求首版同时实现 PostgreSQL store
- 本次不要求把 live browser session、Router 排队状态或其他运行时 attachment 直接持久化
- 本次不要求把 `ThreadContext` 完全拆成高度范式化的 message/turn/event 多表模型
- 本次不重新设计 Router 的线程排队语义
- 本次不要求一次性删除所有兼容缓存，只要求明确其从持久化快照重建

## Decisions

### 1. 以 `SessionStore` trait 抽象线程持久化边界，而不是让 `SessionManager` 直接依赖某个数据库

持久化层将定义统一的 store trait，由 `SessionManager` 通过该 trait 读写 session 与 thread 快照。抽象边界放在领域操作上，而不是放在 SQL 语句或某个 ORM 结果对象上。

首版 store 接口至少覆盖：

- 根据 `SessionKey` 解析或创建 session 元数据
- 根据 `ThreadLocator` 读取线程快照
- 保存最新的 `ThreadContext` 快照
- 读取或写入 `external_message_id` 对应的线程级去重记录
- 初始化 schema / 迁移版本

这样可以确保：

- `SessionManager` 不感知 SQLite 或 PostgreSQL 细节
- 测试仍然可以注入 memory store
- 后续增加 PostgreSQL 实现时只需要新增 store backend，而不是重写上层运行时

Alternative considered:

- 直接在 `SessionManager` 中内嵌 SQLite 访问逻辑。
  Rejected，因为会把持久化实现与线程运行时耦合在一起，未来切 PostgreSQL 或做测试替身时需要再次拆分。

### 2. 持久化聚合根固定为 `ThreadContext`，而不是扁平 message 列表

系统持久化时以 `ThreadContext` 为聚合根，其中至少包含：

- `locator`
- `conversation`
- `state`

会话历史按 `ThreadConversation.turns: Vec<ConversationTurn>` 保存，不采用“只存扁平 `Vec<ChatMessage>`，运行时再反推 turn”的方案。原因是：

- turn 具备独立业务语义：`turn_id`、`external_message_id`、开始/结束时间、审计绑定关系
- compact turn 的末尾是 follow-up `user` message，而不是“无工具调用 assistant”
- 未来审批、中断、失败和人工接续都可能产生无法靠消息形态稳定逆推的 turn 边界

因此，flattened messages 只作为运行时投影，不作为持久化真相。

Alternative considered:

- 只存扁平 messages，并用“turn 结束必然是无工具调用 assistant”去反推 turn。
  Rejected，因为该规则在 compact 设计下已经不成立，也无法稳定覆盖失败、中断和未来审批流程。

### 3. 首版后端使用 SQLite 线程快照存储，同时保留 PostgreSQL 可替换语义

首版持久化后端采用 SQLite，原因是：

- 单文件部署，适合当前单进程服务
- 支持事务，优于直接写 JSON 文件
- 和当前本地运行模型匹配，重启后恢复成本低

但设计上不把 SQLite 语义泄漏到上层。store trait 只表达统一的线程持久化语义，未来 PostgreSQL backend 必须提供相同的领域行为：

- 同样的 thread 定位规则
- 同样的 `ThreadContext` 快照结构
- 同样的去重/幂等语义
- 同样的 schema version/migration 管理入口

Alternative considered:

- 先用单个 JSON/YAML 文件保存整个 session map。
  Rejected，因为写入原子性、并发安全和后续迁移空间都较差，只适合临时调试，不适合作为正式线程持久化基础。

Alternative considered:

- 直接首版实现 PostgreSQL。
  Rejected，因为当前最紧迫的问题是先获得本地可恢复能力和稳定的抽象边界，不需要一开始就引入外部数据库部署成本。

### 4. `SessionManager` 采用“懒加载缓存 + write-through store”模型

`SessionManager` 将保留内存缓存，但缓存不再是唯一事实来源。新的运行模型为：

- 启动时初始化 store，但默认不全量预加载所有线程
- 当某个 thread 首次被命中时，如果 cache miss，则从 store 懒加载 `ThreadContext`
- thread 被修改后，`SessionManager` 先更新内存，再通过 store 写通保存
- 写入成功后，缓存中的最新快照继续作为该线程的热数据

这样可以避免：

- 启动时把所有线程一次性加载进内存
- 每次读写都直接命中数据库导致热点线程性能下降

Alternative considered:

- 启动时全量加载全部 sessions 与 threads。
  Rejected，因为后续线程数增长后，启动时间和内存占用都会放大，而且当前运行时并不需要一次性拥有全部线程热数据。

Alternative considered:

- 完全去掉内存缓存，所有操作都直连数据库。
  Rejected，因为当前 Router/AgentLoop 对线程快照的访问频繁，完全去缓存会拉高延迟并放大数据库访问次数。

### 5. 持久化快照只包含 declarative state，运行时 attachment 不落盘

持久化层只保存可以声明式恢复的线程状态，不保存纯运行时 attachment。首版明确不持久化：

- `pending_tool_events`
- Router 的 `pending_threads` / `queued_messages` / `seen_messages`
- `ToolRegistry` 的兼容线程缓存
- `CompactRuntimeManager` 的兼容 override map
- browser sidecar 进程、页面句柄、live session 目录等对象

恢复规则是：

- 先从 store 取回 `ThreadContext`
- 再由运行时基于 `ThreadContext` 重建工具可见性缓存、compact 兼容缓存和其他 live runtime state
- 兼容缓存只允许从 `ThreadContext` 单向重建，不允许反向覆盖已持久化快照

Alternative considered:

- 把 runtime cache 和 live object 一并持久化，力图实现“完全恢复现场”。
  Rejected，因为这会把不可序列化对象和高度易变状态引入持久化层，恢复复杂度与失败面都会显著增加。

### 6. 去重记录需要进入持久化层，保证重启后的幂等性

仅靠进程内 `seen_messages` 不能解决重启后的上游消息重投问题。持久化层需要至少保存线程级的 `external_message_id` 去重记录，使系统在重启后仍能识别：

- 某个外部消息是否已经落成 turn
- 某个 turn 是否已完成

这样可以避免服务重启后由于上游重复投递，导致重复追加 turn 或重复回复。

Alternative considered:

- 继续只依赖 Router 内存去重。
  Rejected，因为进程重启会直接清空该状态，无法满足“重启后仍能做到”的目标。

### 7. 线程快照写入需要带 revision 语义，避免旧快照覆盖新状态

当前系统已经存在这样的风险：命令路径和 Agent turn 完成路径都可能在不同时间持有同一个 thread 的快照，如果后写入的是旧版本，就会覆盖新状态。持久化方案需要给线程快照增加 revision 或等价版本戳，并让 store 在写入时执行 compare-and-swap 语义。

首版至少要满足：

- 线程快照读取时返回 revision
- 保存线程快照时校验 revision
- revision 冲突时返回明确错误或触发上层重读再合并

Alternative considered:

- 继续采用 last-write-wins。
  Rejected，因为这会让重启前后、命令路径和 Agent turn 路径之间的状态覆盖更隐蔽，问题从内存扩展到数据库后更难排查。

## Risks / Trade-offs

- [SQLite 快照模式查询灵活性有限] → 首版优先解决恢复和一致性；若后续需要复杂检索，可在 PostgreSQL 或后续 schema 中继续范式化
- [引入 revision 会增加写接口复杂度] → 用统一 store trait 收口版本控制逻辑，避免并发语义散落到上层
- [兼容缓存仍然存在一段时间] → 明确 `ThreadContext` 是唯一持久化事实来源，兼容缓存只允许从快照重建
- [SQLite 单写者特性可能限制吞吐] → 当前单进程、线程级串行模型下可接受；未来若升级到更高并发，再切 PostgreSQL backend
- [懒加载恢复会让首个命中线程多一次 store 读取] → 用内存 cache 保持热点线程命中率，避免全量预热的启动成本

## Migration Plan

1. 新增 session persistence 配置与 `SessionStore` trait，提供 memory store 与 SQLite store 两种实现。
2. 为 SQLite store 增加 schema 初始化、版本迁移和线程快照表/去重表。
3. 让 `SessionManager` 注入 store，并改为 cache miss 时懒加载 `ThreadContext`。
4. 将 `store_thread_context`、`store_turn_with_thread_context` 等写路径改为写通式保存。
5. 让 Router/AgentLoop/Command 在恢复线程后继续按现有 `ThreadContext` 路径运行，不直接读取数据库细节。
6. 将工具集恢复和 compact 兼容缓存调整为从 `ThreadContext` 重建，并移除重启后反向覆盖的路径。
7. 补充 UT 和重启恢复集成测试，覆盖 turn 边界、compact turn、loaded toolsets、`/auto-compact` 状态和去重恢复。

Rollback strategy:

- 若 SQLite store 集成出现风险，可先保留 `SessionStore` trait 与 memory store 实现，把默认 backend 临时切回 memory；这样不会破坏上层新的 store 抽象边界。

## Open Questions

- 首版配置默认是否直接切到 SQLite，还是保留 memory 默认并由配置显式开启；如果目标是“开箱即具备重启恢复”，则更倾向默认 SQLite
- revision 冲突的上层合并策略是直接报错重试，还是为特定线程命令提供自动重读合并
- SQLite 表结构首版是单表快照优先，还是提前把去重索引与 session/thread 元数据拆成多表；当前更倾向“元数据 + 快照 JSON + 去重表”的折中方案
