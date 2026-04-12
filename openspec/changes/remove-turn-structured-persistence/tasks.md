## 1. Store 与持久化模型重写

- [x] 1.1 设计并实现 thread-first 的持久化快照模型，只保留 request context、flat message 序列、thread state、revision 与必要定位字段。
- [x] 1.2 移除 `Session` 聚合对持久化的前置依赖，让 store 可以直接按稳定 thread identity 读写线程。
- [x] 1.3 从 `Session/ThreadStore` 中移除平台入口 dedup 的持久化职责与相关 schema 字段，保持 `Session/Thread` 接口精炼。
- [x] 1.4 为 memory/sqlite store 补充 compare-and-swap revision 写入与冲突错误覆盖测试。

## 2. Thread API 与运行时模型重写

- [x] 2.1 将 `Thread` 改写为 live handle，并在内部拆分 `PersistedThreadSnapshot` 与 live-only 请求期状态。
- [x] 2.2 让 `push_message(...)` 成为正式消息的唯一原子持久化边界，删除 `persist_after_message_commit(...)`、`clone_for_persistence(...)`、`restore_live_runtime_from(...)` 这一类兼容层。
- [x] 2.3 为 toolset、tool audit、feature state、compact 写回提供与消息相同的 thread-owned 原子持久化 mutator。
- [x] 2.4 删除非日志用途的 turn 结构体与相关 API，包括 `ThreadCurrentTurn`、`ThreadFinalizedTurn`、`finalize_turn_success(...)`、`finalize_turn_failure(...)`。
- [x] 2.5 引入独立的 `ThreadRuntime`，承接原 `ThreadRuntimeAttachment` 上的线程创建期初始化、工具、memory 和 feature prompt 运行时职责，并删除 attachment 接口。
- [x] 2.6 把线程初始化前移到 `SessionManager` 派生 thread 的路径：由 `ThreadRuntime` 生成并落盘初始化消息，删除 `request_context_initialized_at`、`ensure_initialized()` 及同类运行中补初始化机制。

## 3. Session / Worker / Router 主链路收口

- [x] 3.1 收缩 `SessionManager` 为线程解析与 handle cache 边界，删除 `commit_finalized_turn(...)` 及其锁内版本，并移除 dedup 相关接口。
- [x] 3.1.1 让 `SessionManager` 在首次派生 thread handle 时完成线程初始化并返回已初始化线程，而不是把初始化延后到 worker/AgentLoop。
- [x] 3.2 重写 `AgentLoopOutput`、worker 事件和 router 协作协议，去掉 finalized turn 载荷与 turn-based 收尾逻辑。
- [x] 3.3 让 worker 成功/失败路径只依赖 thread-owned message/state 提交与 request completion，不再补做 turn snapshot 提交，也不再在进入 AgentLoop 前调用 `ensure_initialized()`。
- [x] 3.4 更新命令路径，确保命令对线程的修改也只通过 thread-owned 原子持久化接口完成。
- [x] 3.5 为 `Feishu` 入口增加独立的内存 TTL deduper，覆盖 `Processing/Completed` 状态、失败删除和定期清理。

## 4. Compact 与 Tool Runtime 适配

- [x] 4.1 重写 compact 写回逻辑，使 compact 结果直接替换线程正式消息序列，而不是生成 compacted turn。
- [x] 4.2 重写 tool audit 记录路径，确保工具审计在记录成功后就进入正式线程状态，而不是依赖 turn finalize。
- [x] 4.3 更新 toolset 可见性刷新逻辑与运行时恢复逻辑，确保其基于当前线程与当前请求期状态工作，而不暴露 turn 结构。

## 5. 测试与清理

- [x] 5.1 重写 `thread/session/worker/router/compact` UT，覆盖“`push_message(...)` 成功即已落盘”的主契约与边界情况。
- [x] 5.2 补充 `Feishu` dedup、TTL 过期、进程重启后重复处理、副作用重复风险提示、revision conflict、compact 写回、tool audit 持久化的集成测试。
- [x] 5.3 删除旧的 turn-based 测试辅助、兼容提交接口和冗余 snapshot attachment 代码。
- [x] 5.3.1 删除旧数据库兼容假设；测试与开发环境按 thread-first schema 直接建库，必要时直接清理旧数据库文件。
- [x] 5.4 在实现完成后补充必要的调试日志检查，确保线程解析、消息落盘、`Feishu` dedup 命中/过期/清理与 compact 写回都有可追踪日志。
