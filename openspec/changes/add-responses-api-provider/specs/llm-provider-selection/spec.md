## ADDED Requirements

### Requirement: 系统 SHALL 支持多 provider LLM 配置与 active provider 选择
系统 SHALL 在 `llm` 配置段中支持以下结构：

- `active_provider`: 当前启用的 provider 名称
- `providers`: `provider_name -> provider profile` 的映射

`active_provider` SHALL 精确指向 `providers` 中的一个已声明 provider profile。系统启动、全局配置安装和显式 provider 构造 SHALL 基于该 active provider 的已解析 profile 工作，而 SHALL NOT 依赖 map 遍历顺序或其他隐式默认项。

#### Scenario: 通过 active provider 选择 Responses provider
- **WHEN** 配置中声明 `llm.active_provider = "dashscope-responses"`，且 `llm.providers.dashscope-responses` 存在
- **THEN** 运行时会使用 `dashscope-responses` 对应的 profile 构造 LLM provider
- **THEN** 其他已声明但未激活的 provider 不会被当前主链路使用

#### Scenario: active provider 未命中已声明 profile 时配置校验失败
- **WHEN** `llm.active_provider` 指向一个未出现在 `llm.providers` 中的 provider 名称
- **THEN** 配置校验显式失败
- **THEN** 错误信息会指出缺失的 active provider 名称

### Requirement: provider profile SHALL 显式声明协议与该协议所需的运行参数
每个 `llm.providers.<name>` profile SHALL 至少支持以下字段：

- `protocol`
- `model`
- `base_url`
- `api_key` 或 `api_key_path`
- `context_window_tokens`
- `max_output_tokens`
- `tokenizer`
- 可选 `headers`

当 `protocol = "mock"` 时，profile 还 SHALL 支持 `mock_response`；当 `protocol` 为真实网络协议时，系统 SHALL 使用该 profile 自己的鉴权、URL、预算和 header 配置发起请求，而 SHALL NOT 回退读取其他 provider profile 的同名字段。

#### Scenario: 每个 provider 使用自己的鉴权与 base_url
- **WHEN** 配置中同时存在 `anthropic-prod` 和 `dashscope-responses` 两个 provider profile，且二者的 `base_url` 与 `api_key_path` 不同
- **THEN** 系统在切换 active provider 后会使用对应 profile 自己的 `base_url` 与鉴权信息
- **THEN** 不会把另一个 provider profile 的鉴权或 URL 混入当前请求

#### Scenario: provider profile 可以声明静态额外请求头
- **WHEN** `llm.providers.dashscope-responses.headers` 中声明了额外 header
- **THEN** 该 provider 发起网络请求时会携带这些 header
- **THEN** 未声明这些 header 的其他 provider 不受影响

### Requirement: 系统 SHALL 兼容旧单 provider `llm` 配置并归一化为统一视图
系统 SHALL 继续接受当前单 provider 平铺 `llm` 配置写法。若用户未声明 `llm.providers`，系统 SHALL 将旧配置归一化为一个隐式 provider profile，并生成稳定的 resolved 视图用于后续 provider 构造。该兼容路径 SHALL 保留现有 `protocol`、`model`、`base_url`、`api_key(_path)`、`mock_response`、`context_window_tokens`、`max_output_tokens`、`tokenizer` 语义。

#### Scenario: 旧配置会被归一化为隐式 active provider
- **WHEN** 用户只配置旧格式的单 provider `llm.protocol/model/base_url/...`
- **THEN** 系统仍可以成功加载配置
- **THEN** 后续 provider 构造会使用归一化后的隐式 active provider 视图

### Requirement: 系统 SHALL 拒绝歧义的混合 LLM 配置模式
系统 SHALL 要求用户在“旧单 provider 平铺配置”和“`active_provider + providers` 多 provider 配置”之间二选一。若用户显式同时提供两套来源且它们都参与 provider 解析，系统 SHALL 视为歧义配置并拒绝启动，而不是静默决定优先级。

#### Scenario: 同时声明旧平铺字段和 providers map 时失败
- **WHEN** 用户在同一份配置里既显式声明旧 `llm.protocol/model/...`，又声明 `llm.active_provider` 与 `llm.providers`
- **THEN** 配置校验显式失败
- **THEN** 错误信息会提示用户只保留一种 LLM 配置模式
