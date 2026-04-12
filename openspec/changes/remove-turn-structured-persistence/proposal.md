## Why

当前主链路仍然把正式持久化边界拆成两段：消息先通过 `Thread.push_message(...)` 进入线程，再通过 `SessionManager::commit_finalized_turn(...)` 以 turn 结构补做最终提交。这导致 `attachment`、`snapshot` 兼容层和 turn-based 收尾逻辑长期存在，`Session/Thread` 边界持续混乱。现在需要彻底重写这条链路，让 `push_message(...)` 成为唯一正式持久化边界，并把非日志用途的 turn 结构从运行时、持久化和恢复模型中全部移除。

## What Changes

- **BREAKING** 删除非日志用途的 turn 主链路结构与接口，包括 `ThreadCurrentTurn`、`ThreadFinalizedTurn`、`finalize_turn_success(...)`、`finalize_turn_failure(...)`、`SessionManager::commit_finalized_turn(...)` 及其兼容提交路径。
- **BREAKING** 规定 `Thread.push_message(...)` 和其他 thread-owned 事实写入 API 是唯一正式持久化边界；调用成功返回时，对应消息或状态必须已经完成原子落盘。
- **BREAKING** `SessionManager` 退化为线程身份解析、thread handle 管理与热缓存边界，不再拥有“turn 最终提交”职责，也不再负责 dedup 或补做 thread snapshot 提交。
- **BREAKING** 引入独立的 `ThreadRuntime` 承接线程创建期初始化、工具可见性、工具调用、memory 访问和 feature prompt 重建职责；当 `SessionManager` 派生/创建 `Thread` handle 时，就要完成 feature 初始化消息注入与落盘，删除 `ThreadRuntimeAttachment` 这一类挂在 `Thread` 上的运行时补丁层。
- 重写 `Thread` 持久化模型：持久化快照只包含线程身份、稳定 `System` 前缀、正式消息历史、线程状态和 revision；不得再包含任何 turn working set、turn finalization 结果或 turn 持久化快照。
- 删除 `request_context_initialized_at`、`ensure_initialized()` 及同类“运行中补初始化”机制；初始化完成状态只通过已持久化的线程消息与线程状态表达。
- **BREAKING** 不保留旧数据库兼容层；若 thread-first schema 与旧库冲突，允许直接删除旧数据库并按新 schema 重建。
- 将当前请求期的临时执行状态从 turn 模型中拆出，改为 thread 内部的 request-local/live-only 状态，仅用于日志、当前请求串行约束和临时工具审计，不得形成持久化结构体契约。
- 重写 compact 写回语义：compact 直接改写线程消息序列，不再生成或依赖 compacted turn。
- 新增 `Feishu` 专用内存 TTL deduper 层，在 channel/router 入口对 `external_message_id` 做 best-effort 去重；该层与 `Session/Thread` 持久化完全解耦。
- 显式接受内存 dedup 的副作用边界：若进程重启、记录过期或未来多实例部署，同一 external message 仍可能再次进入主链路，因此副作用路径必须接受重复执行风险或自行实现幂等。

## Capabilities

### New Capabilities
- `message-atomic-thread-persistence`: 定义 thread 级消息/状态原子持久化、thread handle 与 thread store 的职责边界，以及“`push_message(...)` 成功即已落盘”的契约。
- `feishu-memory-dedup`: 定义 `Feishu` 入口的内存 TTL 去重层、过期清理、失败回收与重复副作用边界。

### Modified Capabilities
- `thread-context-runtime`: 线程运行时不再暴露或依赖非日志用途的 turn 结构；正式消息、线程状态与 live-only 请求状态边界需要重写。
- `chat-compact`: compact 不再写入 compacted turn，而是直接替换线程中的正式消息序列。
- `thread-managed-toolsets`: toolset 可见性刷新与工具审计语义改为基于当前线程与当前请求执行期状态，而不是 turn 结构。

## Impact

- Affected code: `src/thread.rs`、`src/session.rs`、`src/session/store/**`、`src/agent/agent_loop.rs`、`src/agent/worker.rs`、`src/router.rs`、`src/channels/feishu.rs`、`src/compact/**` 及对应测试。
- API impact: `Thread` 的写入/导出接口、`SessionManager` 的提交接口、worker/router 事件载荷、compact 写回模型都会发生 breaking change。
- Behavior impact: 正式消息一旦 `push_message(...)` 成功即成为已持久化事实；系统不再依赖 turn finalization 作为持久化节拍。`Feishu` dedup 只提供单进程、内存级 best-effort 去重，不保证跨重启或跨实例的 exactly-once。旧数据库若与新 schema 冲突，可直接清库重建，不承诺自动迁移。
