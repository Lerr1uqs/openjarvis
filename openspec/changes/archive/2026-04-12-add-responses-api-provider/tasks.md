## 1. 多 provider 配置与解析

- [x] 1.1 扩展 `src/config.rs` 的 `LLMConfig`，新增 `active_provider`、`providers`、`LLMProviderProfileConfig` 与 resolved active provider 解析入口
- [x] 1.2 实现旧单 provider `llm` 配置到隐式 active provider 视图的兼容归一化，并拒绝歧义的混合配置模式
- [x] 1.3 让 `main.rs`、`build_provider(...)`、`build_provider_from_global_config()` 与相关日志统一使用 resolved active provider / protocol 视图

## 2. 统一消息兼容层

- [x] 2.1 提升 `src/context/mod.rs` 中的 `ChatMessage` / `ChatToolCall`，加入 `Reasoning` 与 Responses continuation 所需的最小 metadata 字段
- [x] 2.2 更新 `src/llm.rs` 的 provider contract，把 `LLMResponse` 从 `message + tool_calls` 改为保序 `items`
- [x] 2.3 调整 `src/thread.rs`、compact 预算/渲染和其他依赖 `ChatMessageRole` 的模块，使其接受新的 `Reasoning` 与单条 `Toolcall` 语义

## 3. 协议适配器与 Responses provider

- [x] 3.1 重构现有 OpenAI Chat Completions 序列化/反序列化逻辑，使其基于保序 `ChatMessage` items 合并连续 `Toolcall` 并保持旧行为兼容
- [x] 3.2 新增 OpenAI Responses API provider，使用 `reqwest + serde` 实现 `instructions + input` 请求构造、响应解析与 continuation input 组装
- [x] 3.3 让 Anthropic serializer 切换到同一 `ChatMessage` 兼容层，不再保留旧的独立消息假设

## 4. AgentLoop 与工具循环集成

- [x] 4.1 改造 `src/agent/agent_loop.rs`，按 provider 返回的保序 `items` 依次提交 `Reasoning`、`Assistant`、`Toolcall` 与 `ToolResult`
- [x] 4.2 保持当前宿主工具执行路径不变，并确保 Responses `function_call_output` 能由现有 `ToolResult` 提交路径回填到下一轮 provider 请求
- [x] 4.3 补齐关键日志，明确 active provider、protocol、Responses continuation 与 reasoning/tool call 提交顺序

## 5. 验证与回归

- [x] 5.1 在对应测试目录补齐配置单测，覆盖多 provider 选择、header 解析、旧配置兼容和混合配置报错
- [x] 5.2 在对应测试目录补齐 LLM 适配层单测，覆盖 Chat Completions tool-call 合并、Anthropic 投影和 Responses output 保序归一化
- [x] 5.3 增加 Responses tool loop 集成测试，覆盖 `reasoning -> function_call -> function_call_output -> final assistant` 的 continuation 链路
