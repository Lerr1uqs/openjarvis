## Why

当前内建线程命令里的 `/clear` 会把当前 thread 直接重置成空初始状态，导致稳定 `System` 前缀、当前 `ThreadAgentKind` 对应的初始化 truth 和 child thread 身份都暂时消失。这样虽然“清空了历史”，但线程在下一次真正进入初始化前处于一个不完整状态，不符合当前 thread-first runtime 已经建立的能力边界真相。

现在需要把这个命令语义改成“当前 thread 开一个新的会话轮廓，但立即重新初始化”，让线程在命令执行完成后仍然保持一个完整、可解释、与当前 kind 一致的 initialized 状态。

## What Changes

- **BREAKING** 删除线程命令 `/clear`，不再允许把当前 thread 清成空消息状态。
- 新增线程命令 `/new`，用于清空当前 thread 的非稳定历史与线程级运行态，并立即按当前 thread 的 `ThreadAgentKind` 重新写入稳定初始化前缀。
- 明确 `/new` 对 child thread 也适用；执行后 SHALL 保留当前 child thread identity，而不是把 browser child thread 重置成不带身份的 main 空线程。
- 复用现有 `ThreadRuntime::initialize_thread(...)` 初始化链路完成重初始化，避免命令层再维护第二套 prompt / feature / toolset 初始化逻辑。
- 补齐命令与 router UT，覆盖 `/new` 成功、`/clear` 变成 unknown command、重初始化后 system messages 恢复、thread kind 不丢失以及“不触发 agent dispatch”的行为。

## Capabilities

### New Capabilities
- `thread-reinitialize-command`: 在线程命令层提供 `/new`，把当前 thread 重置并立即重新初始化为同一个 agent kind 的新会话。

### Modified Capabilities
- `thread-context-runtime`: 明确显式重初始化完成后，线程必须立即恢复与当前 `ThreadAgentKind` 一致的稳定 `System` 前缀，而不是留下空 thread 状态。

## Impact

- Affected code: `src/command.rs`、`src/router.rs`、`src/session.rs`、`src/thread.rs`、`src/main.rs` 以及对应测试 `tests/command.rs`、`tests/router.rs`。
- Runtime impact: `/clear` 被移除，线程重置语义改为立即重初始化；执行完成后的 thread 会保持 initialized 状态。
- API impact: 用户可见线程命令从 `/clear` 切换为 `/new`。
- Verification impact: 需要新增回归测试，覆盖命令返回、thread kind/child identity 保留、system prefix 恢复和 router 不派发 agent。
