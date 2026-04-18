## 1. Subagent Tool 语义重整

- [x] 1.1 调整 `spawn_subagent` 参数与执行流程，使其在创建或复用 child thread 后立即执行首个 task 并返回结果
- [x] 1.2 调整 `send_subagent` 为仅支持已存在 `persist` child thread 的后续交互，不再隐式创建 child thread
- [x] 1.3 调整 `close_subagent` 与 `list_subagent` 的模式语义，使 `yolo` 不再依赖 `send/close`

## 2. Parent `/new` 级联重初始化

- [x] 2.1 扩展 `ThreadRuntime::reinitialize_thread(...)`，在 parent thread 上级联 reset/reinit 全部 `persist` child thread
- [x] 2.2 保持 child thread 自己执行 `/new` 时只重置自己，并补充相关日志

## 3. 测试与验证

- [x] 3.1 更新 subagent 工具与 main->subagent roundtrip 的 UT，覆盖 `spawn` 首轮执行、`send` persist-only、`yolo` 单次回收
- [x] 3.2 补充 `/new` 级联 persist child thread 的 UT
- [x] 3.3 运行 `cargo fmt`、`cargo test` 与 `openspec validate align-subagent-lifecycle-with-spawn-modes --type change`
