## 1. 数据模型与持久化边界

- [x] 1.1 删除 `Thread` 和 `Session` 中的 dispatch ledger、ack cursor、pending dispatch turn 等发送账本结构。
- [x] 1.2 明确当前 turn 内哪些对象属于正式消息历史，哪些只是执行期即时 event，并删掉它们对 dispatch 身份的依赖。
- [x] 1.3 为 `Thread` 引入抽象的 commit persistence attachment，使 `push_message(...)` 在成功返回前完成持久化。
- [x] 1.4 删除 `buffer_turn_event(...)`、`current_turn.dispatch`、finalize 兜底 failure event 等 turn event buffer 结构。

## 2. 运行时链路重构

- [x] 2.1 重构 thread/session 写路径，去掉整轮长持锁；即时持久化只保留在 `push_message(...)`，`begin_turn`/`finalize_turn` 只维护本地 turn 生命周期。
- [x] 2.2 重构 `AgentLoop`/`Worker`，收敛成 `agent.commit_message(...) = thread.push_message(...) + send event`。
- [x] 2.3 重构 `Router`，让其只负责消费 committed message event 并向 channel 发送消息，不再回写 thread/session。

## 3. 恢复与完成态

- [x] 3.1 删除中断后的 dispatch 恢复/重放路径，确保后续请求只依赖已持久化的消息上下文继续执行。
- [x] 3.2 调整失败收尾逻辑，确保用户可见 failure message 只能由 agent 在 finalize 前显式 commit，而不是由 finalize 内部兜底生成。
- [x] 3.3 清理旧的 dispatch 兼容入口、日志和结构体，确保 turn finalization 只承担完成态、审计和 dedup 职责。

## 4. 验证与回归测试

- [x] 4.1 在 `tests/thread.rs`、`tests/session.rs` 中补充“thread 不再保存发送进度”“commit 边界短持锁”的 UT。
- [x] 4.2 在 `tests/agent/worker.rs`、`tests/router.rs` 中补充“agent 在 push_message 成功后发事件”“router 不改 thread/session”“消息 commit 后逐条发送”的 UT。
- [x] 4.3 删除或改写旧的 dispatch ledger 相关测试，确保回归覆盖失败 turn、无自动恢复、即时 event 与正式消息分层等边界情况。
