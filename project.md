# OpenJarvis 项目索引

## 1. 项目目标

OpenJarvis 当前是一个消息驱动的 Rust agent 运行时，目标是让外部聊天平台把消息送进来后，经过统一路由、会话管理、Agent ReAct loop、LLM 和工具调用，再把结果回发到原平台。

当前实际最小闭环是：

`Feishu long connection -> ChannelRouter -> AgentWorker -> AgentLoop -> LLMProvider / ToolRegistry -> ChannelRouter -> FeishuChannel`

这个仓库已经落地的重点不是“完整多 agent 平台”，而是：

- 一个可运行的单 crate Rust 服务
- 一个统一的 channel/router/agent 边界
- 一个可测试的 ReAct 执行链
- 一套 builtin tool + MCP tool 的统一注册与调用模型

## 2. 阅读优先级

如果是 agent 首次加载仓库，建议按这个顺序看：

1. `src/main.rs`
2. `src/router.rs`
3. `src/agent/worker.rs`
4. `src/agent/agent_loop.rs`
5. `src/session.rs`
6. `src/thread.rs`
7. `src/context.rs`
8. `src/agent/tool/mod.rs`
9. `src/agent/tool/mcp/mod.rs`
10. `src/channels/feishu.rs`
11. `src/config.rs`

注意：

- 当前代码比 `arch/` 和 `agent-doc/` 里的部分文档更可信。
- 文档里有一些旧名字，如 `begin_turn`、`complete_turn`、独立 `agent::mcp` 子系统等；当前真实实现应以 `src/` 和 `tests/` 为准。

## 3. 启动链路

### 3.1 入口

- `src/main.rs`
  - 解析 CLI 参数
  - 初始化 tracing
  - 读取 `AppConfig`
  - 构建 `AgentWorker`
  - 创建 `ChannelRouter`
  - 注册 channel
  - 进入 `run_until_shutdown`

- `src/lib.rs`
  - crate root，只做模块导出

### 3.2 主启动流程

`main`

-> `AppConfig::load()`

-> `AgentWorker::from_config()`

-> `AgentRuntime::from_config()`

-> `ChannelRouter::builder().agent(...).build()`

-> `ChannelRouter::register_channels()`

-> `ChannelRouter::run_until_shutdown()`

## 4. 实际消息链路

### 4.1 入站

`scripts/feishu_ws_client.mjs`

-> 飞书长连接事件转成 JSON 行

-> `src/channels/feishu.rs` 读取 sidecar stdout

-> `FeishuChannel::parse_long_connection_incoming()`

-> `IncomingMessage`

-> `ChannelRouter::handle_incoming()`

-> 先走 `CommandRegistry::try_execute()`

-> 非命令消息走 `SessionManager::load_or_create_thread()`

-> `AgentWorker` 收到 `AgentRequest`

-> `build_context()`

-> `AgentLoop::run()`

-> LLM 返回文本 / tool_calls

-> `ToolRegistry::call()` 或 MCP tool 调用

-> `AgentWorkerEvent`

-> `ChannelRouter::store_completed_turn()` / `store_failed_turn()`

### 4.2 出站

`AgentLoop` 产生 `AgentDispatchEvent`

-> `ChannelRouter::process_agent_dispatch_event()`

-> `OutgoingMessage`

-> `ChannelRouter::dispatch_outgoing()`

-> `FeishuChannel::deliver_outgoing()`

-> 先加 Typing reaction

-> 再发飞书文本消息

## 5. 目录结构与职责

### 5.1 根目录

- `Cargo.toml`
  - Rust crate 配置和依赖
- `README.md`
  - 当前 MVP 说明
- `config.example.yaml`
  - 默认配置样例
- `config/openjarvis/mcp.json`
  - 外部 MCP sidecar 配置样例
- `package.json`
  - Node sidecar 依赖，仅用于飞书长连接
- `scripts/`
  - 辅助脚本与测试 bin
- `arch/`
  - 架构设计文档，部分内容已滞后
