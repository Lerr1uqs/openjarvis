## 背景与动机

`ContextMessage`（即 `MessageContext`）最初设计为 Context 层核心类型，将发给 LLM 的消息分为三段：`system`、`memory`、`chat`。现在 `ThreadContext` 已经是线程运行时的唯一宿主，对话历史由 `ThreadContext.load_messages()` 管理。`ContextMessage` 的三段式分割变得多余。

当前数据流的冗余：

```
Worker: build_context(system_prompt, &thread_context, &incoming)
    → context.chat = thread_context.load_messages() + [user_message]   // 拷贝 thread 历史
    → context.system = [system_prompt_message]

Agent Loop: run_with_thread_context(input, &context, thread_context)
    → current_user_message = context.chat.last()                       // 从 ContextMessage 取
    → working_chat = thread_context.load_messages()                     // 又从 ThreadContext 取
    → working_chat.push(current_user_message)
    → build_react_messages(&context.system, &context.memory, working_chat, ...)
```

`context.chat` 中的历史是对 `ThreadContext` 的冗余拷贝。`context.system` 和 `context.memory` 只是被透传给 `build_react_messages` 的 `Vec<ChatMessage>`，不应该构成一个独立的"上下文层"。

关于 memory 的现状：`ChatMessageRole::Memory` 已定义且管道已接通（LLM 序列化、budget 估算、compact 排除），但当前没有任何 memory provider 往里填充数据。`build_context` 中 `context.memory` 始终为空 `Vec`，`push_memory()` 标记了 `#[allow(dead_code)]`。Memory 是可选的——只有命中主动记忆时才会存在。因此 memory 不应该作为独立的结构层存在，它就是消息序列的一部分，有就包含，没有就不包含。

正确的设计：agent loop 只接收 `ThreadContext`（历史）和一组 `messages`（本轮输入）。这组 messages 就是一个普通的 `Vec<ChatMessage>`，由调用方组装，可能包含 system、memory（可选）、user_input，不再区分为三段。

## ADDED Requirements

### Requirement: `run_with_thread_context` SHALL 接收 `ThreadContext` + `messages: Vec<ChatMessage>`

当前签名：
```rust
run_with_thread_context(input, context: &ContextMessage, thread_context)
```

变更后签名：
```rust
pub async fn run_with_thread_context(
    &self,
    input: InfoContext,
    messages: Vec<ChatMessage>,
    thread_context: ThreadContext,
) -> Result<AgentLoopOutput>
```

`messages` 是本轮输入的消息序列，由调用方组装。典型内容：
- 新 thread（空历史）：`vec![system_msg, user_input]`
- 已有历史的 thread：`vec![user_input]`
- 命中主动记忆时：`vec![system_msg, memory_msg..., user_input]`（memory 可选，只有在命中时才包含）

agent loop 内部不再区分 system/memory/user——它只关心：加载 thread 历史，追加本轮 messages，运行 ReAct 循环。

**关于 memory**：当前 `ChatMessageRole::Memory` 已定义且管道已接通（LLM 序列化为 system message、budget 估算、compact 排除），但没有任何 memory provider 往里填充数据。Memory 是可选的——只有命中主动记忆时才会出现在 messages 中。当前实现中 messages 不会包含 memory 消息。

#### Scenario: agent loop 不再依赖 ContextMessage
- **GIVEN** 一个有效的 `ThreadContext`
- **WHEN** 调用 `run_with_thread_context`
- **THEN** 签名中 SHALL NOT 包含 `context: &ContextMessage` 参数
- **THEN** 本轮输入 SHALL 通过 `messages: Vec<ChatMessage>` 传入
- **THEN** 对话历史 SHALL 从 `thread_context.load_messages()` 获取
- **THEN** agent loop 将 `messages` 追加到 working_chat 后开始 ReAct 循环

### Requirement: `build_react_messages` SHALL 不再单独接收 system/memory

当前签名：
```rust
fn build_react_messages(
    system_messages: &[ChatMessage],
    memory_messages: &[ChatMessage],
    working_chat_messages: &[ChatMessage],
    toolset_catalog_prompt, skill_catalog_prompt, auto_compact_prompt,
) -> Messages
```

system/memory 消息现在包含在 working_chat_messages 内（来自 thread 历史或本轮 input messages），不再需要单独注入。

变更后签名：
```rust
fn build_react_messages(
    working_chat_messages: &[ChatMessage],
    toolset_catalog_prompt: Option<&str>,
    skill_catalog_prompt: Option<&str>,
    auto_compact_prompt: Option<&str>,
) -> Messages
```

组装逻辑：runtime instructions（tool-use mode、toolset catalog、skill catalog、auto-compact prompt）仍作为 system 消息注入到 working_chat_messages 前方。

#### Scenario: build_react_messages 只注入 runtime instructions
- **WHEN** `prepare_request_state` 调用 `build_react_messages`
- **THEN** SHALL 只接收 `working_chat_messages`（已包含 system/memory/user）
- **THEN** runtime instructions 仍注入到 messages 的合适位置
- **THEN** 不再单独接收 system_messages / memory_messages

### Requirement: `prepare_request_state` SHALL 不再接收 ContextMessage

变更后签名：
```rust
async fn prepare_request_state(
    &self,
    thread_context: &mut ThreadContext,
    working_chat_messages: &[ChatMessage],
    toolset_catalog_prompt: Option<&str>,
    skill_catalog_prompt: Option<&str>,
) -> Result<RequestState>
```

