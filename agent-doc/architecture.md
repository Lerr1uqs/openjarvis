# OpenJarvis 模块关系

## 当前分层

当前代码按下面几层组织：

1. `channels`
   - 对接外部平台
   - 负责把平台消息转换成统一 `IncomingMessage`
   - 负责把统一 `OutgoingMessage` 发回外部平台
2. `router`
   - 持有全部 channel 的双向 `mpsc`
   - 负责入站去重和出站分发
   - 不处理平台协议细节，也不处理 LLM 细节
3. `agent`
   - 负责单条消息的 agent 执行
   - 负责把 `IncomingMessage` 放进 `Session / Thread / Context`
   - 负责调用 `LLMProvider`
4. `session / thread / context`
   - `Session` 管用户级上下文空间
   - `Thread` 管单条会话链
   - `Context` 管送给 LLM 的消息组织
5. `llm`
   - 负责具体模型协议适配
   - 当前支持 `mock` 和 `openai_compatible`

## 模块依赖方向

依赖方向保持单向：

`channels -> model`

`router -> channels + agent + model`

`agent -> session + context + llm + model`

`session -> thread + context + model`

`thread -> 基础数据结构`

`llm -> provider 实现`

原则是：

- channel 不感知 agent 内部状态
- router 不感知平台实现细节
- agent 不感知具体平台 SDK
- session/thread/context 不直接操作外部 channel

## Channel 抽象

`channels` 模块现在有统一基类 trait：

- `Channel::name`
- `Channel::on_start`
- `Channel::start`
- `Channel::check_health`
- `Channel::on_stop`

当前真正用到的是：

- `name`
- `on_start`
- `start`

其中 `start` 接收 `ChannelRegistration`：

- `incoming_tx`
  - channel 把平台入站消息写入 router
- `outgoing_rx`
  - channel 从 router 读取需要发回平台的消息

这保证每个 channel 都遵守同一条边界：

`平台协议 <-> Channel trait <-> Router`

## Context / Session / Thread 关系

关系是：

`Session(channel + user_id) -> Thread(thread_id/default) -> Turn(user/assistant) -> MessageContext`

具体含义：

- `Session`
  - 一个用户在某个 channel 下的长期会话空间
- `Thread`
  - 这个用户下面的一条对话链
  - 有平台 thread_id 就用平台的
  - 没有就先落到 `default`
- `Turn`
  - 一次用户输入和一次 assistant 回复组成的一轮
- `MessageContext`
  - 把 `system / memory / chat` 整理成当前给 LLM 的输入

## 当前实际调用链

### 入站

`Feishu long connection`

`-> FeishuChannel`

`-> ChannelRouter::handle_incoming`

`-> AgentWorker::handle_message`

`-> SessionManager::begin_turn`

`-> MessageContext::render_for_llm`

`-> LlmProvider::generate`

`-> SessionManager::complete_turn`

`-> ChannelRouter::dispatch_outgoing`

`-> FeishuChannel::deliver_outgoing`

### 出站

`OutgoingMessage`

`-> router 按 channel 名称查找对应 sender`

`-> channel 收到消息后决定如何发送`

`-> 当前 feishu 会先打 Typing reaction，再发文本`

## 当前实现边界

当前这版只实现了最小闭环，所以有几个明确边界：

- `Session / Thread / Context` 目前是内存态
- 还没有持久化
- 还没有 memory 检索
- 还没有 command / tool / hook / sandbox
- `Context` 目前先把线程历史渲染成一段文本再交给 LLM
- 后续如果升级成真正多消息协议，可以直接从 `MessageContext` 扩展