- `agent-doc/`
  - 旧版模块说明，适合作为设计意图参考，不应覆盖源码事实
- `openspec/`
  - 变更提案、设计和任务清单
- `src/`
  - 主源码
- `tests/`
  - 与 `src/` 基本镜像的集成测试

### 5.2 `src/` 顶层模块

- `src/main.rs`
  - 二进制入口
- `src/lib.rs`
  - crate 导出入口
- `src/config.rs`
  - YAML 配置加载
  - `config/openjarvis/mcp.json` 合并
  - hook / tool / MCP / LLM 配置模型
- `src/model.rs`
  - `IncomingMessage` / `OutgoingMessage` / `ReplyTarget`
- `src/router.rs`
  - Router 主循环
  - channel 注册
  - agent 事件消费
  - command 前置拦截
  - thread 串行化与排队
- `src/context.rs`
  - `ChatMessage`
  - `ChatMessageRole`
  - `MessageContext`
- `src/session.rs`
  - `SessionManager`
  - `Session`
  - `ThreadLocator`
  - 历史消息加载与存储策略
- `src/thread.rs`
  - `ConversationThread`
  - `ConversationTurn`
- `src/llm.rs`
  - LLM provider 抽象
  - mock provider
  - OpenAI-compatible provider
  - Anthropic 协议占位
- `src/command.rs`
  - slash command 解析与执行
- `src/cli.rs`
  - 二进制 CLI 和内部 MCP demo 子命令

### 5.3 `src/channels/`

- `src/channels/mod.rs`
  - `Channel` trait
  - `ChannelRegistration`
- `src/channels/feishu.rs`
  - 当前唯一内置 channel
  - 负责 Node sidecar 启动、飞书消息解析、飞书消息回发、tenant token 缓存

### 5.4 `src/agent/`

- `src/agent/worker.rs`
  - Router 唯一直接依赖的 agent 入口
  - 把 `IncomingMessage + history` 组装成一次 `AgentRequest`
  - 启动长生命周期 worker task
  - 把 loop 结果转成 `CompletedAgentTurn` / `FailedAgentTurn`

- `src/agent/agent_loop.rs`
  - 当前真正的 agent 执行核心
  - 单轮或多次循环地调用 LLM
  - 把 tool_call / tool_result / text_output 事件实时回传给 router
  - 负责 hook 触发时机

- `src/agent/runtime.rs`
  - `AgentRuntime`
  - 聚合 `HookRegistry` 和 `ToolRegistry`

- `src/agent/hook.rs`
  - hook 事件枚举
  - 配置驱动的脚本 hook 执行器
  - 通过环境变量传递 `OPENJARVIS_HOOK_EVENT` 和 `OPENJARVIS_HOOK_PAYLOAD`

- `src/agent/sandbox.rs`
  - 只有 `DummySandboxContainer`
  - 目前没有真实沙箱

### 5.5 `src/agent/tool/`

- `src/agent/tool/mod.rs`
  - `ToolHandler` trait
  - `ToolDefinition`
  - `ToolRegistry`
  - builtin tool 批量注册
  - MCP tool 同步到统一 registry

- `src/agent/tool/read.rs`
  - 读取 UTF-8 文件，可按行截取

- `src/agent/tool/write.rs`
  - 全量覆盖写文件，自动建目录

- `src/agent/tool/edit.rs`
  - 精确匹配字符串并替换首个命中

- `src/agent/tool/shell.rs`
  - 执行本地 shell 命令
  - Windows 下实际走 PowerShell 封装
  - 对外暴露工具名仍然叫 `bash`

- `src/agent/tool/mcp/mod.rs`
  - MCP server 定义、启停、探测、工具发现、调用转发
  - 当前支持 `stdio` 和 `streamable_http`
  - 将远端工具映射为 `mcp__<server>__<tool>`

- `src/agent/tool/mcp/demo.rs`
  - 内置 demo MCP server
  - 仅用于测试和协议验证

## 6. 核心组件关系

### 6.1 依赖方向

当前比较稳定的依赖方向如下：

