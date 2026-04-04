## Context

当前 `AppConfig` 的事实边界有两个特点：

- 它在运行期是只读的，没有热更新或在线写回需求。
- 它只在进程启动早期会发生一次“最终化”动作，例如根据 CLI flag 调整 builtin MCP 配置。

但现状里，配置仍然被显式地一层层传递：

- `main.rs` 先 load 配置，再把子配置拆给 `AgentRuntime::from_config(...)`
- `AgentWorker::from_config(...)` 再次接收 `AppConfig`
- `build_provider(...)` 继续接收 `LLMConfig`

这带来两个问题：

1. 启动链路参数传递冗长，但这些参数并不是请求级或线程级动态状态。
2. “配置是进程级只读事实”没有被架构显式表达，反而像是普通依赖对象。

同时，这个 change 不能简单做成“全代码库到处直接读全局”：

- 测试里大量使用 `AppConfig::default()` 或临时 YAML 直接构造组件。
- 显式构造入口对于 UT、嵌入式调用和局部隔离仍然有价值。
- 如果把所有子模块都改成隐式读取全局，会把依赖关系隐藏掉，反而让测试和边界更差。

另外，配置构造入口本身也需要被收清楚：

- 正常启动需要一个默认加载入口
- 正常启动主要走文件路径配置
- 某些测试更适合直接喂 YAML 字符串
- 小型 UT 更适合用最小 builder，而不是写整段 YAML

如果没有正式入口，这些场景就会退化成直接 `serde_yaml::from_str::<AppConfig>(...)` 或在测试里硬拼结构体。

## Goals / Non-Goals

**Goals:**

- 提供进程级只读配置单例，表达“启动后配置不可变”的真实边界。
- 简化主启动链路，减少 `config` 在 runtime 装配过程中的传递。
- 明确配置初始化顺序：先 load/调整，再 install，install 后只读。
- 保留显式配置构造入口，避免测试和库边界全面退化成隐式全局依赖。
- 为默认加载、文件配置、字符串配置和 UT 快速构造分别提供正式入口。

**Non-Goals:**

- 不在这个 change 中引入配置热重载、动态刷新或运行期写配置能力。
- 不在这个 change 中删除所有 `from_config(...)` API。
- 不把 `Thread`、`Session`、`AgentLoop` 这类请求期或线程期状态改成全局单例。
- 不解决所有启动装配问题；这个 change 只聚焦配置访问边界。
- 不在这个 change 中清理 `--builtin-mcp` 这类额外启动功能；它先作为临时显式开关保留。

## Decisions

### 1. 使用进程级只读 `OnceLock<AppConfig>`，而不是可写单例或服务定位器

系统将引入一个进程级 `OnceLock<AppConfig>` 作为全局配置注册表。

原因：

- 配置是只读快照，直接存 `AppConfig` 比 `Mutex<AppConfig>` 或其他可写单例更符合事实。
- `OnceLock<AppConfig>` 可以直接返回 `&'static AppConfig`，不需要额外的引用计数或锁开销。
- 它天然表达“只能初始化一次”的约束。

拒绝方案：

- 方案 A: `Mutex<AppConfig>` / `RwLock<AppConfig>`
  原因: 给了运行期写入错觉，不符合“只读配置”边界。

- 方案 B: `OnceLock<Arc<AppConfig>>`
  原因: 也可行，但当前没有共享所有权需求；直接存 `AppConfig` 更简单。

### 2. 配置必须在 install 前最终化，install 后禁止修改

启动流程必须严格收敛为：

1. `AppConfig::load()`
2. 根据 CLI 或启动参数做最后一次调整
3. `install_global_config(config)`
4. 后续所有全局访问都只读

这样像 `--builtin-mcp` 这种启动期开关仍然有位置可放，但不会把“全局单例可写”带进运行期。
这里对 `--builtin-mcp` 的定位是：

- 它只是启动期临时显式开关
- 它不属于全局配置架构的核心能力
- 它后续可以单独删除，但不应阻塞这次全局只读配置收敛

拒绝方案：

- 方案 C: 先 install，再通过全局接口修改配置
  原因: 会立即破坏“只读单例”前提，也会把初始化顺序变得不可预测。

### 3. 全局访问只作为顶层运行时装配入口，不鼓励深层模块随手读取

这次 change 的主要目标是简化启动装配，而不是把所有依赖都改成隐藏的全局读取。

因此建议新增：

- `config::install_global_config(...)`
- `config::global_config()`
- `config::try_global_config()`
- 顶层组件的 `from_global_config()` 或等价辅助入口

但保留现有显式入口：

- `AgentRuntime::from_config(...)`
- `AgentWorker::from_config(...)`
- `build_provider(&LLMConfig)`

