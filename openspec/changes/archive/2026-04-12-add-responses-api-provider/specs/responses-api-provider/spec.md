## ADDED Requirements

### Requirement: 系统 SHALL 提供基于 OpenAI Responses API 的 LLM provider
当某个 provider profile 的 `protocol = "openai_responses"` 时，系统 SHALL 走 OpenAI Responses API 协议分支，而不是继续复用 Chat Completions 请求路径。该 provider SHALL 接收统一 conversation item 与可见工具定义，并构造合法的 Responses API 请求。

#### Scenario: active provider 选择 openai_responses 时走 Responses 协议
- **WHEN** `llm.active_provider` 指向一个 `protocol = "openai_responses"` 的 provider profile
- **THEN** 系统会使用 OpenAI Responses API provider 发起请求
- **THEN** 不会误走 OpenAI Chat Completions 协议分支

### Requirement: Responses provider SHALL 保留 prior response output item 并用 `function_call_output` 续写
当 Responses provider 收到包含 `reasoning`、`function_call` 或其他可续写 output item 的模型返回时，系统 SHALL 将这些 output item 以统一 conversation item 形式保留在当前会话中。宿主执行工具后，系统 SHALL 追加与该 `call_id` 对应的 `tool_result` / `function_call_output` 事实，并在下一次 Responses 请求中把 prior response output item 与新的 tool output 一起作为 continuation input 发送。

#### Scenario: 按 PoC 方式把 prior output item 与 function_call_output 一起续写
- **WHEN** Responses provider 某一轮返回 `reasoning` item 与 `function_call` item，宿主随后为该调用生成对应的工具结果
- **THEN** 下一轮 Responses 请求输入中会同时包含前一轮的 prior response output item 与新的 `function_call_output`
- **THEN** 该 provider 可以继续完成同一次工具循环，而不是丢失前一轮的 reasoning / tool call 上下文

### Requirement: Responses provider SHALL 将模型输出归一化为统一 conversation item 顺序
Responses provider SHALL 以输出顺序遍历模型返回项，并将其归一化为统一 conversation item。若某次返回同时包含 `reasoning`、`function_call` 与最终 `assistant_output`，系统 SHALL 保持原始相对顺序；若返回中不存在最终 `assistant_output`，系统 SHALL 仍返回可继续执行工具循环的统一 item 列表，而不是把该轮视为失败。

#### Scenario: 只有 reasoning 和 function_call 的返回仍被视为合法中间轮次
- **WHEN** Responses provider 某次返回包含 `reasoning` 与 `function_call`，但还没有最终 assistant 输出文本
- **THEN** 系统会把该轮视为合法的中间工具调用轮次
- **THEN** AgentLoop 可以继续执行工具并发起下一次续写请求

#### Scenario: reasoning、function_call 和 assistant_output 同时出现时保持原顺序
- **WHEN** Responses provider 某次返回同时包含 `reasoning`、`function_call` 与最终 `assistant_output`
- **THEN** 统一 conversation item 列表会按模型原始输出顺序保留这些 item
- **THEN** 后续调试和持久化可以看到同一轮的真实生成顺序

### Requirement: Responses provider SHALL 与当前工具循环语义保持一致
Responses provider 的首版实现 SHALL 与当前 AgentLoop 工具循环语义保持一致：一轮 provider 返回可以携带多个 `tool_call`，宿主按统一工具执行路径处理它们，随后再继续下一轮请求；provider SHALL NOT 依赖并行工具执行或 provider 托管的自动工具运行才能得到正确结果。

#### Scenario: 多个 function_call 仍走当前宿主工具执行路径
- **WHEN** Responses provider 一次返回多个 `function_call` item
- **THEN** 系统会把这些调用归一化为统一 `tool_call` item 列表并交给当前宿主工具执行路径处理
- **THEN** provider 不要求引入新的并行工具调度模型才能完成本次轮次
