# Router

## 定位

- `ChannelRouter` 是系统总编排器。
- 它把 `channel`、`command`、`session`、`agent` 串成一条稳定处理链，但自己不做业务执行。

## 边界

- 负责收敛所有入站/出站事件、线程串行化、命令分流、结果持久化。
- 不负责平台协议细节，不负责 LLM 推理，不负责工具实现。

## 关键概念

- `ThreadLocator`
  Router 使用的线程身份结果，后续所有线程操作都靠它定位。
- `pending_threads`
  当前正在执行的线程集合，保证同一线程串行处理。
- `queued_messages`
  同线程排队消息缓存。
- `AgentWorkerEvent`
  Agent 回传给 Router 的流式事件和 turn 结果。

## 核心能力

- 接收 channel 入站消息并做去重。
- 先解析命令，再决定是否进入 Agent。
- 为每条消息解析 `Session/ThreadContext`。
- 把同线程消息串行送入 Agent。
- 在 turn 完成或失败后落盘并继续派发队列中的下一条消息。

## 使用方式

- Router 是运行时主循环，不是某个组件内部的 helper。
- 新能力如果会改变消息流向，优先落在 Router；如果只是线程内部执行逻辑，不应该塞进 Router。