- `channels -> model + config`
- `router -> channels + agent + command + session + context + model`
- `agent -> llm + context + model + config`
- `agent/runtime -> hook + tool`
- `tool -> config + mcp`
- `session -> thread + context + model`
- `llm -> context + agent(tool schema)`

### 6.2 组件边界

- Channel 不知道 agent 内部如何执行，只负责平台协议收发。
- Router 不做 LLM 协议处理，只负责路由、排队、命令拦截、结果回发。
- Agent 不关心飞书 SDK，只接收统一 `IncomingMessage`。
- Session / Thread / Context 不关心具体 channel，也不直接碰网络。
- ToolRegistry 把 builtin tools 和 MCP tools 统一成一套调用接口。

## 7. 关键数据模型

### 7.1 消息模型

- `IncomingMessage`
  - 外部平台 -> Router 的统一入站消息
- `OutgoingMessage`
  - Router -> 外部平台的统一出站消息
- `ReplyTarget`
  - 平台发送目标抽象

位置：

- `src/model.rs`

### 7.2 会话模型

当前会话分层是：

`Session(channel + user_id) -> Thread(external_thread_id -> internal UUID) -> Turn(messages)`

关键点：

- `SessionKey = channel + user_id`
- `ThreadLocator` 同时保存 session_id、internal thread_id 和 external_thread_id
- 没有 thread_id 的平台消息会落到 `"default"`
- `SessionStrategy` 默认只保留每个 thread 最近 10 条消息

位置：

- `src/session.rs`
- `src/thread.rs`

### 7.3 Prompt / Chat 模型

`MessageContext` 目前拆成：

- `system`
- `memory`
- `chat`

真正传给 provider 的是 `Vec<ChatMessage>`，不是字符串 prompt。

位置：

- `src/context.rs`
- `src/llm.rs`

## 8. 配置与外部依赖

### 8.1 主配置

- 默认配置文件：`config.yaml`
- 示例文件：`config.example.yaml`
- 环境变量覆盖入口：`OPENJARVIS_CONFIG`

### 8.2 关键配置段

- `feishu`
  - 飞书 channel 运行配置
- `agent.hook`
  - hook 脚本配置
- `agent.tool.mcp.servers`
  - YAML 内联 MCP server 定义
- `llm`
  - provider / model / base_url / api_key

### 8.3 MCP 外部 sidecar 文件

- 默认路径：`config/openjarvis/mcp.json`
- 加载逻辑：`AppConfig::from_path()` 会在 YAML 根目录旁边尝试加载它
- 若 YAML 和 JSON 里 server 同名，会直接报错

### 8.4 Node 侧依赖

- `package.json`
  - 仅用于飞书长连接 Node SDK sidecar
- `scripts/feishu_ws_client.mjs`
  - 飞书事件转发到 Rust 进程 stdout

## 9. 测试结构

测试目录基本按 `src/` 镜像组织，这是这个仓库很重要的导航信号。

### 9.1 顶层测试

- `tests/config.rs`
  - 配置解析、校验、MCP sidecar 合并
- `tests/router.rs`
  - Router 主行为
- `tests/router_timeout_root_cause.rs`
  - Router 常驻与关闭语义
- `tests/session.rs`
  - session/thread 解析和存储策略
- `tests/thread.rs`
  - turn 存储与消息裁剪
- `tests/context.rs`
  - MessageContext 组织逻辑
- `tests/llm.rs`
  - provider 构造和序列化
- `tests/model.rs`
  - 入站/出站模型辅助逻辑
- `tests/command.rs`
  - slash command
- `tests/main.rs`
  - 启动路径和配置错误行为

### 9.2 agent 子目录测试

- `tests/agent/agent_loop.rs`
  - ReAct loop 行为
- `tests/agent/worker.rs`
  - worker 封装和 turn 上报
- `tests/agent/runtime.rs`
  - runtime 聚合
- `tests/agent/hook.rs`
  - hook 执行
- `tests/agent/sandbox.rs`
  - dummy sandbox
