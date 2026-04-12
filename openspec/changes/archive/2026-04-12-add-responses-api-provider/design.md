## Context

当前 LLM 主链路有三个明显限制：

1. `src/config.rs` 里的 `LLMConfig` 还是单 provider 平铺结构，`build_provider(...)` 也只会基于这一份配置选出一个协议分支。
2. `src/llm.rs` 的 provider 边界仍然是 `LLMRequest { messages, tools } -> LLMResponse { message, tool_calls }`，它默认一轮输出最多只有“一段 assistant 文本 + 一组 tool calls”，无法表达 Responses API 里的 `reasoning -> function_call -> function_call_output -> assistant_output` 保序链路。
3. 当前 `ChatMessage` / `ChatToolCall` 只能较好承接 Chat Completions 语义，无法完整保留 Responses 所需的 `reasoning item id`、`function_call item id`、`call_id` 这类续写事实。

同时，这次 change 是跨模块的：

- 配置层要支持多 provider 与 active provider 选择
- `llm` 层要新增 Responses provider，并重写统一 provider contract
- `agent_loop` 要从“读 message + tool_calls”改成“按顺序消费 provider items”
- `thread` / `context` / compact 需要接受新的消息兼容层事实

因此这次 change 不能只是在 `OpenaiProvider` 旁边多加一个 HTTP 调用函数，而是要把“LLM 配置解析 + 上层消息兼容层 + provider 适配器”三者一起收敛。

## Goals / Non-Goals

**Goals:**

- 在配置层支持多个具名 provider profile，并通过 `llm.active_provider` 解析当前生效 provider。
- 保持旧单 provider `llm` 配置可用，但在运行时统一归一化为“active provider + provider profiles”视图。
- 把 `ChatMessage` 提升为跨协议兼容层，使其能够承接 `anthropic`、`openai_responses`、`openai_chat_completions` 三种协议的上层语义。
- 将 provider 返回值改成保序 item 序列，让 Responses API 可以无损表达 reasoning / tool call / tool result continuation。
- 保持当前宿主工具循环语义不变，不要求引入 provider 托管自动工具执行或并行 tool scheduler。

**Non-Goals:**

- 本次不修改 `model/**` 架构文档；先通过 OpenSpec 收敛行为与实现方案。
- 本次不引入配置热重载，也不支持运行时动态切换 active provider。
- 本次不实现 provider streaming、增量 token 事件或多模态输入输出。
- 本次不重做 channel 对外展示协议；reasoning 首版主要用于续写、日志与调试，而不是直接对外发送。
- 本次不把 Anthropic 扩展成完整的新功能面；重点是让它进入同一兼容层和协议投影边界。

## Decisions

### 1. 保留 `LLMConfig` 作为顶层入口，但新增 provider profile 与 resolved 视图

`LLMConfig` 不会被拆成新的顶层配置段。相反，它会演进为“两种输入模式 + 一种统一解析结果”：

- 旧模式：继续接受当前单 provider 平铺字段
- 新模式：支持 `active_provider + providers`
- 统一结果：`ResolvedLLMProviderConfig`

推荐的新增结构如下：

```yaml
llm:
  active_provider: dashscope-responses
  providers:
    dashscope-responses:
      protocol: openai_responses
      model: qwen3.6-plus
      base_url: https://dashscope.aliyuncs.com/compatible-mode/v1
      api_key_path: ~/.dashscope.apikey
      headers:
        X-CHJ-GWToken: ${CHJ_GWTOKEN}
    anthropic-prod:
      protocol: anthropic
      model: claude-sonnet-4-5
      base_url: https://api.anthropic.com
      api_key_path: ~/.anthropic.key
```

实现上会新增：

- `LLMProviderProfileConfig`
- `ResolvedLLMProviderConfig`
- `LLMConfig::resolve_active_provider()`
- `LLMConfig::resolve_all_providers()`

这样 `build_provider(...)`、`build_provider_from_global_config()`、`main.rs` 日志和预算读取都只面对 resolved 视图，而不是在多个调用点各自拼规则。

Rejected alternatives:

- 方案 A：直接把旧 `llm.protocol/model/...` 删除，强制所有用户立刻迁移到 `providers`
  Rejected，因为当前仓库已有大量 builder、示例和测试基于旧结构，立即 breaking 没必要。

- 方案 B：使用 `Vec<ProviderProfile>` 而不是 `HashMap<String, ProviderProfile>`
  Rejected，因为 active provider 本质是“按名字选择 profile”，map 更符合查找和校验语义。

