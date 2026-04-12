# Router

## 定位

- `ChannelRouter` 是系统总编排器。
- 它把 `channel`、`command`、`session`、`agent` 串成一条稳定处理链，但自己不做业务执行。

## 边界

- 负责收敛所有入站/出站事件、线程串行化、命令分流和结果转发。
- 负责接住 channel 侧已经完成基础预处理的入站消息，并在需要时挂接平台专用 dedup 层。
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
- `FeishuMemoryDeduper`
  `Feishu` 入口专用的内存 TTL 去重层，职责是挡住短时间重复投递，而不是提供跨重启一致性。

## 核心能力

- 接收 channel 入站消息，并在入口侧完成平台专用的 best-effort 去重。
- 先解析命令，再决定是否进入 Agent。
- 为每条消息解析 `Session/ThreadContext`。
- 把同线程消息串行送入 Agent。
- 在请求完成或失败后继续派发队列中的下一条消息。

## 验收标准

- Router 不把 dedup 状态写入 `Session/Thread`。
- 若 `FeishuMemoryDeduper` 因进程重启或 TTL 过期失效，Router 允许同一消息再次进入主链路；副作用重复风险由上层业务幂等或显式接受。

## 使用方式

- Router 是运行时主循环，不是某个组件内部的 helper。
- 新能力如果会改变消息流向，优先落在 Router；如果只是线程内部执行逻辑，不应该塞进 Router。
