## 1. 线程访问生命周期拆分

- [x] 1.1 在 `src/session.rs` 中新增 `create_thread`、`load_thread`、`lock_thread` 公开入口，并拆出“纯读取恢复”和“create-or-restore”两层 helper
- [x] 1.2 删除 `load_or_create_thread`、`load_thread_context`、`lock_thread_context` 这些混合或旧命名公开入口，统一迁移调用方到新生命周期接口
- [x] 1.3 补齐关键调试日志，覆盖 create/load/lock 命中 cache、store 恢复、store miss 创建和线程缺失分支

## 2. 初始化入口收口到 `initialize_thread`

- [x] 2.1 在 `src/thread.rs` 中新增 `ThreadAgentKind` / `ThreadAgent`，持久化线程 agent 类型与该类型绑定的默认工具集合
- [x] 2.2 调整 `ThreadRuntime::initialize_thread` 签名与实现，让它根据 `ThreadAgentKind` 选择预定义 system prompt 和默认工具绑定，并保持为唯一初始化入口
- [x] 2.3 让已初始化线程恢复后以持久化 `ThreadAgent` 为真相，重复 `create_thread` 不再改写既有稳定前缀，只记录 agent 类型不一致日志
- [x] 2.4 将 `Main` / `Browser` 的预定义 system prompt 模板迁移到随程序打包的 markdown 文件，移除 runtime/config 自定义 thread prompt 入口，并补齐对应 spec 与回归测试

## 3. 主链路迁移与回归

- [x] 3.1 将 `src/router.rs`、`src/agent/worker.rs` 迁移到 `create_thread(..., ThreadAgentKind::Main)`、`load_thread`、`lock_thread`
- [x] 3.2 更新 `tests/session.rs`、`tests/feature_runtime.rs`、`tests/agent/worker.rs`、`tests/router.rs` 等用例到新 API，并补齐缺失的边界断言
- [x] 3.3 新增回归测试，覆盖 `Browser` agent 的预定义 prompt + browser toolset 绑定、`load_thread` / `lock_thread` miss 不创建线程，以及重复 create 不改写已初始化线程