### 2. 直接提升 `ChatMessage`，而不是再引入第二套独立 conversation item 模型

当前 `Thread`、compact、预算估算、feature prompt、AgentLoop 都已经深度依赖 `ChatMessage`。如果再引入一套新的 `LLMConversationItem`，将立刻产生三份映射：

- `Thread <-> new item`
- `compact <-> new item`
- `provider <-> new item`

这会让“上层统一消息模型”反而变成第二套漂移源。

因此本次 change 直接提升 `ChatMessage` 本身，使它成为真正的协议兼容层。具体做法：

- `ChatMessageRole` 新增 `Reasoning`
- `ChatToolCall` 增加 provider continuation 所需的最小引用字段，至少区分：
  - 统一 `tool_call_id`
  - Responses `function_call item id`
- `ChatMessage` 增加可选 provider item metadata，用于保存 Responses `reasoning item id` / `output item id`
- `Toolcall` 语义收紧为“一条消息只承载一个统一 tool call item”；Chat Completions 适配层负责把连续多条 `Toolcall` 重新合并回一个 assistant `tool_calls` payload

对应地，`LLMResponse` 不再是：

- `message: Option<ChatMessage>`
- `tool_calls: Vec<LLMToolCall>`

而是改成：

- `items: Vec<ChatMessage>`

provider 按原始顺序返回 item，AgentLoop 再按 item 序列消费。

Rejected alternatives:

- 方案 C：新建 `LLMConversationItem`，保留 `ChatMessage` 不动
  Rejected，因为这会让 thread/compact/llm 出现长期双模型并存，复杂度高于本次需求。

### 3. AgentLoop 改为按 item 序列提交 turn，而不是先看 `message` 再看 `tool_calls`

当前 loop 默认顺序是：

1. 如果没有 tool call，则提交最终 assistant 文本并结束
2. 如果有 tool call，则先提交 assistant 文本，再提交 tool call，再执行工具

这与 Responses API 的真实输出模型不一致，因为一轮可能先出现 `reasoning`，中间只有 `function_call`，或者同轮同时出现 `reasoning + function_call + assistant_output`。

因此 loop 需要改成：

1. provider 返回 `Vec<ChatMessage>` items
2. loop 按顺序扫描 item
3. 对 `Reasoning`：
   - 写入 thread 正式消息
   - 写调试日志
   - 不发送 channel-facing text output
4. 对 `Assistant`：
   - 写入 thread 正式消息
   - 若内容非空，则发送 text output event
5. 对 `Toolcall`：
   - 写入 thread 正式消息
   - 生成 tool call event
   - 进入现有宿主工具执行路径
6. 工具执行后继续写入 `ToolResult`
7. 若本轮没有任何 `Toolcall`，且出现非空最终 `Assistant`，则 turn 完成

这让 Responses 的保序输出和当前工具执行链路可以兼容。

额外约束：

- `Reasoning` item 首版进入持久化与日志，但默认不进入 channel 文本输出
- compact 继续把它当作普通非 system message 处理
- 对不消费 reasoning 的协议，provider 序列化时可以显式丢弃 `Reasoning` item，而不是要求上层手动过滤

### 4. OpenAI Chat Completions、Responses、Anthropic 全部变成 `ChatMessage -> protocol payload` 适配器

这次 change 会把 `src/llm.rs` 里的协议逻辑拆得更明确：

- `chat_completions` 适配器：
  - 消费统一 `ChatMessage`
  - 合并连续 `Toolcall`
  - 继续用 `async-openai` 构建请求
- `responses` 适配器：
  - 消费统一 `ChatMessage`
  - 把 `System` 消息稳定投影为 `instructions`
  - 把其他 item 投影为 Responses `input`
  - 解析 `response.output` 为保序 `ChatMessage` items
- `anthropic` 适配器：
  - 继续消费统一 `ChatMessage`
  - 序列化为 Anthropic 所需 payload 结构
  - 即便底层 transport 仍是占位实现，也不再保留第二套独立消息规则

这里对 Responses 的关键选择是：

- 所有线程导出的 `System` 消息按稳定顺序拼接为单个 `instructions` 字符串
- 非 system item 进入 `input`

原因：

- 当前线程里 system message 主要是稳定提示前缀，不属于普通 user/assistant/tool 往返历史
- 使用 `instructions` 更贴合 Responses API 语义，也能避免把稳定前缀混进续写 conversation

Rejected alternatives:

