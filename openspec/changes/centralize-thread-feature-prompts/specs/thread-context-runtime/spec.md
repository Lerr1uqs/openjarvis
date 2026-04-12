## ADDED Requirements

### Requirement: 稳定 feature prompt SHALL 直接写入线程初始化 `System` 前缀
系统 SHALL 在初始化阶段将稳定 feature prompt 直接写入 `ThreadContext.messages()` 的 `System` 前缀，而不是在 `ThreadContext` 中新增 `features_system_prompt` 之类的固定槽位。首版稳定 feature prompt SHALL 至少覆盖 `toolset catalog`、`skill catalog` 和 `auto_compact` 的稳定说明。

#### Scenario: 线程以固定顺序导出稳定 feature prompt
- **WHEN** 某个线程准备导出本轮发送给 LLM 的完整 messages
- **THEN** `ThreadContext.messages()` 会按稳定顺序以 `System` 前缀开头，再包含后续 conversation history
- **THEN** loop 不需要再单独拼接 toolset、skill、auto-compact 或 memory 的 prompt 向量

### Requirement: 系统 SHALL 通过统一的 feature 构造入口物化稳定 feature prompt
系统 SHALL 通过统一的 feature 构造入口，根据当前线程状态生成稳定 feature prompt。系统 SHALL NOT 再要求用 `FeaturePromptProvider` contract 和固定 prompt 槽位来维护这些稳定消息。

#### Scenario: 统一入口在初始化阶段写入稳定 feature prompt
- **WHEN** AgentLoop 在某一轮请求前需要刷新线程的 feature prompt
- **THEN** 系统会基于当前线程状态确定需要的稳定 feature prompt
- **THEN** 这些 prompt 会直接表现为 `ThreadContext.messages()` 开头的 `System` messages，而不是固定槽位

### Requirement: 基础 system prompt SHALL 继续由线程初始化前缀管理
系统 SHALL 将基础角色设定 system prompt 继续视为线程初始化阶段写入的稳定 `System` 前缀，而不是动态 feature prompt。该 system prompt SHALL 在新线程初始化时直接进入持久化消息序列开头，并 SHALL NOT 在每次 turn rebuild 时重复生成。

#### Scenario: rebuild feature prompt 不会重写基础 system 前缀
- **WHEN** 某个线程因为 feature 状态变化而刷新稳定 feature prompt
- **THEN** 线程初始化阶段固化的基础 system prompt 仍保持不变
- **THEN** 系统不会为此引入独立的固定槽位或覆盖整份持久化消息序列

### Requirement: feature 状态变化后 SHALL 通过 rebuild 生效，而不是追加历史 prompt
当线程 feature 开关、预算状态或 memory 命中结果发生变化时，系统 SHALL 通过刷新线程状态并在合适时机重新物化稳定 feature prompt 或 request-time live message 使其生效。系统 SHALL NOT 依赖新增固定 `features_system_prompt` 成员，且 SHALL NOT 通过向历史或 live chat 追加一条一次性的 system message 来表达 feature 状态变化。

#### Scenario: auto-compact 激活后重建 feature prompt
- **WHEN** 某个线程从 `auto_compact=off` 变为 `auto_compact=on`
- **THEN** 系统会刷新该线程的稳定 feature prompt
- **THEN** auto-compact prompt 会表现为线程稳定 `System` 前缀中的一部分
- **THEN** 系统不会通过向历史追加一次性 system message 来表达该状态变化

### Requirement: `auto_compact` SHALL 将稳定说明与动态容量信息分层
当线程启用 `auto_compact` 时，系统 SHALL 将该 feature 的稳定功能说明与动态上下文容量信息分成不同的消息来源。稳定说明 SHALL 作为稳定 `System` 前缀的一部分存在；动态容量信息 SHALL 作为可频繁刷新的 live message 存在。预算刷新 SHALL NOT 改写稳定说明部分。

#### Scenario: 上下文容量变化时只更新动态容量消息
- **WHEN** 某个启用了 `auto_compact` 的线程上下文容量发生变化
- **THEN** 系统会更新该线程的 auto-compact 动态容量 live message
- **THEN** auto-compact 的稳定说明 prompt 保持不变
- **THEN** 系统不会因为预算变化而重写该 feature 的稳定 system prompt

## MODIFIED Requirements

### Requirement: Agent loop SHALL 基于 `ThreadContext + current user input` 组装请求
系统 SHALL 让 AgentLoop 主链路接收 `ThreadContext` 与当前轮 user input，而不是接收由外部预组装的 `MessageContext`。发送给 LLM 的 messages SHALL 通过 `ThreadContext.messages()` 从线程内部统一导出；在导出前，AgentLoop SHALL 只负责触发稳定 feature prompt 的刷新、动态容量 live message 的刷新和 live chat messages 的追加，而 SHALL NOT 再手工拼装分散的 feature prompt 向量。

#### Scenario: worker 只传当前 user input，loop 只触发 feature rebuild
- **WHEN** worker 准备把某个线程请求交给 AgentLoop
- **THEN** worker 不需要先构造完整的 `MessageContext`
- **THEN** AgentLoop 会先刷新当前线程的稳定 feature prompt，再按预算状态刷新动态容量消息，最后把当前 user input 追加到 live chat messages
- **THEN** Router 不会在转发过程中主动操控 memory 或其他 message 上下文

### Requirement: request-time memory SHALL 保持动态注入而非线程初始化固化
系统 SHALL 将 memory 视为 request-time 的可选动态注入，而不是线程初始化时固定写入的独立 request context 结构。即使未来接入 memory provider，memory 的存在与内容也 SHALL 由统一的运行时逻辑决定并写入 `ThreadContext` 的 live memory 层，而不是由 Router 或线程创建阶段一次性固化。

#### Scenario: 命中 memory 时只更新 live memory 层
- **WHEN** 某一轮请求命中 memory provider 并需要向 LLM 注入 memory
- **THEN** 这些 memory messages 只会进入当前线程的 `live_memory_messages` 并参与本轮 request 组装
- **THEN** 它们不会被回写为线程初始化 `System` 前缀的永久内容
