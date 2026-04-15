## ADDED Requirements

### Requirement: `browser` toolset SHALL 暴露诊断查询工具
当当前线程加载 `browser` toolset 后，系统 SHALL 在该线程的可见工具列表中额外暴露只读诊断工具 `browser__console`、`browser__errors` 和 `browser__requests`。这些工具 SHALL 复用当前线程已有的 browser session，而 SHALL NOT 启动独立的诊断会话或新的 toolset。

#### Scenario: 加载 browser 后诊断工具可见
- **WHEN** 当前线程成功加载 `browser` toolset
- **THEN** 当前线程后续可见工具列表中包含 `browser__console`、`browser__errors` 和 `browser__requests`
- **THEN** 未加载 `browser` 的其他线程不会看到这些诊断工具

### Requirement: 浏览器 session SHALL 持续采集最近诊断记录
系统 SHALL 为每个 thread-scoped browser session 持续采集最近的 console 消息、页面错误和网络请求摘要。诊断记录 SHALL 随浏览器 session 生命周期存在，并在 session 关闭后释放；当记录量超过实现的缓冲上限时，系统 SHALL 保留最近记录，而 SHALL NOT 因为诊断量过大而导致工具调用失败。

#### Scenario: 诊断记录跨多次动作后仍可查询
- **WHEN** 当前线程的 browser session 在一次或多次 `navigate`、`click`、`type` 动作期间产生了 console、page error 或 request 事件
- **THEN** 后续调用对应诊断工具时可以查询到当前 session 最近保留的诊断记录
- **THEN** 这些记录会在 `browser__close` 或 toolset 卸载后随 session 一起释放

### Requirement: 系统 SHALL 通过 `browser__console` 返回结构化 console 记录
`browser__console` SHALL 作为只读查询工具返回当前 browser session 最近的 console 记录，并支持可选的 `limit` 参数收敛输出规模。每条 console 记录 SHALL 至少包含 `timestamp`、`level`、`text` 和 `page_url`；如果事件源能够提供源码位置信息，系统 SHALL 一并返回可选 `location` 字段。返回结果 SHALL 以“最新记录优先”顺序排列。

#### Scenario: 查询最近 console 记录
- **WHEN** 当前 browser session 已经采集到多条 console 记录，调用方执行 `browser__console` 并传入 `limit=5`
- **THEN** 系统返回不超过 5 条最近的 console 记录
- **THEN** 每条记录都包含规范化后的级别、文本和页面 URL 字段

### Requirement: 系统 SHALL 通过 `browser__errors` 返回统一错误视图
`browser__errors` SHALL 返回当前 browser session 最近的错误记录，至少覆盖页面运行时错误和请求失败两类事实。每条错误记录 SHALL 至少包含 `timestamp`、`kind`、`message` 和定位上下文字段；其中页面错误记录 SHALL 包含 `page_url`，请求失败记录 SHALL 至少包含 `request_url`，并在可用时附带失败原因。

#### Scenario: 查询最近页面错误与请求失败
- **WHEN** 当前 browser session 内出现页面脚本异常或网络请求失败
- **THEN** 调用 `browser__errors` 可以看到对应的结构化错误记录
- **THEN** 调用方可以根据 `kind` 区分这是页面错误还是请求失败

### Requirement: 系统 SHALL 通过 `browser__requests` 返回最近请求摘要
`browser__requests` SHALL 返回当前 browser session 最近的网络请求摘要，并支持可选 `limit` 参数；系统 SHALL 至少支持一个失败态过滤参数，使调用方可以只查看失败请求。每条请求记录 SHALL 至少包含 `timestamp`、`method`、`url`、`resource_type` 和 `result`；如果响应状态码已知，系统 SHALL 返回 `status` 字段。

#### Scenario: 只查询失败请求
- **WHEN** 当前 browser session 同时产生了成功请求与失败请求，调用方执行 `browser__requests` 并启用失败态过滤
- **THEN** 系统只返回失败请求的摘要记录
- **THEN** 每条记录都包含请求方法、URL、资源类型和结果状态

### Requirement: 保留 browser artifacts 时系统 SHALL 同步写出诊断文件
当 browser session 配置为保留 artifacts 时，系统 SHALL 把诊断记录同步写入当前 session 目录中的结构化文件，至少包括 `console.jsonl`、`errors.jsonl` 和 `requests.jsonl`。这些文件中的单条记录格式 SHALL 与对应诊断工具返回的规范化字段兼容，以便 helper、smoke 和人工排障复用。

#### Scenario: helper 保留 session 目录时可以查看诊断文件
- **WHEN** 开发者通过保留 artifacts 的 helper 或 smoke 入口运行 browser session，并且该 session 期间产生了诊断记录
- **THEN** session 目录中会生成 `console.jsonl`、`errors.jsonl` 和 `requests.jsonl`
- **THEN** 开发者在 session 关闭后仍然可以从这些文件中读取本次会话的诊断记录
