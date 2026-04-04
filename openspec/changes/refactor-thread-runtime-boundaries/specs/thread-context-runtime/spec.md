## MODIFIED Requirements

### Requirement: 系统 SHALL 在线程初始化时将稳定 system messages 直接写入 `Thread`
系统 SHALL 在目标线程第一次进入主执行链路前完成线程初始化。初始化 SHALL 将基础 system prompt 与 feature 生成的稳定 system prompt 直接写入 `Thread` 的持久化消息序列，而不是落到独立的 prefix 分层里。

#### Scenario: 新线程首次进入主链路时完成初始化
- **WHEN** 系统首次处理某个还没有持久化消息的线程
- **THEN** 会先构造该线程的稳定 system messages
- **THEN** 这些 system messages 会直接写入 `Thread`
- **THEN** 初始化后的线程会被持久化

### Requirement: `Thread` SHALL 只暴露单一持久化消息域
系统 SHALL 让 `Thread.messages()` 表示线程全部持久化消息，而不是分别导出 prefix / conversation 多段结构。线程恢复后 SHALL 直接还原这份稳定消息序列。

#### Scenario: 线程恢复时直接得到完整持久化消息
- **WHEN** session/store 恢复某个线程
- **THEN** 调用方会得到一份包含稳定 system messages 和后续历史消息的持久化消息序列
- **THEN** 调用方不需要再从多个 prefix 字段拼接线程历史

### Requirement: request-time live messages SHALL 只存在于 `ThreadRuntimeContext`
系统 SHALL 只允许 request-time `system` / `memory` / `chat` live messages 存在于 `ThreadRuntimeContext`。这些消息 SHALL NOT 被当作线程初始化内容写回持久化 `Thread`。

#### Scenario: 当前轮 live messages 不随线程恢复
- **WHEN** 某轮请求在 `ThreadRuntimeContext` 中追加了 live system / memory / chat messages
- **THEN** 这些消息只参与当前轮 request 组装
- **THEN** 后续重新加载 `Thread` 时不会恢复这些 live messages

### Requirement: compact SHALL 直接基于 message 序列工作
系统 SHALL 让 compact 首版直接处理消息序列。compact 输入 SHALL 是当前 working set 中全部非 `System` message；compact 写回时 SHALL 保留线程持久化的 `System` message，并用 compact replacement 覆盖原有非 system 持久化消息。

#### Scenario: compact 只替换非 system 持久化消息
- **WHEN** 某个线程触发 runtime compact 或模型主动调用 `compact`
- **THEN** compact 输入只包含当前 working set 中 role 不是 `System` 的消息
- **THEN** compact 结果回写后，原有持久化 `System` message 保持不变
- **THEN** 原有非 system 持久化消息被 replacement 覆盖

## ADDED Requirements

### Requirement: Turn SHALL 不再作为线程主消息边界
系统 SHALL 保留 Turn 这个兼容概念，但 SHALL NOT 再要求主链路依赖 turn slice、turn plan 或 `ConversationThread` 才能完成线程执行与 compact。

#### Scenario: 主链路直接围绕 message 序列工作
- **WHEN** 线程进入初始化、请求组装或 compact 主链路
- **THEN** 主链路直接围绕 `Thread` 的消息序列工作
- **THEN** 新代码不会再要求先构造 turn slice 才能执行 compact
