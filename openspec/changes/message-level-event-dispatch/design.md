## Context

当前系统继承了 `enforce-thread-owned-turn-boundaries` 的语义：`AgentLoop` 在一个 turn 内先把文本输出、tool 事件和 compact 事件缓冲到 turn 级 event batch，只有 turn finalization 完成后，Router 才统一外发并持久化对应 thread snapshot。这个模型保证了“用户看到的结果”和“最终落盘的 turn 状态”严格一致，但代价是中间文本、tool 结果和失败进展都必须等到 turn 结束后才能被用户看到。

现在需要把用户可见事件改成按 message / event 级别发送，但又不能退回到“Router 在 `Thread` 之外自己拼消息真相”的旧模式。因此这个 change 的核心不是简单把发送时机提前，而是要重新定义：

- 哪些 message / event 在何时算“可外发”
- 外发后的 thread 状态如何 checkpoint
- turn finalization 在 message 级发送模型下还承担什么职责
- 失败与恢复时，如何避免重复发送或不可解释的最终状态

## Goals / Non-Goals

**Goals:**

- 让文本输出、tool 事件和失败通知可以按 message / event 级别及时外发，而不是等到 turn 结束。
- 保持 `Thread` 作为当前 turn 消息和事件的唯一 owner，避免 Router 重新组装消息。
- 把“外发顺序”和“持久化 checkpoint”绑定到可重放的 message 级序号，保证恢复时可判定哪些事件已经发送。
- 保留 turn finalization，但将其职责收缩为“完成态、审计态和 dedup 提交边界”，而不是唯一外发边界。
- 为失败 turn 定义明确的 message 级收尾语义，避免已发出事件被隐式回滚。

**Non-Goals:**

- 本次不引入 token 级 streaming；粒度仍然是 message / structured event，不是半句文本流。
- 本次不修改 memory、toolset、compact 的业务语义，只调整它们对外发送的时机和边界。
- 本次不修改 `model/**` 架构文档；先通过 openspec 收敛语义。
- 本次不要求所有 channel 都支持“编辑上一条消息”；只定义按 message / event 顺序发送，而不是 UI 渲染形态。

## Decisions

### 1. 外发边界从 finalized turn 改为 committed message / event

系统将把“对外可见”的最小单位定义为 committed message / event，而不是 finalized turn。凡是已经进入 `Thread` 当前 turn state 且被标记为用户可见的文本输出、tool 调用记录、tool 结果或失败通知，都可以在 turn 未结束时按顺序外发。

这里的“committed”不是指 turn 已落盘完成，而是指：

- 事件已经被 `Thread` 接受并进入当前 turn 的正式状态；
- 该事件已经分配稳定的 turn-local 顺序号；
- 它不再只是 loop 的临时局部变量。

Alternative considered:

- 保持 finalized turn 作为唯一外发边界，只在 UI 层伪造“流式感”
  Rejected，因为用户仍然无法真正看到中间 tool 结果和失败进展，不能满足按 message 发送的目标。

### 2. `Thread` 继续是唯一消息 owner，但需要显式的 dispatch 序号/游标

`Thread` 仍然负责持有当前 turn working set，不允许 Router 或 Worker 在外部再维护第二份“待发送消息列表”。为了支持按 message 发送，`Thread` 需要额外为当前 turn 维护稳定的 dispatch 序号，例如：

- `dispatch_seq`
- `last_dispatched_seq`
- 或等价的 turn-local cursor

这样可以让外部模块通过 `Thread` 提供的 API 拿到“自上次发送之后新增的 dispatch items”，而不是自行 diff message 向量。

Alternative considered:

- Router 对比两次 thread snapshot 自行推导新增消息
  Rejected，因为这样会重新把消息真相拆回 `Thread` 之外，而且在 tool event / compact event 混合场景下不可靠。

### 3. 外发前必须先完成 in-progress checkpoint

message 级发送会打破“finalized turn 才持久化”的旧假设。为了避免“用户已经看到了消息，但进程崩溃后 thread 中完全没有对应状态”，系统需要在每次对外发送前先完成当前 in-progress turn 的 checkpoint，至少要保存：

- 当前 thread snapshot
- 当前 turn id
- 已提交 message / event 的 dispatch 序号
- 可选的 turn status（in_progress / finalized）

这不等同于把 dedup 提交提前到每条消息，而是为恢复和重放提供稳定锚点。