- `tests/agent/tool/*.rs`
  - builtin tools
- `tests/agent/tool/mcp/*.rs`
  - MCP server 生命周期、真实协议联调、demo server

### 9.3 channel 子目录测试

- `tests/channels/feishu.rs`
  - 飞书消息解析

规则上，如果新增 `src` 文件，最好同步补一个 `tests/` 下对应镜像测试文件。

## 10. 常见改动应该从哪里入手

### 10.1 修改启动或配置加载

先看：

- `src/main.rs`
- `src/config.rs`
- `tests/config.rs`
- `tests/main.rs`

### 10.2 修改消息路由、排队、线程串行化

先看：

- `src/router.rs`
- `src/session.rs`
- `src/thread.rs`
- `tests/router.rs`
- `tests/router_timeout_root_cause.rs`

### 10.3 修改 agent 推理、事件回发、tool 执行

先看：

- `src/agent/worker.rs`
- `src/agent/agent_loop.rs`
- `tests/agent/worker.rs`
- `tests/agent/agent_loop.rs`

### 10.4 修改 builtin tools

先看：

- `src/agent/tool/mod.rs`
- `src/agent/tool/read.rs`
- `src/agent/tool/write.rs`
- `src/agent/tool/edit.rs`
- `src/agent/tool/shell.rs`
- `tests/agent/tool/`

### 10.5 修改 MCP 管理、远端工具暴露、MCP 调用

先看：

- `src/agent/tool/mcp/mod.rs`
- `src/agent/tool/mcp/demo.rs`
- `src/config.rs`
- `config/openjarvis/mcp.json`
- `tests/agent/tool/mcp/`

### 10.6 修改飞书接入

先看：

- `src/channels/feishu.rs`
- `src/channels/mod.rs`
- `scripts/feishu_ws_client.mjs`
- `tests/channels/feishu.rs`

### 10.7 修改 LLM provider

先看：

- `src/llm.rs`
- `src/context.rs`
- `tests/llm.rs`

## 11. 当前已实现能力与未完成边界

### 11.1 已实现

- Feishu 长连接接入
- 统一 Router / Channel / Agent 边界
- slash command 前置拦截
- Session / Thread / Context 内存态组织
- OpenAI-compatible provider
- mock provider
- builtin tools: `read` / `write` / `edit` / `bash`
- tool-managed MCP
- demo MCP server
- 配置驱动 hook

### 11.2 明确未完成或占位

- 真实 sandbox 还没有，只有 `DummySandboxContainer`
- 持久化 session / memory 还没有
- 非飞书 channel 还没有
- webhook 模式没有实现完成，当前主路径是 long connection
- Anthropic provider 只是协议占位，未真正接通
- 权限审批、工具授权、管理员认证都还没有落地

## 12. 文档地图

以下文档可作为补充，但不要高于源码事实：

- `README.md`
  - 当前 MVP 使用说明
- `arch/system.md`
  - 早期整体架构草图
- `arch/mcp.md`
  - MCP 设计意图，和当前代码较接近
- `arch/tools.md`
  - builtin tool 抽象说明
- `agent-doc/architecture.md`
  - 旧版模块关系说明
- `agent-doc/agent.md`
  - 旧版 agent 子目录说明
- `agent-doc/context-session-thread.md`
  - 旧版会话模型说明
- `openspec/changes/add-tool-managed-mcp/`
  - MCP 变更提案、设计和任务清单

## 13. 给后续 agent 的结论

如果只是想快速定位问题，优先记住这几个事实：

- 入口在 `src/main.rs`
- 主循环在 `src/router.rs`
- agent 执行核心在 `src/agent/agent_loop.rs`
- 会话历史在 `src/session.rs` + `src/thread.rs`
- tool 统一入口在 `src/agent/tool/mod.rs`
- MCP 统一入口在 `src/agent/tool/mcp/mod.rs`
- 飞书接入在 `src/channels/feishu.rs` 和 `scripts/feishu_ws_client.mjs`
- 代码事实优先级高于 `arch/` 与 `agent-doc/`
