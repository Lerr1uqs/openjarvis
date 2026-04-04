## ADDED Requirements

### Requirement: 系统 SHALL 提供进程级只读 `AppConfig` 全局快照
系统 SHALL 提供一个进程级只读 `AppConfig` 全局快照，作为启动完成后全局配置的唯一共享事实来源。该快照 SHALL 在同一进程生命周期内最多安装一次，并在安装完成后只允许只读访问。

#### Scenario: 启动后安装全局配置快照
- **WHEN** 进程完成配置加载与启动期调整
- **THEN** 系统会安装一个全局只读 `AppConfig` 快照
- **THEN** 后续运行时组件可以读取同一份配置事实
- **THEN** 系统不会在运行期替换这份配置快照

### Requirement: 配置 SHALL 在 install 前完成最终化
系统 SHALL 要求所有启动期配置调整在全局配置安装前完成。安装后的全局配置 SHALL NOT 再被修改；像 CLI flag 驱动的 builtin MCP 开关这类动作 SHALL 在 install 前完成。

#### Scenario: builtin MCP 在 install 前完成注入(也就是写到配置文件中 加载文件的时候自动注入)
- **WHEN** 用户通过启动参数要求启用 builtin MCP
- **THEN** 系统会先把该调整应用到待安装的 `AppConfig`
- **THEN** 之后才把最终配置安装到全局
- **THEN** install 之后不会再修改全局配置

### Requirement: `--builtin-mcp` SHALL 只作为临时启动期开关存在
系统 MAY 暂时保留 `--builtin-mcp` 这类额外启动开关，但该开关 SHALL 只作为启动期显式 override 存在。它 SHALL NOT 变成运行期可写配置接口，也 SHALL NOT 成为全局只读配置架构的长期依赖能力。

#### Scenario: builtin MCP 只影响待安装配置
- **WHEN** 用户通过 `--builtin-mcp` 启动程序
- **THEN** 系统只会在 install 前修改待安装的 `AppConfig`
- **THEN** install 后不会再通过该开关修改全局配置
- **THEN** 该开关后续可以被独立移除，而不影响全局只读配置能力

### Requirement: 运行时装配 SHALL 可以基于全局配置完成
系统 SHALL 允许顶层运行时装配基于全局只读配置完成，而不必继续在主启动链路中层层传递 `AppConfig`。至少 runtime、worker 和 provider 这些顶层装配入口 SHALL 可以通过全局配置构造；当系统为这些入口提供便捷构造 API 时，命名 SHALL 显式体现来源是 global config，而不是使用泛化命名。

#### Scenario: 主启动链路不再层层下传 config
- **WHEN** 主程序开始构造 runtime、worker 和 channel router
- **THEN** 顶层装配代码可以直接读取全局配置
- **THEN** 主启动链路不需要继续把同一份 `AppConfig` 逐层传递下去

#### Scenario: 全局配置构造入口名称显式表达来源
- **WHEN** 系统为 `AgentRuntime`、`AgentWorker` 或 provider 增加全局配置便捷构造入口
- **THEN** 这些入口会采用 `from_global_config()`、`build_provider_from_global_config()` 或等价显式命名
- **THEN** 系统不会使用语义过泛的 `from_global()` 命名

### Requirement: 未初始化的全局配置访问 SHALL 快速失败
系统 SHALL 为全局配置访问提供明确的未初始化失败语义。调用方如果在配置安装前访问全局配置，系统 SHALL 快速失败并给出清晰错误，而不是静默返回一份默认配置。

#### Scenario: install 前访问全局配置会失败
- **WHEN** 某个运行时代码在全局配置尚未安装时访问全局配置
- **THEN** 系统会立即返回显式失败
- **THEN** 调用方不会得到隐式默认配置或空配置

### Requirement: 显式配置构造入口 SHALL 继续可用
系统 SHALL 保留显式 `from_config(...)` 或等价配置构造入口，用于测试、嵌入式使用和局部隔离场景。全局只读配置访问 SHALL 作为顶层装配简化手段，而 SHALL NOT 成为唯一配置获取方式。

#### Scenario: 单元测试继续使用局部配置
- **WHEN** 某个单元测试需要用临时 YAML 或 `AppConfig::default()` 构造组件
- **THEN** 它仍然可以通过显式配置入口完成构造
- **THEN** 该测试不需要依赖全局配置单例

### Requirement: `AppConfig` SHALL 提供四类正式构造入口
系统 SHALL 为 `AppConfig` 提供四类正式构造入口，分别覆盖默认加载、文件配置、字符串配置和 UT 快速构造场景。至少 SHALL 包含：

- `load()`
- `from_yaml_path(...)`
- `from_yaml_str(...)`
- `builder_for_test()`

系统 SHALL NOT 只依赖 `load()` 作为唯一公开配置构造方式。

#### Scenario: 正常启动通过默认规则加载配置
- **WHEN** 正常应用启动且调用方不显式指定配置文件路径
- **THEN** 系统可以通过 `load()` 按默认规则加载配置
- **THEN** 该入口会继续走统一配置解析和校验流程

#### Scenario: 正常启动从 YAML 文件路径加载配置
- **WHEN** 正常应用启动需要从配置文件加载配置
- **THEN** 系统会通过 `from_yaml_path(...)` 或等价正式入口解析配置
- **THEN** 该入口会执行统一配置校验

#### Scenario: 测试通过 YAML 字符串构造配置
- **WHEN** 测试或嵌入式场景希望直接提供一段 YAML 字符串
- **THEN** 系统可以通过 `from_yaml_str(...)` 解析出 `AppConfig`
- **THEN** 调用方不需要直接使用裸 `serde_yaml::from_str::<AppConfig>(...)`

#### Scenario: 单元测试快速构造最小配置
- **WHEN** 单元测试只需要覆盖少数字段，而不想写完整 YAML
- **THEN** 调用方可以通过 `builder_for_test()` 构造最小可用配置
- **THEN** 该场景不需要依赖全局配置单例

### Requirement: 公开配置构造入口 SHALL 通过注释说明语义边界
系统 SHALL 为公开的配置构造入口提供文档注释，并明确说明各自适用场景与语义边界。注释 SHALL 至少覆盖“适合什么场景”“是否校验”“是否处理路径/sidecar”“与其他入口的区别”。

#### Scenario: 调用方可以直接从注释理解入口差异
- **WHEN** 调用方查看 `load()`、`from_yaml_path(...)`、`from_yaml_str(...)` 或 `builder_for_test()` 的注释
- **THEN** 它可以直接理解每个入口的适用场景和处理语义
- **THEN** 它不需要通过阅读底层实现来判断该使用哪个入口
