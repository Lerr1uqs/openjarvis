# Tool Role 归一重构

- [ ] 将 `ChatMessageRole::Toolcall` 和 `ChatMessageRole::ToolResult` 重构为单一 `ChatMessageRole::Tool`

## 背景

当前 `ChatMessage` 与 OpenAI message 协议没有做到完全同构。
`Toolcall` 是内部额外引入的 role，导致需要在 LLM adapter 层做额外聚合/适配，这会破坏“正式 message 真相即协议真相”的目标。

## 目标

- `ChatMessage` 直接对齐 OpenAI 可接受的 message 结构
- 删除内部自定义的 `Toolcall` / `ToolResult` 二分语义
- 用单一 `Tool` role 承载工具相关消息
- 取消为了兼容现有 role 设计而增加的协议适配逻辑

## 验收点

- `ChatMessageRole` 中不再存在 `Toolcall`
- `ChatMessageRole` 中不再存在 `ToolResult`
- agent loop 仍然按 message 粒度 commit / persist / dispatch
- LLM 请求序列化不再需要合并 `Toolcall` message 的补丁逻辑
- thread / session / router / compact / command / tests 全量切到新 role 语义
