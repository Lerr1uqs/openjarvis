# Session

## 定位

- `Session` 是 `channel + user_id` 维度的长期会话空间。
- `SessionManager` 是线程解析、热缓存和持久化写入的统一边界。

## 边界

- 负责把外部消息解析成稳定线程身份，并管理可变 `ThreadContext` 热缓存与持久化快照。
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
- thread-level lock
  `SessionManager` 在内存热缓存里按 thread 维护独立 mutex，允许外部按 locator 锁定并修改 live `ThreadContext`。

## 核心能力

- 首次命中时创建 session 和 thread。
- cache miss 时从 store 懒加载 `ThreadContext`。
- 通过 thread 级锁提供 live `ThreadContext` 修改入口，而不是只暴露 detached snapshot。
- write-through 持久化 thread 快照和外部消息去重记录。
- 通过 revision/CAS 做冲突恢复，避免旧快照覆盖新状态。