也就是说：

- 顶层 app 装配可以读全局
- 深层纯组件仍然优先保持显式参数

拒绝方案：

- 方案 D: 让所有模块都直接 `global_config()` 取配置
  原因: 会把依赖隐藏到函数体内部，测试隔离和模块边界都会恶化。

### 4. `AppConfig` 应显式提供四类构造入口

系统应明确提供四类配置构造方式：

- `AppConfig::load()`
  用于正常启动时按默认规则加载配置。
- `AppConfig::from_yaml_path(...)`
  用于正常启动和真实文件配置场景。
- `AppConfig::from_yaml_str(...)`
  用于测试、嵌入式调用和无需真实文件的配置解析场景。
- `AppConfig::builder_for_test()`
  用于 UT 快速构造最小配置。

其中 `AppConfig::load()` 应作为默认路径加载入口存在，但不能替代其他三种显式构造方式。

拒绝方案：

- 方案 E: 只提供 builder
  原因: 集成测试和贴近真实配置的场景仍然需要 YAML 入口。

- 方案 F: 只提供 YAML 入口
  原因: 小型 UT 会变得很啰嗦，字段覆盖也不够聚焦。

- 方案 G: 只保留 `load()`
  原因: 默认路径加载无法替代显式文件路径、字符串解析和测试 builder 这几种不同语义入口。

### 5. 公开配置构造 API 必须带文档注释，说明适用场景

这次 change 中新增或重命名的配置构造入口都应带有清晰的 `///` 注释，明确说明：

- 这个入口适用于什么场景
- 是否会做校验
- 是否会处理相对路径 / sidecar
- 与其他入口的区别

这样调用方不需要反向阅读实现猜语义。

### 6. 未初始化访问必须 fail fast，并提供可探测接口

全局配置访问需要两类 API：

- `global_config()`：未初始化时直接明确失败
- `try_global_config()`：供需要探测初始化状态的代码使用

这样可以让生产路径简单，也不会把“Maybe initialized”传播成大面积 `Option`/`Result` 污染。

### 7. 测试与嵌入场景继续保留显式配置构造，不引入全局 reset

不建议为测试引入 `reset_global_config_for_test()` 这类 API。

原因：

- 全局 reset 容易让测试对执行顺序敏感。
- 当前代码已经有比较完整的显式配置构造路径，继续保留更稳。

测试策略应该是：

- 需要全局初始化语义的测试，单独初始化一次并做串行约束
- 大多数 UT 继续走显式 `from_config(...)`

## Risks / Trade-offs

- [Risk] 全局配置会引入隐藏依赖的诱惑
  → Mitigation: 规范上只允许顶层运行时装配优先读取全局，深层组件保留显式参数入口。

- [Risk] 初始化顺序变得更严格，install 前后边界必须明确
  → Mitigation: 用单独的 `install_global_config(...)` 和 fail-fast 访问器把边界收清楚。

- [Risk] 某些测试如果强依赖全局初始化，可能互相干扰
  → Mitigation: 不删除显式构造路径；大部分测试继续使用局部配置，不依赖全局单例。

- [Risk] `enable_builtin_mcp(...)` 这类可变 API 看起来与“只读全局配置”冲突
  → Mitigation: 明确它只能用于 install 前的启动期调整，install 后不再允许调用。

## Migration Plan

1. 在 `config` 模块中新增全局只读注册与访问 API。
2. 调整 `main.rs`，将启动顺序改成“load -> adjust -> install -> build runtime”。
3. 为 `AgentRuntime`、`AgentWorker`、LLM provider 等顶层装配点增加基于全局配置的便捷入口。
4. 保留现有显式 `from_config(...)` 路径，先不删除。
5. 更新文档和测试，明确全局配置只读语义与 install 前后边界。
6. 将 `from_yaml_path()`、`from_yaml_str()`、`builder_for_test()` 的使用边界写入 API 注释与文档。
7. 将 `load()` 与其他三种构造入口的职责差异写入 API 注释与文档。

命名上统一要求来源显式：

- `AgentRuntime::from_global_config()`
- `AgentWorker::from_global_config()`
- `build_provider_from_global_config()`

不使用过于泛化的 `from_global()`，避免把“来源是全局配置”这个语义藏掉。

## Open Questions

- 是否需要同时提供 `global_agent_config()`、`global_llm_config()` 这类子配置便捷访问器，还是统一通过 `global_config()` 下钻即可。
- `logging::init_tracing_from_default_config()` 是否也要纳入同一套全局配置初始化顺序，还是继续走独立默认配置路径。
- `--builtin-mcp` 后续是否应该从 CLI 启动参数中移除，改为纯配置文件控制。
