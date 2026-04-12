# Session

## 定位

- `Session` 是 `channel + user_id` 维度的长期会话空间。
- `SessionManager` 是线程解析、thread handle 热缓存和 thread store 访问边界。

## 边界

- 负责把外部消息解析成稳定线程身份，并管理 live `Thread` handle 热缓存与 thread 快照恢复。
- 不负责平台入口 dedup，不负责 turn/finalized snapshot 提交。
- 不负责 LLM、工具执行、平台通信。
- 不定义线程内部语义，线程内部模型由 `thread.md` 负责。

## 关键概念

- `SessionKey`
  `channel + user_id` 的稳定键。
- `thread_key`
  `user:channel:external_thread_id`，用于稳定派生 internal thread id。
- `ThreadLocator`
  已解析的线程身份，包含 `session_id / external_thread_id / thread_id`。
- `ThreadHandle`
  对外暴露的轻量线程访问入口，负责驱动 thread 内部的消息与状态原子持久化。
- `ThreadStore`
  线程快照持久化后端，当前实现有 memory 和 sqlite。

## 核心能力

- 首次命中时创建 session 和 thread。
- cache miss 时从 store 懒加载 live `Thread` handle 所需的正式快照。
- 为目标线程提供精炼的 handle 管理入口，而不是暴露额外的 turn 提交接口。
- 通过 revision/CAS 做冲突恢复，避免旧快照覆盖新状态。

## 验收标准

- `SessionManager` 不拥有 dedup 状态，不拥有 turn/finalized snapshot 提交接口。
- `Session/Thread` 对外接口保持小而少：主链路只保留线程解析、handle 获取和 thread-owned 原子持久化入口。