- 方案 D：把所有 `System` 消息也直接塞进 Responses `input`
  Rejected，因为这会把稳定 system 前缀与真正 conversation history 混在一起，续写时更难维护。

### 5. Responses provider 首版使用 `reqwest + serde`，不强绑 `async-openai` 升级

当前依赖里的 `async-openai` 已启用的是 chat-completion 相关 feature，而仓库本身已经有 `reqwest`。这次 change 的目标是先把 Responses API 能力正式落地，而不是先做 SDK 升级与 feature 面梳理。

因此 Responses provider 首版选择：

- 继续保留当前 Chat Completions provider 对 `async-openai` 的使用
- Responses provider 单独使用 `reqwest + serde` 组装请求/解析响应
- 两者在更外层共享同一 `ChatMessage` / `ResolvedLLMProviderConfig` / `LLMProvider` trait

这样可以把风险限制在新增协议分支，不扩大到现有 Chat Completions 路径。

### 6. Responses 续写只保留最小必要事实，不持久化 provider 私有大对象

用户给出的 PoC 说明 Responses 续写至少需要：

- `reasoning` 的顺序事实
- `function_call` 的 `call_id`
- prior response output item
- `function_call_output`

但不意味着需要把 provider 私有的完整 SDK 对象或未知字段整包持久化。

本次设计选择只保留“续写必需的最小事实”：

- `Reasoning`：摘要文本 + provider item id
- `Toolcall`：统一 `tool_call_id` + provider item id + 工具名 + arguments
- `ToolResult`：`tool_call_id` + output

若未来某家 Responses 兼容实现要求更多字段，再通过可选 metadata 扩展；本次不为未知字段提前引入“原样透传 blob”。

## Risks / Trade-offs

- [Risk] 直接提升 `ChatMessage` 会波及 thread、compact、budget、llm、agent loop 多个模块
  → Mitigation: 继续沿用单一消息模型，避免演化成长期双模型；实现时按“先模型、再 provider、再 loop”分阶段推进。

- [Risk] `Reasoning` 持久化后会增加上下文长度
  → Mitigation: 首版只保存摘要文本与最小 metadata；对不消费 reasoning 的协议在请求投影时显式过滤。

- [Risk] 旧单 provider 配置与新多 provider 配置并存，校验规则容易产生歧义
  → Mitigation: 明确只允许两种模式二选一，并提供统一 `resolve_active_provider()` 入口，禁止多处自行拼优先级。

- [Risk] Responses provider 与 Chat Completions provider 暂时使用两套 HTTP/SDK 实现
  → Mitigation: 让它们只在 transport 层分叉，统一共享 resolved config、日志字段和 `ChatMessage` 兼容层。

- [Risk] Responses 一轮中可能同时出现中间 item 和最终文本，loop 处理顺序容易出错
  → Mitigation: provider 统一返回保序 `items`，loop 只做顺序消费，不再基于“先 message 后 tool_calls”的隐式假设。

## Migration Plan

1. 扩展 `LLMConfig`，新增 provider profile 结构、`active_provider` 和 resolved 视图，并补齐旧配置兼容测试。
2. 提升 `ChatMessage` / `ChatToolCall` 模型，加入 `Reasoning` 与 Responses continuation 所需的最小 metadata。
3. 改造 `LLMResponse` 和 `LLMProvider` trait，使 provider 输出变为保序 `items`。
4. 先重写现有 Chat Completions provider，确保旧协议在新 item contract 下仍可工作。
5. 新增 Responses provider 与序列化/反序列化逻辑，打通 tool loop continuation。
6. 更新 AgentLoop 的 item 消费逻辑、日志和 thread 持久化顺序。
7. 让 Anthropic serializer 也切到同一 `ChatMessage` 兼容层，去掉当前占位分支里的旧消息假设。
8. 补齐配置解析、跨协议投影、Responses continuation、旧配置兼容和 loop 行为测试。

回滚策略：

- 如果 Responses provider 新分支不稳定，可以保留多 provider 配置与新消息模型，但临时禁用 `openai_responses` 协议构造；
- Chat Completions 路径应保持独立可回退，不依赖 Responses transport 的成功。

## Open Questions

- `Reasoning` item 在 CLI / 调试命令中是否需要单独展示，还是只进入日志与 thread 历史即可。
- Responses provider 是否需要在首版就暴露 `max_tool_calls`、`parallel_tool_calls`、`tool_choice` 等更细粒度 profile 配置，还是先固定为与当前 AgentLoop 兼容的保守默认值。
