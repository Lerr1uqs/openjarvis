# system 架构符合性审计

审计时间：2026-03-23

基线文档： ` arch/system.md `

## 结论

当前项目已经具备 ` Channel -> Router -> AgentWorker -> AgentLoop -> LLMProvider / ToolRegistry -> Channel ` 的主链，实现上与架构文档的总体分层基本一致。

以下只列出目前仍然不符合，或仅部分符合 ` arch/system.md ` 的项。

## 不符合项

### 1. Thread 定位策略与架构定义不一致

- 架构要求： ` SessionManager ` 通过 ` uuid:channel:external_thread_id ` 定位 thread，首次遇到时创建内部唯一 uuid。证据： ` arch/system.md:48 `, ` arch/system.md:49 `
- 当前实现： ` SessionKey ` 只包含 ` channel + user_id `， ` ThreadLocator ` 只包含 ` channel + user_id + thread_id `；当外部线程为空时会退化成固定值 ` default `，没有建立 ` external_thread_id -> internal_uuid ` 的映射。证据： ` src/session.rs:17 `, ` src/session.rs:33 `, ` src/session.rs:74 `, ` src/model.rs:24 `, ` src/model.rs:65 `, ` src/model.rs:69 `
- 影响：当前 thread 身份模型和设计稿不一致，后续如果接 PostgreSQL 或做 thread 迁移，键设计需要重做。
- Uuid::parse_str(external_thread_id)

### 2. SessionStrategy 的保留语义与文档不一致

- 架构要求：当前策略是“只保留最新 5 个 message”。证据： ` arch/system.md:52 `, ` arch/system.md:58 `
- 当前实现：策略字段是 ` max_messages_per_turn `，只会在单次 ` store_turn ` 时裁剪该 turn 内的消息；整个 thread 的历史 turn 仍会持续累积。证据： ` src/session.rs:105 `, ` src/session.rs:118 `, ` src/session.rs:123 `, ` src/session.rs:219 `, ` src/thread.rs:62 `
- 影响：长线程不会按“最近 5 条消息”收敛，真实上下文增长行为与架构文档描述不同。

### 3. AgentWorker 未包装沙箱容器

- 架构要求： ` AgentWorker ` = 沙箱容器 + ` AgenticLoop `。证据： ` arch/system.md:62 `, ` arch/system.md:63 `
- 当前实现： ` AgentWorker ` 只持有 ` AgentLoop ` 和 ` system_prompt `；内置 ` bash ` 工具直接执行本机 ` powershell/sh `；README 也明确写了 ` memory / sandbox ` 仍为空壳。证据： ` src/agent/worker.rs:53 `, ` src/agent/worker.rs:54 `, ` src/agent/worker.rs:55 `, ` src/agent/worker.rs:79 `, ` src/agent/tool/shell.rs:46 `, ` src/agent/tool/shell.rs:53 `, ` src/agent/tool/shell.rs:96 `, ` README.md:25 `
- 影响：工具执行边界和安全模型尚未达到架构稿对 Worker 的预期。
- 用户结论: 现阶段不需要沙箱 只需要一个占位沙箱即可

### 4. Hook 体系存在，但没有从配置文件加载

- 架构要求：Hook 由配置文件加载。证据： ` arch/system.md:68 `, ` arch/system.md:69 `
- 当前实现：配置模型只有 ` server/channels/llm `，没有 ` agent.hook ` 相关字段； ` AgentWorker::from_config ` 只接收 ` LLMConfig ` 并注入默认 prompt； ` AgentRuntime::new ` 只创建空的 ` HookRegistry `。证据： ` src/config.rs:14 `, ` src/config.rs:15 `, ` src/config.rs:17 `, ` src/config.rs:18 `, ` src/agent/worker.rs:85 `, ` src/agent/worker.rs:88 `, ` src/agent/runtime.rs:17 `, ` src/agent/hook.rs:47 `
- 影响：Hook 目前只能在代码中手工注册，不能按 YAML 配置驱动。

### 5. Command 组件未落地

- 架构要求：收到用户消息后，要先经过 ` Command ` 组件截取，命中注册命令时不再进入上下文和 Agent 处理。证据： ` arch/system.md:85 `, ` arch/system.md:86 `
- 当前实现： ` Router ` 在接收消息后会直接构造 ` ThreadLocator `、加载 history，并发送 ` AgentRequest `；没有命令前置截流分支。证据： ` src/router.rs:149 `, ` src/router.rs:168 `, ` src/router.rs:169 `, ` src/router.rs:316 `, ` src/router.rs:319 `
- 影响：像 ` /approve ` 这样的控制命令目前不会绕过正常对话流程。
- 用户结论: 暂时不需要实现

