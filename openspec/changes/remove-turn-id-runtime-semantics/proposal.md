## 背景

当前 `direct-message-channel-delivery` 已经把消息发送边界收敛到了 `thread.push_message(...)`，但代码里仍然残留一套 turn identity 语义：

- `ThreadToolEvent.turn_id`
- `ThreadCurrentTurn.turn_id`
- `ThreadFinalizedTurn.turn_id`
- `ExternalMessageDedupRecord.turn_id`
- `CommittedAgentDispatchItem.turn_id`
- `pending_tool_events` 这种“先缓存、后绑定 turn_id”的 buffer

这套模型的问题是：

- message 已经按 commit 逐条发送，`turn_id` 不再提供必要的发送语义；
- tool audit 仍然被要求等待 turn identity，导致出现 pending event buffer；
- thread push_message 每次持久化时还会把 active turn 一并写入 snapshot，和“turn 只属于执行期框架”的目标冲突；
- session/store/dedup 仍然带着 turn 级字段，边界没有真正收干净。

## 目标

- 删除 runtime、持久化、event payload 中所有 `turn_id` 字段。
- 删除 `pending_tool_events` 及其“先缓存后绑定”的 buffer 语义。
- 让 active turn 只保留本地执行期状态，不进入 thread 持久化快照。
- 让 tool audit event 直接以 message/调用事实写入，不再依赖 turn identity。
- 让 external message dedup 只记录“消息是否已完成处理”，不再记录 turn 关联。

## 非目标

- 本 change 不重命名 `open_turn(...)` / `finalize_turn_*` 这类生命周期接口。
- 本 change 不处理 `dispatch` 命名残留，只先移除 turn identity 语义。
- 本 change 不修改 `model/**` 架构文档。
