## Why

当前项目的 LLM 配置仍然是单一 `llm` 段，运行时也只真正打通了 `mock` 与 `openai_compatible chat completion` 两条路径。这样既无法在同一份配置中声明多个 provider 并按需切换，也无法支持 OpenAI Responses API 这类需要保留 reasoning / function_call / function_call_output 顺序语义的协议。

现在需要把 LLM 配置与消息兼容层一起收敛：一方面允许用户在配置里声明多个具名 provider 并用一个总开关选择当前启用项，另一方面为 `anthropic`、`openai responses` 与 `openai chat completions` 建立统一的上层消息/回合抽象，避免 AgentLoop 直接绑定某一家协议。

## What Changes

- 新增多 provider LLM 配置模型：允许在配置文件中声明多个具名 provider，每个 provider 显式指定协议、模型、鉴权和预算参数，并提供一个顶层选择开关决定当前启用哪个 provider。
- 为现有单 provider `llm` 配置增加兼容归一化路径：旧配置仍可被解析，但运行时会先收敛成“一个 active provider + 一组 provider profile”的统一视图。
- 引入统一的 LLM 会话兼容层，定义能够表达 `user`、`assistant`、`reasoning`、`tool call`、`tool result` 等语义的上层消息/回合模型，并支持投影到 `anthropic`、`openai responses`、`openai chat completions` 三种协议。
- 新增 OpenAI Responses API provider，实现基于 Responses API 的请求构造、工具调用回填和多轮续写，并确保可以像用户给出的 PoC 那样把 prior response output item 与 `function_call_output` 一起追加回下一轮 conversation。
- 调整 provider 构建与日志口径，让全局配置、显式构造和运行时日志都围绕“active provider + protocol”工作，而不是只围绕单个平铺 `llm` 字段工作。

## Capabilities

### New Capabilities
- `llm-provider-selection`: 定义多 provider 配置、active provider 选择与旧单 provider 配置的兼容归一化规则。
- `llm-conversation-compatibility`: 定义统一上层消息/回合模型，以及该模型到 Anthropic、OpenAI Responses、OpenAI Chat Completions 的协议投影与回填规则。
- `responses-api-provider`: 定义 OpenAI Responses API provider 的请求、工具调用、续写和结果归一化行为。

### Modified Capabilities

## Impact

- Affected code: `src/config.rs`、`src/llm.rs`、`src/context/mod.rs`、`src/agent/agent_loop.rs`、`src/main.rs` 以及相关测试。
- API impact: LLM 配置结构会从单 provider 视图扩展为“active provider + providers map”；现有 `build_provider(...)` / `build_provider_from_global_config()` 的解析逻辑会改为先解析 active provider。
- Runtime impact: AgentLoop 与 provider 层之间的返回/续写契约将从“单条 assistant message + tool_calls”扩展为“可保序的统一 conversation turn items”。
- Dependency impact: 可能需要扩展现有 OpenAI SDK 用法，或为 Responses API 补充 HTTP/SDK 适配能力；Anthropic 分支也会从占位序列化边界升级为受兼容层约束的正式协议分支。
- Verification impact: 需要新增配置解析、协议投影、Responses API tool loop、旧配置兼容和跨协议 round-trip 的单元测试与集成测试。
