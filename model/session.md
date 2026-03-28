# Session

## 定位

- `Session` 是 `channel + user_id` 维度的长期会话空间。
- `SessionManager` 是线程解析、热缓存和持久化写入的统一边界。

## 边界

- 负责把外部消息解析成稳定线程身份，并读写 `ThreadContext` 快照。
- 不负责 LLM、工具执行、平台通信。
- 不定义线程内部语义，线程内部模型由 `thread.md` 负责。

## 关键概念

- `SessionKey`
  `channel + user_id` 的稳定键。
- `thread_key`
  `user:channel:external_thread_id`，用于稳定派生 internal thread id。
- `ThreadLocator`
  已解析的线程身份，包含 `session_id / external_thread_id / thread_id`。
- `SessionStore`
  持久化后端，当前实现有 memory 和 sqlite。
- `SessionStrategy`
  历史保留策略；当前仍保留兼容裁剪能力，但主路径已经是 compact 优先。

## 核心能力

- 首次命中时创建 session 和 thread。
- cache miss 时从 store 懒加载 `ThreadContext`。
- write-through 持久化 thread 快照和外部消息去重记录。
- 通过 revision/CAS 做冲突恢复，避免旧快照覆盖新状态。

## 使用方式

- Router 先调用 `load_or_create_thread` 得到 `ThreadLocator`。
- 线程执行前调用 `load_thread_context` 或 `load_thread_state`。
- turn 完成后调用 `store_turn_with_thread_context`；只改线程状态时调用 `store_thread_context`。
