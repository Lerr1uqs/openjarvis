## Context

当前实现的核心问题不是“`push_message(...)` 能不能落盘”，而是同一个 `Thread` 同时承担了三种彼此冲突的角色：

- 持久化快照；
- live 运行时对象；
- turn/request 生命周期容器。

因此当前代码才会出现：

- `Thread.push_message(...)` 内部还要通过 `persist_after_message_commit(...)` 间接回调外部持久化；
- 写盘前必须 `clone_for_persistence()` 剥离 runtime attachment 和 active turn，再在成功后 `restore_live_runtime_from(...)` 回填；
- 请求结束时还要再走一次 `SessionManager::commit_finalized_turn(...)`，把 dedup 和最终快照绑到 turn 结构上收尾。

这说明当前困难不在“message 级持久化不可行”，而在“持久化模型和 live 模型没有拆开”。只要继续让一个 `Thread` 结构同时承担快照和运行时对象，这类 attachment/snapshot 兼容层就会持续存在。

## Goals / Non-Goals

**Goals:**

- 让 `Thread.push_message(...)` 成为正式消息的唯一原子持久化边界。
- 让 thread 级 state 变更也遵守与消息相同的原子持久化语义。
- 让 `SessionManager` 只负责线程身份解析、handle 管理与热缓存，不再负责任何 turn/finalized snapshot 提交，也不再承担 dedup。
- 从运行时、持久化模型、store schema 和公共接口中移除非日志用途的 turn 结构体。
- 让 compact、tool audit、toolset load/unload、feature state 更新都直接落到 thread 的正式消息/状态模型，而不是依赖 turn finalize 收尾。
- 去掉当前 `attachment + snapshot compatibility` 这一层补丁式设计。
- 用独立的 `ThreadRuntime` 取代 `ThreadRuntimeAttachment`，承接线程的运行时服务访问。
- 为 `Feishu` 引入独立的内存 TTL deduper 层，把平台重复投递处理从 `Session/Thread` 主链路中彻底移走。

**Non-Goals:**

- 本次不保留旧 turn 持久化模型的双写兼容层。
- 本次不提供旧数据库到新 thread-first schema 的自动迁移兼容。
- 本次不尝试恢复“进程在单次请求中途崩溃时的执行进度”；只恢复已经正式持久化的消息和线程状态。
- 本次不提供跨重启、跨实例的 exactly-once dedup 保证。
- 本次不重做 channel 协议、LLM provider 协议或工具协议本身。
- 本次不在 `model/**` 架构文档中直接落地；先通过 openspec 固化实现契约。

## Decisions

### 1. `Thread` 改为 handle，持久化快照与 live-only 状态彻底拆开

新的主模型分为三层：

- `Thread`: 对外暴露的线程句柄，负责 `push_message(...)`、状态变更和请求期串行约束。
- `PersistedThreadSnapshot`: 仅包含可持久化字段，例如 `locator`、稳定 request context、正式消息序列、线程状态、revision。
- `ActiveRequestState`: 仅存在于内存中的请求期状态，例如当前外部消息 id、请求开始时间、临时工具审计缓冲和日志辅助字段。

`ActiveRequestState` 不得序列化、不得写入 store、不得作为跨请求 API 结构返回。这样 `Thread.push_message(...)` 就不再需要先剥离 runtime 字段再伪装成“纯快照”。

拒绝方案：

- 保留一个大而全的 `Thread`，继续通过 `clone_for_persistence()`/`restore_live_runtime_from(...)` 做剥离与回填。
  原因：这正是当前复杂度的来源，继续保留只会把 message-level 持久化做成更深的补丁层。

### 2. `SessionManager` 只保留线程解析与 handle registry，不再持久化 Session 聚合

`SessionManager` 保留的职责只有：

- 根据 `channel + user_id + external_thread_id` 解析稳定 `thread_key` / `thread_id`；
- 缓存 live thread handle；
- 在 cache miss 时从 `ThreadStore` 恢复 thread。

`SessionManager` 不再拥有以下职责：

- `commit_finalized_turn(...)`；
- `commit_finalized_turn_locked(...)`；
- turn 级最终 snapshot 提交；
- 持久化 Session 聚合元数据。

持久化层以 thread 为核心聚合，直接按 `thread_id` 或 `thread_key` 读写，不再要求先有 `session_metadata` 再有 `thread_metadata`。

拒绝方案：

- 继续保留持久化 `Session` 记录，仅把 `commit_finalized_turn(...)` 改名。
  原因：这样 `Session` 仍是事实上的持久化 owner，无法满足“Session 只是 Thread 的管理接口”这个目标。

### 3. `push_message(...)` 与 thread state mutator 共享同一个原子持久化内核

