# AgentWorker

## 定位

- `AgentWorker` 是 Router 对接 Agent 的长生命周期执行体。
- 它负责消费 `AgentRequest`，但不自己实现 ReAct 细节。

## 边界

- 负责接收请求、装配执行输入、调用 `AgentLoop`、把结果回传给 Router。
- 不负责线程归属解析，不负责最终持久化落盘，不负责平台通信。

## 关键概念

- `AgentRequest`
  一次线程级执行请求，必须带上 `locator`、`incoming`、`ThreadContext`。
- `AgentWorkerHandle`
  Router 持有的 worker 通道句柄。
- `AgentWorkerEvent`
  Worker 回传事件，分为 `Dispatch / TurnCompleted / TurnFailed`。

## 核心能力

- 持有一个长生命周期 inbox。
- 在请求进入 Loop 前把兼容态线程状态并回 `ThreadContext`。
- 把 Agent 的流式事件转发给 Router。
- 在成功和失败两条路径上都返回完整线程执行结果。

## 使用方式

- Router 不直接调 `AgentLoop`，而是通过 `AgentWorkerHandle` 投递请求。
- 跨轮线程真相应留在 `ThreadContext`，不要放在 Worker 私有缓存里。
