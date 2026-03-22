# Context / Session / Thread

## 定义

当前项目里这三个概念分工如下：

### Session

`Session = channel + user_id`

表示某个用户在某个外部平台上的上下文空间。

例如：

- 飞书用户 A 在 `feishu`
- Telegram 用户 B 在 `telegram`

它们会各自拥有独立 session。

### Thread

`Thread = session 下的一条对话链`

来源优先级：

1. 外部平台原生 `thread_id`
2. 如果平台没有 thread 概念，先挂到 `default`

所以当前代码里：

- 飞书线程消息会复用飞书 thread_id
- 非线程消息先进入 `default`

### Turn

`Turn = 一轮 user -> assistant`

现在一轮里至少记录：

- 用户消息内容
- 外部消息 id
- assistant 回复内容
- 开始时间
- 完成时间

### Context

`Context = 当前送给 LLM 的消息组织结果`

当前结构：

`system + memory + chat`

其中：

- `system`
  - 系统提示词
- `memory`
  - 预留给长期/短期记忆
  - 当前还没接 memory provider
- `chat`
  - 当前 thread 里的 user / assistant 历史

## 当前代码位置

- [src/session.rs](F:/coding-workspace/openjarvis/src/session.rs)
  - `SessionManager`
  - `Session`
  - `PendingTurn`

- [src/thread.rs](F:/coding-workspace/openjarvis/src/thread.rs)
  - `ConversationThread`
  - `ConversationTurn`

- [src/context.rs](F:/coding-workspace/openjarvis/src/context.rs)
  - `MessageContext`
  - `ChatMessage`
  - `RenderedPrompt`

## 当前执行流程

1. channel 把外部消息转成 `IncomingMessage`
2. router 把消息转给 agent
3. agent 调 `SessionManager::begin_turn`
4. session manager 找到或创建对应 `Session`
5. session manager 找到或创建对应 `Thread`
6. 把当前用户消息追加成一个 pending turn
7. 用当前 thread 历史构造 `MessageContext`
8. worker 把 `MessageContext` 直接交给 `AgentLoop`
9. `AgentLoop` 内部再把 `MessageContext` 渲染成当前 LLM 需要的 prompt
10. LLM 返回后，agent 调 `SessionManager::complete_turn`
11. 把 assistant 回复写回对应 turn

## 为什么先这样实现

当前 `MessageContext` 内部已经是结构化消息：

- `system / memory / chat`
- provider 侧消费的是 `List[ChatMessage]`

所以 `Context` 当前先做了一层过渡：

- 内部仍然保留结构化 `system / memory / chat`
- 同时也能展开成给 provider 使用的消息列表

这样后面切到真正多 message 协议时，不需要推翻 session/thread 结构，只需要调整 `Context -> LLMRequest` 这层映射。

## 下一步建议

下一步比较合理的顺序是：

1. 给 `SessionManager` 增加持久化
2. 给 `Context` 接 memory 命中逻辑
3. 再接 command / tool / hook

原因是：

- session/thread 先稳定，后面的 tool 和记忆才有挂载位置
- context 先结构化，后续 provider 扩展不会反复改调用链
