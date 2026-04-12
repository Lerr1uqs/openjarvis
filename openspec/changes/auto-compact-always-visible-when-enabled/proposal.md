## Why

当前 `chat-compact` 契约里仍保留“`auto_compact` 开启后，只有预算达到可见阈值才暴露 `compact` 工具”的旧语义。这和当前产品意图不一致：只要线程已启用 `auto_compact`，模型就应该立刻看到 `compact` 工具，预算相关信息最多只能影响提示内容，不能再决定工具是否存在。

## What Changes

- 修改 `chat-compact` 能力的 `auto_compact` 行为要求：`auto_compact` 一旦开启，`compact` 工具 SHALL 在当前线程的模型请求中立即可见。
- 移除“预算达到可见阈值才暴露 `compact` 工具”的旧要求，不再把工具可见性和预算阈值绑定。
- 明确上下文预算、容量提示或早期预警文案若继续存在，其职责仅限于帮助模型判断是否应尽快压缩，而不是控制 `compact` 工具是否出现在工具列表中。
- 为后续实现留出清理范围：运行时显隐逻辑、相关配置语义、测试断言以及引用旧要求的文档都需要对齐到新契约。

## Capabilities

### New Capabilities

### Modified Capabilities

- `chat-compact`: 将 `auto_compact` 的工具暴露语义从“达到阈值后可见”修改为“开启即对模型可见”

## Impact

- Affected specs: `openspec/specs/chat-compact/spec.md`
- Likely affected code later: `src/agent/agent_loop.rs`、`src/thread.rs`、`src/agent/feature/**`、`src/config.rs`
- Likely affected tests/documents later: `tests/**`、`model/auto-compactor.md` 以及任何仍描述“阈值后才可见”的说明文本