正式写入边界统一为 thread-owned mutation：

- `push_message(...)`：追加正式消息并持久化；
- `replace_messages_after_compaction(...)`：替换正式消息序列并持久化；
- `set_loaded_toolsets(...)` / `append_tool_event(...)` / `set_feature_state(...)`：更新正式线程状态并持久化。

统一算法：

1. 读取当前 `PersistedThreadSnapshot` 与 `revision`；
2. 构造下一个 snapshot；
3. 通过 `ThreadStore` 执行 compare-and-swap 持久化；
4. 仅当 store 写入成功后，更新内存中的 snapshot/revision；
5. 若写入失败，则内存态保持旧值。

这使“成功返回即已落盘”成为统一契约，而不是只对消息生效、对其他状态变更仍走额外提交路径。

拒绝方案：

- 只让 `push_message(...)` 立即落盘，其他 thread 状态继续等请求结束再统一提交。
  原因：compact、tool audit、toolset load/unload、feature override 仍会残留第二条提交路径，问题不会真正消失。

### 4. `ThreadRuntime` 承接全部运行时依赖，`ThreadRuntimeAttachment` 彻底删除

当前 `ThreadRuntimeAttachment` 的存在，只是为了把 `tool_registry`、`memory_repository`、`feature_prompt_rebuilder`、`message_persistence` 这类运行时对象硬挂到可持久化的 `Thread` 上。本次设计改为：

- 新增独立的 `ThreadRuntime`；
- `ThreadRuntime` 拥有线程创建期初始化、工具可见性计算、工具调用、memory 访问、feature prompt 重建等运行时能力；
- `AgentLoop` / worker 在执行时显式持有 `ThreadRuntime`，并用它驱动 live `Thread`；
- `Thread` 不再保存任何 attachment，也不再暴露 `attach_runtime(...)` / `detach_runtime()` 之类接口。

`Thread` 仍只负责正式消息与正式状态的原子写入；运行时能力一律由 `ThreadRuntime` 调用 thread-owned mutator 完成。

这里的“线程初始化”被进一步限定为“`SessionManager` 派生 thread handle 时的一次性正式消息注入”，而不是“agent loop 开始前的兜底补写”：

- 当 `SessionManager` 首次解析并派生某个 thread handle 时，`ThreadRuntime` 负责根据 feature、tool registry 和稳定 request context 生成初始化消息；
- 初始化消息直接通过 thread-owned mutation 写入正式消息序列，并在返回 live `Thread` 前完成持久化；
- worker 与 `AgentLoop` 只接收已经初始化完成的 live `Thread`，不再承担 `ensure_initialized()` 之类的补写职责；
- `request_context_initialized_at` 这类“是否完成初始化”的补丁字段直接删除，初始化完成状态只由已持久化的 system/feature message 前缀与线程状态表达。

拒绝方案：

- 保留 `ThreadRuntimeAttachment`，只是把名字改成别的。
  原因：只要运行时对象仍然挂在 `Thread` 内部，持久化模型和 live 模型的耦合就没有真正拆开。

### 5. 删除所有非日志用途的 turn 结构，改为 request-local 生命周期

结构化 turn 从主链路中全部移除：

- 不再公开 `ThreadCurrentTurn`；
- 不再公开 `ThreadFinalizedTurn`；
- 不再保留 `finalize_turn_success(...)` / `finalize_turn_failure(...)` 这类以 turn snapshot 为核心产物的接口；
- `AgentLoopOutput` 不再携带 `turns: Vec<...>`；
- worker/router 事件不再以 finalized turn 为载荷。

如果线程需要在单次请求期内记录“当前是谁触发的”“当前请求是否仍在执行”，这些都归入 `ActiveRequestState`，只用于日志、串行保护和临时运行时约束。

拒绝方案：

- 保留一个“轻量 turn struct”，声明它只是日志用途。
  原因：只要它仍是公共结构体或 store/schema 的显式概念，后续实现就会继续把提交、dedup、恢复、审计往它上面挂。

### 6. `FeishuMemoryDeduper` 独立承担入口去重，`Session/Thread` 完全不感知 dedup

dedup 要解决的是“平台重复投递同一 external message 时，系统是否要再次进入主链路”。这不是 `Session`、`Thread` 或 `ThreadStore` 的职责，而是平台入站入口的职责。本次设计选择：

- 新增 `FeishuMemoryDeduper`，位于 `FeishuChannel -> Router` 入口侧；
- key 使用 `channel + external_message_id`；
- 记录状态只保留 `Processing` / `Completed` 与过期时间；
- 第一条消息原子进入 `Processing`；
- 请求成功后标记 `Completed`；
- 请求失败时删除记录，让平台重试可以重新进入；
- 后台定期清理过期记录。

