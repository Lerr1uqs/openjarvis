## 1. Thread Dispatch 模型

- [ ] 1.1 在 `src/thread.rs` 中为当前 turn 引入稳定的 dispatch item / dispatch sequence 模型，支持从 `Thread` 导出“新提交且可外发”的 message / event
- [ ] 1.2 调整 `Thread.push_message(...)`、turn event 缓冲和 finalization 语义，使“消息 ownership”与“对外发送边界”解耦，但仍保持 `Thread` 是唯一 owner
- [ ] 1.3 为 `Thread` 增加 in-progress dispatch cursor / checkpoint 所需的数据结构，并保持与 finalized turn snapshot 分层

## 2. Message-Level 派发链路

- [ ] 2.1 重构 `src/agent/agent_loop.rs`，让每个 committed assistant text、tool event、tool result 和 terminal failure 都可以即时导出为 dispatch item
- [ ] 2.2 重构 `src/agent/worker.rs`，使 worker 不再等待 turn finalization 才上报用户可见结果，而是按 dispatch item 顺序转发
- [ ] 2.3 重构 `src/router.rs`，让 router 按 message / event 级别立即发送，同时不在 `Thread` 之外重新拼装消息

## 3. Checkpoint、恢复与 Dedup

- [ ] 3.1 调整 `src/session.rs` 与 `src/session/store/**`，为未完成 turn 保存 in-progress checkpoint 与 dispatch cursor
- [ ] 3.2 定义并实现恢复路径下的 dispatch 去重 / 跳过逻辑，避免已发送的 message / event 被盲目重发
- [ ] 3.3 保留 finalized turn 作为完成态与 external-message dedup 边界，并补齐 message-level dispatch 下的失败收尾语义

## 4. 回归验证

- [ ] 4.1 在 `tests/agent/agent_loop.rs` 中补充 UT，覆盖“同一 turn 内按 message / event 即时产出 dispatch item”的行为
- [ ] 4.2 在 `tests/agent/worker.rs`、`tests/router.rs` 中补充 UT，覆盖按顺序发送、失败追加 terminal failure、且不再等待 turn batch 的场景
- [ ] 4.3 在 `tests/session.rs` 与 store 相关测试中补充 UT，覆盖 in-progress checkpoint、恢复跳过已发送项和 finalized dedup 的场景
