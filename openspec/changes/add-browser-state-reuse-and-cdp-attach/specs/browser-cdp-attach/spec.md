## ADDED Requirements

### Requirement: `browser__open` SHALL 支持显式的 `mode=attach`
当当前线程加载 `browser` toolset 后，系统 SHALL 允许调用方通过 `browser__open(mode=attach, cdp_endpoint=...)` 把当前线程的 browser session 连接到一个已经存在且已开启 remote debugging 的 Chromium/Chrome 实例。该入口 SHALL 要求调用方显式提供明确的 CDP endpoint，而 SHALL NOT 自动扫描、本地猜测或隐式附着到用户默认浏览器实例。

#### Scenario: 调用方通过 open 参数建立 attach 会话
- **WHEN** 当前线程执行 `browser__open`
- **AND** 调用方提供 `mode=attach` 与一个可访问的 `cdp_endpoint`
- **THEN** 系统会为该线程建立 attach 模式的 browser session
- **THEN** 后续 `browser__navigate`、`browser__snapshot`、`browser__click_ref`、`browser__type_ref` 与 `browser__screenshot` 都作用在该 attach 会话上

### Requirement: attach 失败时系统 SHALL 显式报错且不回退到 launch 模式
当调用方通过 `browser__open(mode=attach, cdp_endpoint=...)` 提供的 CDP endpoint 不可访问、协议不合法或目标浏览器不支持 attach 时，系统 SHALL 返回显式错误，并 SHALL NOT 静默回退为“自行拉起一个新的本地 Chromium 实例”。错误结果 SHALL 能让调用方区分这是 endpoint 配置错误还是连接失败。

#### Scenario: endpoint 不可用时 attach 直接失败
- **WHEN** 当前线程执行 `browser__open(mode=attach, cdp_endpoint=...)`
- **AND** 提供的 endpoint 无法连接
- **THEN** 系统返回 attach 失败的显式错误
- **THEN** 当前线程不会因为这次失败而偷偷启动新的本地浏览器会话

### Requirement: attach 模式与 launch 模式 SHALL 在同一线程 session 内互斥
对于同一个 thread-scoped browser session，系统 SHALL 保证 attach 模式与 launch 模式互斥。若当前线程已经持有活动 browser session，再次执行 `browser__open` 时，系统 SHALL 先显式关闭并替换当前会话，再建立新的 launch 或 attach 会话，而 SHALL NOT 让同一线程同时绑定多个浏览器来源。

#### Scenario: 再次 open 时会替换已有 session 来源
- **WHEN** 当前线程已经持有一个活动的 launch 会话或 attach 会话
- **AND** 调用方再次执行 `browser__open`，并要求切换到另一种来源或另一个 endpoint
- **THEN** 系统会先关闭并替换当前会话
- **THEN** 同一线程始终只持有一个活动 browser session

### Requirement: `browser__close` 在 attach 模式下 SHALL 只断开当前会话
当当前线程的 browser session 来源于 `browser__open(mode=attach, ...)` 时，`browser__close` SHALL 只关闭当前线程持有的 page/context/client 连接与 sidecar 会话，而 SHALL NOT 关闭或杀掉外部已有的 Chromium/Chrome 进程。关闭完成后，该线程后续若要继续使用 attach 浏览器，必须重新显式执行 `browser__open(mode=attach, ...)`。

#### Scenario: 关闭 attach 会话不会杀掉外部浏览器
- **WHEN** 当前线程已经通过 `browser__open(mode=attach, ...)` attach 到一个外部 Chromium 实例，并执行 `browser__close`
- **THEN** 当前线程持有的 attach 会话会被释放
- **THEN** 被 attach 的外部 Chromium 进程仍然保持运行

### Requirement: attach open 结果 SHALL 返回当前接管的页面上下文
通过 `browser__open(mode=attach, ...)` attach 成功后，系统 SHALL 返回当前线程已经接管的页面上下文摘要，至少包括当前页面 URL 与标题；如果目标浏览器当前没有可复用页面，系统 SHALL 为该 attach 会话建立一个可操作页面，并把对应上下文返回给调用方，以便后续动作重新建立观察基线。

#### Scenario: attach 成功后调用方可以直接建立新的观察基线
- **WHEN** 调用方成功执行 `browser__open(mode=attach, ...)`
- **THEN** 返回结果中包含当前接管页面的 URL 与标题，或者新建页面的对应信息
- **THEN** 调用方可以立刻继续执行 `browser__snapshot` 来建立新的页面观察基线
