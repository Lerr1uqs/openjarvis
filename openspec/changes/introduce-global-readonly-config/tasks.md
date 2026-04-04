## 1. 全局配置注册

- [x] 1.1 在 `src/config.rs` 中新增进程级只读全局配置注册与访问 API
- [x] 1.2 明确 install 前后边界，确保配置只能安装一次，未初始化访问会显式失败
- [x] 1.3 调整 `AppConfig` 启动期可变动作的边界说明，确保 install 后不再修改配置
- [x] 1.4 明确 `--builtin-mcp` 只作为 install 前的临时启动期开关保留
- [x] 1.5 为 `AppConfig` 增加 `load()`、`from_yaml_path()`、`from_yaml_str()`、`builder_for_test()` 四类正式构造入口

## 2. 启动链路收敛

- [x] 2.1 调整 `src/main.rs`，将启动顺序收敛为 `load -> adjust -> install -> build`
- [x] 2.2 为 `AgentRuntime::from_global_config()`、`AgentWorker::from_global_config()` 和 `build_provider_from_global_config()` 增加基于全局配置的便捷构造方式
- [x] 2.3 在主启动链路中移除不必要的 `AppConfig` 参数层层传递

## 3. 测试与文档

- [x] 3.1 保留显式 `from_config(...)` 路径，并补充“无需依赖全局配置”的测试覆盖
- [x] 3.2 增加全局配置初始化行为测试，覆盖“一次安装成功、重复安装失败、install 前访问失败”
- [x] 3.3 为 `load()`、`from_yaml_path()`、`from_yaml_str()`、`builder_for_test()` 补充 `///` 注释，明确各自场景和语义边界
- [x] 3.4 更新架构文档和 model 文档，明确全局只读配置访问层与配置构造入口的职责边界
