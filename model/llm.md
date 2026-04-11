# LLM

## 定位

- `llm` 模块是模型协议适配层。
- 它把 OpenJarvis 的统一消息和工具定义投影到具体供应商协议。

## 边界

- 负责 `LLMRequest/LLMResponse` 抽象和 provider 调用。
- 不负责 prompt 编排，不负责线程状态，不负责工具可见性决策。

## 关键概念

- `LLMProvider`
  统一生成接口。
- `LLMRequest`
  标准输入，包含消息和可见工具。
- `LLMResponse`
  标准输出，包含 assistant 消息和 tool calls。
- `ToolSchemaProtocol`
  工具 schema 的协议投影。

## 核心能力

- 当前支持 `mock` 和 `openai_compatible`。
- 需要支持 `openai_compatible/anthropic/openai_response` 三种协议
- `anthropic` 分支已经留好协议边界，但还没实现真实传输。
- 把统一 `ChatMessage` 和 `ToolDefinition` 序列化为供应商请求格式。

## 使用方式

- 主启动链路可以在配置 install 后，通过 `build_provider_from_global_config()` 收敛顶层装配。
- 单测、嵌入式调用和纯组件构造继续优先使用显式 `build_provider(&LLMConfig)`。
- AgentLoop 只依赖 `LLMProvider` trait，不直接依赖某个 SDK。
- 新 provider 应先满足统一消息模型，再考虑厂商特性。