Alternative considered:

- 继续只在 turn finalization 后持久化，外发不做中间 checkpoint
  Rejected，因为一旦 mid-turn 崩溃，系统无法判断哪些外发事件已经被用户看到，也无法安全恢复。

### 4. turn finalization 保留，但不再是唯一外发边界

turn finalization 仍然保留以下职责：

- 产出 turn 的完成态（成功 / 失败）
- 绑定最终 reply / status / 审计结果
- 完成 external-message dedup 的正式提交
- 结束 in-progress turn checkpoint 生命周期

但它不再承担“把所有事件一起外发”的职责。换句话说：

- 外发按 message / event committed 边界进行
- dedup 与 turn 完成态仍按 finalization 边界进行

Alternative considered:

- 完全删除 turn finalization，让每条消息都独立提交
  Rejected，因为 external input 的去重、完整 turn 审计和失败完成态仍然需要明确的回合结束边界。

### 5. failed turn 不回滚已发送消息，而是追加终止事件

在按 message 发送的模型下，失败 turn 可能已经发送过部分文本或 tool 结果。系统不应尝试“撤回”这些事件，而应追加一个 terminal failure event / message 作为 turn 的结束信号，同时把最终 snapshot 标记为失败态。

这样可以保证：

- 已发送的中间进展仍然可解释
- 用户能明确知道 turn 在何处失败
- 最终持久化状态不会假装这些已发事件从未发生

Alternative considered:

- 失败时丢弃此前所有已发送中间结果，只保留最终失败消息
  Rejected，因为外发已经发生，系统无法保证所有 channel 都支持可靠撤回。

### 6. Session 需要区分“in-progress checkpoint”与“finalized turn commit”

Session/store 需要把 message 级外发引入的中间状态，与旧的 finalized turn 提交边界分开：

- in-progress checkpoint：服务于恢复、重放和 dispatch 去重
- finalized turn commit：服务于 external-message dedup 和完整 turn 提交

这意味着恢复路径不能再只拿“最后一个 finalized thread snapshot”，还需要能看见未完成 turn 的 checkpoint 与 dispatch cursor。

Alternative considered:

- 仅靠内存态 cursor，store 不保存中间发送进度
  Rejected，因为跨进程恢复时会丢失哪些事件已发送的事实。

## Risks / Trade-offs

- [Risk] message 级 checkpoint 会显著增加持久化频率
  Mitigation: 允许按单条 message 或小批 event 做合并 checkpoint，但对外语义仍表现为按 message 顺序发送。

- [Risk] 在“checkpoint 成功但外发失败”与“外发成功但 checkpoint 未提交”之间容易出现重复或缺失
  Mitigation: 为 dispatch item 引入稳定序号，并在 router/channel 层实现基于序号的幂等处理或重放跳过。

- [Risk] 失败 turn 的用户可见结果会与旧的 turn-batch 语义不同
  Mitigation: 将其定义为 BREAKING change，并补齐失败路径测试，确保“已发中间结果 + 终止失败消息”的行为可预测。

- [Risk] Router/Session/Thread 的边界再次耦合
  Mitigation: 严格要求 Router 只消费 `Thread` 导出的 dispatch items，不在外部重新组装消息。

## Migration Plan

1. 新增 message-level dispatch capability，并修改 `thread-context-runtime` 对 turn/finalization 的要求。
2. 在 `Thread` 上引入 dispatch item / dispatch cursor 模型，保留当前 turn working set ownership。
3. 调整 `AgentLoop`，让每次 committed message / event 都可立即导出为待外发 dispatch item。
4. 调整 `Worker` / `Router` / `Session`，先做 in-progress checkpoint，再按序发送 dispatch item。
5. 保留 turn finalization，用于完成态、dedup 和最终 turn 提交。
6. 更新失败恢复、dedup、router、worker、agent loop 等回归测试。

Rollback strategy:

- 如实现风险过高，可在实现阶段先引入 dispatch item / checkpoint 基础设施并保留旧的 turn-batch 发送策略，通过 feature flag 或兼容路径逐步切换。

## Open Questions

- 文本输出是否以“每条 assistant message”为最小外发单位，还是允许更细粒度的结构化文本事件？
- tool call / tool result 是否继续沿用现有 turn event 结构，还是需要新的统一 dispatch item schema？
- channel 若不支持真正的逐条中间事件展示，是否需要在 router 侧提供兼容降级策略？
