## 1. Thread Prompt 模型收口

- [x] 1.1 在 `ThreadContext` 中引入固定的 `features_system_prompt` 槽位，并更新 `messages()` 的稳定导出顺序
- [x] 1.2 保持基础 system prompt 继续由线程初始化 snapshot 管理，不把它并入动态 feature rebuild
- [x] 1.3 为线程 feature 状态变化补齐统一的 `features_system_prompt` rebuild 入口

## 2. Feature Prompt Provider 抽象

- [x] 2.1 新增统一的 `FeaturePromptProvider` trait，并定义 provider 的输入输出 contract
- [x] 2.2 为 toolset catalog、skill catalog、auto-compact 和 memory 分别实现固定 provider
- [x] 2.3 明确 provider 只产出消息，不直接持有或修改 `ThreadContext` 之外的历史状态
- [x] 2.4 将 auto-compact 的稳定说明保留在 `features_system_prompt`，并通过 `AutoCompactor` 单独注入动态容量消息，保证预算刷新不重写稳定 system prompt

## 3. AgentLoop 迁移

- [x] 3.1 调整 `AgentLoop`，在请求前只触发 feature rebuild 和 live chat append，不再手工拼装 feature prompt 向量
- [x] 3.2 移除 loop 中分散的 system prompt build helper，保留 budget/compact 的时机判断但不保留 prompt 拼装职责
- [x] 3.3 在 auto-compact 状态变化时重建稳定 feature 槽位，在预算刷新后通过 `AutoCompactor` 更新动态容量提示而不是追加历史 prompt

## 4. 验证与文档

- [x] 4.1 新增/更新 thread UT，覆盖 fixed feature 槽位的导出顺序和 rebuild 语义
- [x] 4.2 更新 agent loop UT，覆盖 toolset/skill/auto-compact/memory 通过 `FeaturePromptProvider` 注入后的行为
- [x] 4.3 更新 `model/thread.md`、`model/agent/loop.md` 等文档，说明 persisted snapshot / persisted feature state / `features_system_prompt` / live system / live memory / live chat 的分层
