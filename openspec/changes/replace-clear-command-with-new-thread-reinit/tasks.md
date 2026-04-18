## 1. OpenSpec 与重初始化契约

- [x] 1.1 新增 `thread-reinitialize-command` spec，定义 `/new` 的行为、`/clear` 移除，以及保留当前 thread kind / child identity 的要求
- [x] 1.2 修改 `thread-context-runtime` spec，明确显式重初始化完成后必须立即恢复稳定 `System` 前缀，而不是留下空 thread

## 2. 命令与运行时实现

- [x] 2.1 在 `ThreadRuntime` 中新增面向当前 thread 的重初始化入口，复用现有初始化链路并保留 child thread identity
- [x] 2.2 将内建线程命令从 `/clear` 切换为 `/new`，并通过 router/session 主链路把运行时初始化依赖接入命令执行路径
- [x] 2.3 删除 `/clear` 注册和旧成功文案，补齐 `/new` 的日志、usage 和运行时缺失时的显式错误

## 3. 测试验证

- [x] 3.1 更新 `tests/command.rs`，覆盖 `/new` 成功重初始化、system messages 恢复、thread kind 不丢失，以及 `/clear` 变成 unknown command
- [x] 3.2 更新 `tests/router.rs` 或等价链路测试，覆盖 `/new` 不触发 agent dispatch 且会把当前线程恢复为 initialized 状态