system/memory 已包含在 `working_chat_messages` 中，不再需要从 `ContextMessage` 提取。

#### Scenario: prepare_request_state 不再接收 ContextMessage
- **WHEN** 每轮循环调用 `prepare_request_state`
- **THEN** `working_chat_messages` 已包含所有消息（system/memory/chat）
- **THEN** `build_react_messages` 的调用自然不再传递 system/memory

### Requirement: worker SHALL 组装 messages 序列，不再调用 `build_context`

当前 `handle_request` 调用 `build_context()` 构造 `MessageContext`。变更后 SHALL：
1. 移除 `build_context` 函数
2. 直接组装 `messages: Vec<ChatMessage>`
3. 调用新签名的 `run_with_thread_context`

```rust
// worker.rs handle_request 中

let mut messages = vec![ChatMessage::new(
    ChatMessageRole::System,
    self.system_prompt.clone(),
    Utc::now(),
)];
// memory 消息在这里按需注入（可选，当前无 memory provider）
// if let Some(memory) = self.memory_provider.retrieve(&incoming).await {
//     messages.extend(memory.into_iter().map(|m| ChatMessage::new(ChatMessageRole::Memory, m, Utc::now())));
// }
messages.push(ChatMessage::new(
    ChatMessageRole::User,
    request.incoming.content.clone(),
    request.incoming.received_at,
));

let loop_output = self.agent_loop
    .run_with_thread_context(info_context, messages, request.thread_context)
    .await;
```

#### Scenario: worker 不再构造 MessageContext
- **WHEN** `AgentWorker::handle_request` 准备调用 agent loop
- **THEN** SHALL NOT 调用 `build_context`
- **THEN** SHALL 直接构建 `messages: Vec<ChatMessage>`
- **THEN** messages 中包含 system prompt 和 user input
- **THEN** memory 消息是可选的——只有 memory provider 命中时才注入，当前不注入

### Requirement: `AgentLoop::run` 入口 SHALL 标记 deprecated 并适配新签名

当前 `run` 通过 `backfill_thread_context_from_context` 从 ContextMessage 反向构造 ThreadContext。变更后：

```rust
#[deprecated(note = "use run_with_thread_context instead")]
pub async fn run(
    &self,
    input: InfoContext,
    messages: Vec<ChatMessage>,
) -> Result<AgentLoopOutput>
```

内部实现：构造空 `ThreadContext`，转发到 `run_with_thread_context`。

#### Scenario: deprecated run 接收 messages 而非 ContextMessage
- **WHEN** 测试调用 `run`
- **THEN** SHALL 传入 `messages: Vec<ChatMessage>`
- **THEN** 内部创建空 `ThreadContext` 后转发到 `run_with_thread_context`

### Requirement: deprecated `run_with_thread` SHALL 适配新签名

```rust
#[deprecated(note = "use run_with_thread_context instead")]
pub async fn run_with_thread(
    &self,
    input: InfoContext,
    messages: Vec<ChatMessage>,
    active_thread: ConversationThread,
) -> Result<AgentLoopOutput>
```

#### Scenario: run_with_thread 适配新参数
- **WHEN** 调用 `run_with_thread`
- **THEN** SHALL 接收 `messages` 而非 `&ContextMessage`
- **THEN** 内部将 `ConversationThread` 转为 `ThreadContext` 后转发

### Requirement: 移除 `current_user_message_from_context` 和 `backfill_thread_context_from_context`

- `current_user_message_from_context`（`agent_loop.rs:1027`）：从 `context.chat.last()` 提取 → 被 `messages` 参数替代
- `backfill_thread_context_from_context`（`agent_loop.rs:1031`）：从 ContextMessage 反向填充 ThreadContext → 被直接构造空 ThreadContext 替代

### Requirement: `ContextMessage` / `MessageContext` 类型定义 SHALL 保留

保留在 `src/context/mod.rs`。agent loop 和 worker 不再引用，但类型和方法保留备用。产生 unused warning 时用 `#[allow(dead_code)]` 压制。

---

## 受影响的文件与函数

| 文件 | 函数/结构 | 变更 |
|------|-----------|------|
| `src/agent/agent_loop.rs:216` | `AgentLoop::run` | 移除 `context`，改为 `messages: Vec<ChatMessage>`，标记 deprecated |
| `src/agent/agent_loop.rs:227` | `AgentLoop::run_with_thread_context` | 移除 `context`，改为 `messages: Vec<ChatMessage>` |
| `src/agent/agent_loop.rs:638` | `AgentLoop::run_with_thread` | 移除 `context`，改为 `messages: Vec<ChatMessage>` |
| `src/agent/agent_loop.rs:658` | `prepare_request_state` | 移除 `context`，`build_react_messages` 不再接收 system/memory |
| `src/agent/agent_loop.rs:877` | `build_react_messages` | 移除 `system_messages`/`memory_messages` 参数 |
| `src/agent/agent_loop.rs:1027` | `current_user_message_from_context` | 移除 |
| `src/agent/agent_loop.rs:1031` | `backfill_thread_context_from_context` | 移除 |
| `src/agent/agent_loop.rs:15` | imports | 移除 `ContextMessage` |
| `src/agent/worker.rs:438` | `build_context` | 移除 |
| `src/agent/worker.rs:329` | `handle_request` | 直接构建 `messages: Vec<ChatMessage>` |
| `src/context/mod.rs` | `MessageContext` / `ContextMessage` | 保留定义 |
| `tests/agent/agent_loop.rs` | `build_context` helper 及 19 个调用点 | 适配新签名 |
