# Agent

## 定位

- `agent` 是系统执行层。
- 它拿到已经解析好的线程和上下文后，负责把这一轮任务真正跑完。

## 边界

- 负责单轮 ReAct、工具调用、hook 触发、compact 接入、事件回传。
- 不负责平台协议，不负责线程归属解析，不负责最终持久化策略。

## 关键概念

- `AgentWorker`
  长生命周期执行体，负责排队消费 `AgentRequest`。
- `AgentLoop`
  单轮执行核心，维护本轮 `generate -> tool -> generate` 循环。
- `AgentRuntime`
  共享能力容器，持有 hooks、tools、compact runtime。
- `AgentDispatchEvent`
  Agent 侧流式事件，Router 用它把文本、工具事件、compact 事件回发给上游。

## 核心能力

- 在单轮内持续维护 working chat，而不是只做一次问答。
- 对同一 `ThreadContext` 进行原地读写，保证线程状态跟随执行结果更新。
- 在需要时自动 compact，或在模型主动请求时执行 compact。
- 把文本输出、工具调用、工具结果、compact 结果实时回传给 Router。

## 使用方式

- Router 传入 `AgentRequest` 时必须同时提供 `ThreadContext`。
- 如果一个能力需要跨轮保留线程状态，应写进 `ThreadContext`，而不是塞进 Worker 私有字段。

## 继续阅读

- `agent/worker.md`
- `agent/loop.md`
- `agent/runtime.md`
- `agent/hook.md`
- `agent/tool.md`
