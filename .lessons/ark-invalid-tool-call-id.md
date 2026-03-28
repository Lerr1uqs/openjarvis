# ark Invalid tool_call_id bug 复盘

## 现象

长串 tool call 之后，后续请求调用 ark / kimi-k2.5 时返回：

- `Invalid tool_call_id: read:7. No matching tool call exists.`

## Root Cause

问题不是 compact 丢了 tool call。

真正原因是旧的 session 历史保留策略还在按 `message` 数量硬裁剪线程历史：

- assistant 的 `tool_calls` message 和后续的 `tool_result` message 被当成普通消息处理
- 裁剪时可能把前面的 assistant tool-call message 删掉
- 但后面的 `tool_result(tool_call_id=read:7)` 还保留下来
- 下一轮请求把这段“悬空 tool_result”发给 ark
- ark 会严格校验 `tool_call_id` 必须能在前文找到对应 tool call，于是直接报错

所以本质是：

- 历史裁剪破坏了 tool call / tool result 的结构完整性

## 解决方案

这次修复分两部分：

1. 删除 legacy `max_messages_per_thread` 裁剪链路，不再按 message 数量截断线程历史。
2. compact 保留为唯一正式的上下文收缩手段，避免再产生结构损坏的历史。

另外补了一个 compact mock 入口：

- 新增 `agent.compact.mock_compacted_assistant`
- 配置后直接走 `StaticCompactProvider`
- 可以本地稳定验证 compact 流程，不需要真的再调一次 compact provider

## 结论

以后这类带结构约束的历史数据不能做“裸 message 裁剪”：

- 要么完整保留
- 要么走结构化 compact
- 不能把一轮内部的 tool call / tool result 对拆开
