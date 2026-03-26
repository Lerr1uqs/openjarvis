## 1. 配置与预算估算

- [ ] 1.1 扩展 `llm` 与 `agent.compact` 配置，增加 context window、tokenizer、阈值、预留输出和 `auto_compact` 开关
- [ ] 1.2 新增上下文预算估算模块，按 `system`、`memory`、`chat`、visible tools 和预留输出计算线程请求占用
- [ ] 1.3 将现有 `max_messages_per_thread` 临时裁剪路径降级为兼容逻辑，并接入新的 compact 触发前置判断

## 2. Compact Manager 与策略

- [ ] 2.1 新增 `CompactManager`、`CompactStrategy` 和 `CompactionPlan` 抽象，并实现首版 `CompactAllChatStrategy`
- [ ] 2.2 实现 compact provider 调用与固定结构 summary prompt，确保产出 synthetic `user` 和 synthetic `assistant` 两条 message
- [ ] 2.3 为 compact 结果补充 synthetic 标识与最小持久化信息，便于后续识别和再次 compact

## 3. Thread 历史替换与上下文构造

- [ ] 3.1 扩展 thread/session 存储模型，使 compact 后的 synthetic turn 能写回 active chat history
- [ ] 3.2 实现首版“删除被 compact 的旧 chat 并插入 compacted synthetic turn”的 active history 替换逻辑
- [ ] 3.3 更新 worker / agent loop 的上下文构造流程，使 compacted synthetic turn 作为 chat history 继续参与后续对话和后续 compact

## 4. AutoCompact 与工具显隐

- [ ] 4.1 扩展 ToolRegistry 的线程级 visible tool projection，支持按运行时条件动态显隐工具
- [ ] 4.2 实现 compact tool 的条件暴露逻辑：仅在 `auto_compact` 开启且达到可见阈值时对模型可见
- [ ] 4.3 在 agent loop 中注入上下文容量信息，并保持 runtime hard-threshold compact 作为兜底机制

## 5. 测试与文档

- [ ] 5.1 在 `tests/session.rs`、`tests/thread.rs` 和对应 compact 测试中覆盖预算估算、synthetic turn 写回、历史替换和再次 compact
- [ ] 5.2 在 `tests/agent/tool/`、`tests/agent/agent_loop.rs` 中覆盖条件化工具可见性和 `auto_compact` 行为
- [ ] 5.3 同步更新 `arch/system.md` 的 `agent context容量`、`compact` 和 `auto-compact` 章节，使其与实现方案一致