这层 dedup 只提供单进程、内存级 best-effort 去重，不与 `Session/Thread`、持久化快照、turn 或 store schema 产生任何耦合。

拒绝方案：

- 将 dedup 保留在 `SessionStore` 中，继续作为 thread 持久化的一部分。
  原因：这会重新把入口幂等问题耦合回 `Session/Thread` 主链路，违背本次“接口小、职责清晰”的目标。
- 完全删除 dedup。
  原因：在 Feishu 重复投递较常见的前提下，这会让系统过于频繁地重复回复和重复执行副作用。

### 7. compact 直接写回消息序列，不再产生 compacted turn

compact 输出改为一组正式消息，而不是 turn：

- compacted `assistant` message；
- follow-up `user` message（如仍保留“继续”语义）。

写回方式是直接替换 thread 中被 compact 的正式消息区间，并立即持久化。后续 compact 只面对普通消息序列，不再感知 compacted turn。

拒绝方案：

- 继续把 compact 产物包装成 turn，再在运行时摊平成消息。
  原因：这会把 turn 再次带回正式持久化模型，直接违背本次 change 的目标。

### 8. store schema 重写为 thread-first 模型，不保留运行时兼容层

新的持久化模型以 thread 为中心，至少包含：

- `thread_id` / `thread_key` / `channel` / `user_id` / `external_thread_id`；
- request context snapshot；
- 正式消息序列；
- thread state；
- revision；
- `created_at` / `updated_at`。

旧的 turn 持久化字段、turn 相关快照和 `Session` 聚合元数据不再是必须模型。若新 schema 与旧数据库冲突，直接删除旧数据库并按 thread-first schema 重建；不保留双写、兼容读取层或自动迁移脚本。

拒绝方案：

- 保留旧 schema，再在 runtime 上做一层转换兼容。
  原因：这会把“turn 只是历史包袱”永久保留在实现里，违背用户本次对彻底重写的要求。

## Risks / Trade-offs

- [Risk] `FeishuMemoryDeduper` 在进程重启、TTL 过期或未来多实例部署下会失去去重能力，同一消息可能再次进入主链路。
  Mitigation: 在模型文档和 spec 中显式声明这是 best-effort dedup；对有副作用的路径补充幂等性要求或接受重复执行风险，并补充 UT。

- [Risk] 删库重建会直接丢弃旧数据库中的历史线程数据。
  Mitigation: 明确这是本次重构接受的 breaking change；如需保留历史，重构前由开发者自行导出备份，但实现层不承担自动迁移。

- [Risk] 多个 live handle 同时改写同一线程会触发 revision conflict。
  Mitigation: 继续保留 per-thread live handle cache，并让 `ThreadStore` 以 CAS 语义拒绝旧版本覆盖。

- [Risk] 删除 turn 结构后，日志少了一个现成的聚合名词。
  Mitigation: 统一改用 `thread_id + external_message_id + request_started_at` 作为请求期日志锚点。

- [Risk] 重写范围跨越 `thread/session/worker/router/compact`，改动面大。
  Mitigation: 先完成 store 和 thread API 收口，再逐层切 worker/router，最后删除旧接口并补齐回归测试。

## Migration Plan

1. 引入新的 `ThreadStore` 与 thread-first 持久化 schema，支持 flat messages + thread state + revision。
2. 把 `Thread` 改写为 handle，内部拆分 `PersistedThreadSnapshot` 与 `ActiveRequestState`。
3. 让 `push_message(...)`、compact 写回、toolset/state 变更全部走统一的 thread-owned CAS 持久化入口。
4. 删除 `SessionManager::commit_finalized_turn(...)` 及所有 finalized turn 提交路径，把 `SessionManager` 收缩为线程解析和 handle cache。
5. 引入 `ThreadRuntime`，承接原 `ThreadRuntimeAttachment` 上的运行时职责，并把线程初始化前移到 thread 创建路径；删除 `request_context_initialized_at`、`ensure_initialized()` 与 attachment 接口。
6. 为 `Feishu` 入口增加内存 TTL deduper，并明确失败删除、成功完成和定期清理行为。
7. 重写 `AgentLoopOutput`、worker 事件和 router 协作，去掉 `ThreadFinalizedTurn` 载荷与 turn-based 收尾。
8. 删除或重建与新 schema 冲突的旧数据库，按 thread-first 模型初始化全新 store。
9. 删除旧的 attachment/snapshot 兼容层、turn 结构体和对应测试基建，补充新的 message-level UT 与集成测试。

## Open Questions

- 公开 API 是否直接保留名称 `Thread` 作为 handle，还是将持久化结构单独命名为 `ThreadSnapshot` 以减少歧义。