### 6. Cron / CLIAbility 组件未进入当前工程主模块

- 架构要求：存在 ` Cron ` 和 ` CLIAbility ` 两类组件。证据： ` arch/system.md:88 `, ` arch/system.md:99 `
- 当前实现：从当前导出的主模块可以推断，仓库里还没有独立的 ` cron ` 或 ` cliability ` 运行模块；顶层只导出了 ` agent/channels/config/context/llm/model/router/session/thread `，agent 层只导出了 ` agent_loop/hook/mcp/runtime/tool/worker `。证据： ` src/lib.rs:3 `, ` src/lib.rs:11 `, ` src/agent/mod.rs:3 `, ` src/agent/mod.rs:8 `
- 影响：定时任务和 CLI 能力注册尚未进入真实运行流。
- 用户结论: 暂时不需要实现

### 7. Memory 子系统尚未接入上下文与工具闭环

- 架构要求：提供 ` memory_search ` / ` memory_write `，数据写入 ` ./.memory/ `，其中 ` MEMORY.md ` 要加载到上下文。证据： ` arch/system.md:90 `, ` arch/system.md:92 `, ` arch/system.md:93 `, ` arch/system.md:94 `
- 当前实现： ` MessageContext ` 虽然有 ` memory ` 区段和 ` push_memory ` 接口，但 ` build_context ` 只写入 system、history 和当前 user message；另外从顶层模块可以推断，目前没有独立 memory 模块；README 也明确写了 ` memory / sandbox ` 尚未接入。证据： ` src/context.rs:85 `, ` src/context.rs:114 `, ` src/agent/worker.rs:213 `, ` src/agent/worker.rs:218 `, ` src/agent/worker.rs:219 `, ` src/agent/worker.rs:220 `, ` src/lib.rs:3 `, ` src/lib.rs:11 `, ` README.md:25 `
- 影响：长短期记忆、永久记忆加载和记忆检索写入都还没有形成闭环。
- 用户结论: 暂时不需要实现

### 8. ToolRegistry 只注册了四个本地工具，未覆盖 memory 类工具

- 架构要求： ` ToolRegistry ` 管理基础工具，文档示例里明确包含 ` bash `、 ` memory ` 等能力。证据： ` arch/system.md:102 `, ` arch/system.md:103 `
- 当前实现：工具模块只有 ` edit/read/shell/write ` 四类，内置注册也只注册这四个，没有 ` memory_search ` / ` memory_write `。证据： ` src/agent/tool/mod.rs:3 `, ` src/agent/tool/mod.rs:6 `, ` src/agent/tool/mod.rs:138 `, ` src/agent/tool/mod.rs:143 `
- 影响：即使后续补齐 memory 存储层，当前模型侧也没有对应工具入口。
- 用户结论: 暂时不需要实现MEMORY

### 9. MCP 只有注册表，占位多于能力

- 架构要求：MCP 可以通过配置文件配置，使模型具备调用 MCP 的能力。证据： ` arch/system.md:105 `, ` arch/system.md:106 `
- 当前实现： ` AgentRuntime ` 只创建空的 ` McpRegistry `；配置结构没有 ` mcp ` 字段； ` AgentLoop ` 在构建 LLM 请求时只把 ` self.runtime.tools().list() ` 作为 tools 传给模型， ` mcp ` 仅用于统计 ` mcp_server_count `。证据： ` src/config.rs:14 `, ` src/config.rs:18 `, ` src/agent/runtime.rs:19 `, ` src/agent/agent_loop.rs:166 `, ` src/agent/agent_loop.rs:310 `, ` src/agent/mcp.rs:23 `, ` src/agent/mcp.rs:34 `
- 影响：MCP 当前还是数据结构占位，不是可用的运行时能力。
- 用户结论: 需要实现

### 10. LLMProvider 对 Anthropic 只做了占位

- 架构要求：兼容 openai 和 anthropic 协议。证据： ` arch/system.md:96 `, ` arch/system.md:97 `
- 当前实现：provider 枚举已经接受 ` anthropic/claude `，但 Anthropic 分支最终会直接报错 ` not implemented yet `。证据： ` src/llm.rs:78 `, ` src/llm.rs:190 `, ` src/llm.rs:193 `, ` src/llm.rs:299 `, ` src/llm.rs:308 `
- 影响：当前只能算“OpenAI-compatible 已实现，Anthropic 未实现”，与文档里的双协议兼容状态不一致。
