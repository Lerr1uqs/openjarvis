## Why

当前 OpenJarvis 还没有真正的上下文压缩机制，线程历史一旦变长，只能依赖按消息数量裁剪的临时策略。这会在上下文接近模型上限时直接丢失任务目标、用户约束和已完成状态，且无法为模型提供可恢复的紧凑上下文。

随着 thread-managed toolsets、浏览器 sidecar 等能力进入线程上下文，token 压力会继续升高。现在需要把 compact 从“临时裁剪”提升为正式的线程级上下文管理能力，并为可选的 AutoCompact 预留模型自主管理入口。

## What Changes

- 新增线程级 `compact` 能力，对 `chat` 区域进行 token 预算检测与压缩，`system` 和 `memory` 不参与 compact。
- 引入 `CompactManager` 与 `CompactStrategy` 抽象，首版策略在到达阈值时将当前线程全部历史 chat turn 压缩成一个 compacted turn。
- 规定 compact 结果必须写回 message 历史中，首版以一个普通 chat turn 的两条 message 表示：
  - compacted `assistant` message：明确说明“这是压缩后的上下文”，并保留任务目标、用户约束、当前背景、当前规划、已完成、未完成和关键事实
  - follow-up `user` message：固定写入“继续”，让后续对话自然续接
- 首版 compact 后直接替换被压缩的旧 chat 历史，不保留原始 chat 在 active history 中；同时在设计上预留未来改成 archive / shadow copy 的空间。
- 新增上下文容量估算能力，允许按模型上下文窗口、预留输出和工具 schema 一起估算当前线程上下文占用比例。
- 新增可选的 `auto_compact` 特性：仅在开启时向模型注入上下文容量信息，并动态暴露 compact tool，让模型自行选择提早压缩时机。
- 修改工具可见性模型，使某些工具可以按线程运行时状态决定是否对模型可见，而不是仅由注册状态决定。

## Capabilities

### New Capabilities
- `chat-compact`: 对线程 `chat` 历史执行 token 感知的上下文压缩，并将压缩结果作为 compacted turn 写回消息历史。

### Modified Capabilities
- `thread-managed-toolsets`: 增加线程运行时工具可见性控制，使工具不仅能按 toolset 加载状态出现，也能按上下文预算和特性开关动态显隐。

## Impact

- Affected code: `src/session.rs`、`src/thread.rs`、`src/context.rs`、`src/agent/worker.rs`、`src/agent/agent_loop.rs`、`src/agent/tool/**`、`src/config.rs` 及映射测试。
- Affected architecture docs: `arch/system.md` 的 `agent context容量`、`compact`、`auto-compact` 章节需要同步更新。
- Persistence impact: thread 历史将新增 compact 产生的 compacted turn，并需要为未来 source archive / source retention 预留扩展点。
- Tool/runtime impact: `ToolRegistry` 的可见工具投影需要支持线程级动态显隐，供 `auto_compact` 控制 compact tool 的暴露。
- Config impact: 需要新增 compact 相关配置，例如 context window、阈值、预留输出和 `auto_compact` 开关。
