## ADDED Requirements

### Requirement: 系统 SHALL 使用统一的保序 conversation item 兼容层
系统 SHALL 在 provider SDK 之上维护一个统一的上层 conversation item 序列，而不是让 AgentLoop、Thread 持久化或测试直接依赖 OpenAI / Anthropic SDK 类型。该序列 SHALL 保留消息顺序，并至少能够表达以下语义：

- `system`
- `user`
- `assistant_output`
- `reasoning`
- `tool_call`
- `tool_result`

其中 `reasoning` SHALL 是可选 item；不提供显式 reasoning 的协议可以省略该 item，但一旦某协议返回 reasoning，兼容层 SHALL 能在不打乱相对顺序的前提下保存它。

#### Scenario: Responses 输出中的 reasoning 和 tool call 顺序被保留
- **WHEN** 某次模型输出先产生一个 `reasoning` item，再产生一个 `tool_call` item
- **THEN** 统一 conversation item 序列中会按相同顺序保存这两个 item
- **THEN** 后续续写时不会把它们重排为其他顺序

### Requirement: 统一 conversation item SHALL 支持跨协议的工具回合表达
系统 SHALL 允许同一套上层 conversation item 同时表达三种协议共有的工具回合语义：模型发起 `tool_call`、宿主执行工具、宿主回填 `tool_result`。兼容层 SHALL 让上层逻辑只依赖统一 `tool_call_id / tool name / arguments / tool result` 事实，而 SHALL NOT 直接依赖某一家协议的字段命名或对象形状。

#### Scenario: 上层工具循环不依赖供应商 SDK 类型
- **WHEN** AgentLoop 收到某个 provider 返回的工具调用结果
- **THEN** 上层逻辑看到的是统一的 `tool_call` item 与 `tool_result` item 语义
- **THEN** AgentLoop 不需要直接解析 OpenAI 或 Anthropic 的 SDK 原生对象

### Requirement: OpenAI Chat Completions 投影 SHALL 保持与现有工具消息批次兼容
当 active provider 使用 OpenAI Chat Completions 协议时，兼容层 SHALL 将统一 conversation item 投影成合法的 chat-completion message 序列。若上层 turn 中存在 assistant 文本与一个或多个 `tool_call` item，系统 SHALL 以确定性方式将它们组合为合法的 assistant + tool_calls payload；后续 `tool_result` item SHALL 投影为对应的 tool message。

#### Scenario: 连续 tool_call item 会被投影为单个 assistant tool_calls 批次
- **WHEN** 一个上层 turn 中包含 assistant 输出文本以及连续的多个 `tool_call` item
- **THEN** OpenAI Chat Completions 请求中会生成一个合法的 assistant message，并携带对应 `tool_calls`
- **THEN** 后续 `tool_result` item 会按 `tool_call_id` 投影成 tool message

### Requirement: Anthropic 与 OpenAI Responses 投影 SHALL 共享同一上层 conversation item 事实
当 active provider 使用 Anthropic 或 OpenAI Responses 协议时，兼容层 SHALL 继续消费同一套统一 conversation item 事实，并将其投影为各自协议所需的请求结构。协议差异 SHALL 被限制在 provider 适配层内部，而 SHALL NOT 让 `Thread`、`AgentLoop` 或配置层分叉出各自独立的消息模型。

#### Scenario: 切换协议不要求上层改写消息模型
- **WHEN** 用户把 active provider 从 `openai_chat_completions` 切换为 `openai_responses` 或 `anthropic`
- **THEN** AgentLoop 与线程层继续使用同一套统一 conversation item 结构
- **THEN** 协议差异只在 provider 适配层内部处理
