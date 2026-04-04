## Why

当前 `AppConfig` 在启动链路里是只读事实，但仍然需要在 `main -> runtime -> worker -> provider` 之间不断下传，导致初始化路径冗长、构造参数膨胀，也让“启动时只需要一份配置快照”这个事实没有被架构直接表达。现在需要把配置收敛为进程级只读单例，简化启动装配，同时明确“配置只允许在安装前构造和调整，安装后不可修改”的边界。

## What Changes

- 新增一个进程级只读 `AppConfig` 全局注册表，用于保存已经完成加载和启动期调整的最终配置快照。
- 提供显式的全局配置安装与访问接口，要求配置只能安装一次；未初始化访问必须快速失败。
- 启动流程改为“load/mutate/finalize config -> install global config -> 构造 runtime/worker/router/channels”。
- 为顶层运行时组件提供基于全局配置的构造入口，减少主启动链路上的 `config` 传递。
- 对外提供四类正式配置构造入口：`load()`、`from_yaml_path()`、`from_yaml_str()`、`builder_for_test()`。
- 保留显式 `from_config(...)` / 直接传配置的测试与嵌入入口，避免单例污染单元测试和库边界。
- `--builtin-mcp` 继续保留为启动期临时显式开关，但它只允许在全局配置 install 前修改待安装配置，不构成长期配置架构能力。
- **BREAKING** 不允许在全局配置安装后继续修改 `AppConfig`；像 `--builtin-mcp` 这类启动期开关必须在安装前完成调整。

## Capabilities

### New Capabilities

- `global-runtime-config`: 提供进程级只读配置注册、一次性初始化、全局访问和启动期冻结边界。

### Modified Capabilities

- 无

## Impact

- Affected code: `src/config.rs`、`src/main.rs`、`src/agent/runtime.rs`、`src/agent/worker.rs`、`src/llm.rs` 以及相关测试。
- API impact: 会新增全局配置访问 API，并为部分运行时组件增加 `from_global_config()` 或等价入口；同时公开 `load()`、`from_yaml_path()`、`from_yaml_str()`、`builder_for_test()` 四类正式配置构造方式。
- Runtime impact: 启动流程更简单，但初始化顺序会变得更严格，配置必须先完成最终化再安装到全局；`--builtin-mcp` 保留为临时启动开关，后续可单独清理。
