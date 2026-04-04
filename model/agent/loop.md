# AgentLoop

## 定位

- `AgentLoop` 是单轮 Agent 执行核心。
- 它只消费“已经初始化好的 `Thread` + 当前 incoming + event sender”。
- 它维护这一轮里的 `generate -> tool -> generate` 循环，直到模型不再请求工具。

## 严格边界

- 负责可见工具计算、工具执行、runtime compact、hook 触发、事件生成。
- 负责在 loop 内维护当前轮的临时 system messages 和 live chat messages。
- 不负责线程初始化，不负责 session 持久化，不负责 channel 出站。
- 不保存跨轮线程真相；跨轮状态必须回收到 `Thread`。

## 关键概念

- `RequestState`
  一次 generate 所需的消息、工具和预算快照。
- `AgentLoopEvent`
  Router 可直接回发的结构化事件。
- `AgentLoopOutput`
  单轮最终结果，包含 reply、events、commit messages、更新后的 `Thread` 和 tool 审计数据。
- `AutoCompactor`
  预算刷新后注入瞬时容量提示的组件，写入的是 loop 局部的瞬时 system messages。

## 主流程

1. 读取持久化 `Thread`
2. 准备 request-time runtime，例如 builtin tool 注册、可见工具计算、预算估算
3. 在 loop 内维护当前轮 transient system messages 和 live chat messages
4. 调用 `llm.generate(messages, tools)`
5. 如果模型返回文本，立刻发出 `text_output` 事件
6. 如果模型返回工具调用，逐个发出 `tool_call` 事件、执行工具、发出 `tool_result` 事件
7. 必要时触发 runtime compact，然后继续下一轮 generate
8. 当本次 generate 不再返回工具调用时结束，并把结果回收到 `Thread`

## compact 边界

- runtime compact 直接处理 message 序列。
- 输入范围是 `persisted non-system messages + pending live chat messages`。
- compact 完成后，loop 保留持久化 `System` 前缀，只替换非 `System` 历史。
- `compact` 工具只是这项 runtime 能力的暴露入口，不是另一套独立架构。

## 使用方式

- Worker 先完成 `init_thread()`，再把 `Thread` 交给 loop。
- Router 不负责拼 message 列表；主链路由 `AgentLoop` 在运行时临时导出请求消息。
- 当前轮 user message 进入 loop 后，由 loop 统一拼接最终请求消息。
- 只要某个线程级状态要跨轮保留，就应写回 `Thread` 再由 Session 层持久化。
