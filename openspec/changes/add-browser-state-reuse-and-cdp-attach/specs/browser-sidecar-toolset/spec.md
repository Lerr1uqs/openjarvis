## MODIFIED Requirements

### Requirement: 系统 SHALL 为 `browser` toolset 提供统一的 `browser__open` 会话入口
系统 SHALL 在当前线程加载 `browser` toolset 后，把 `browser__open` 作为 browser session 的统一显式打开入口，并继续暴露 `browser__navigate`、`browser__snapshot`、`browser__click_ref`、`browser__type_ref`、`browser__screenshot` 和 `browser__close`。`browser__open` SHALL 至少接受一个 `mode` 参数，用于在 `launch` 与 `attach` 两种会话来源之间做显式选择；当 `mode=launch` 时，系统 SHALL 继续为该会话创建独立临时 `user-data-dir`，而 SHALL NOT 复用系统默认 Chrome Profile。

#### Scenario: 加载 browser 后可以看到统一 open 入口
- **WHEN** 当前线程成功调用 `load_toolset` 加载 `browser`
- **THEN** 当前线程后续可见工具列表中包含 `browser__open`、`browser__navigate`、`browser__snapshot`、`browser__click_ref`、`browser__type_ref`、`browser__screenshot` 和 `browser__close`
- **THEN** 未加载 `browser` 的其他线程不会看到这些工具

### Requirement: browser session 初始化 SHALL 统一经过 open 语义
系统 SHALL 让所有需要 live browser session 的动作都复用同一套 open 语义。调用方显式执行 `browser__open` 时，系统 SHALL 按其参数建立或替换当前线程 session；当调用方未显式 open，而是直接执行需要 session 的 browser 动作时，系统 SHALL 通过与 `browser__open(mode=launch)` 等价的默认 launch 初始化路径创建当前线程 session，使 launch / attach 边界、cookies 自动注入和后续 close 语义保持一致。

#### Scenario: 未显式 open 时默认走等价 launch 初始化
- **WHEN** 当前线程尚未持有活动 browser session
- **AND** 调用方直接执行需要 live session 的 browser 动作
- **THEN** 系统会通过与 `browser__open(mode=launch)` 等价的初始化路径建立默认 launch 会话
- **THEN** 后续 `browser__close` 与 cookies 生命周期语义与显式 open 保持一致

### Requirement: `browser__open` 结果 SHALL 返回当前接管的页面上下文摘要
无论 `browser__open` 最终建立的是 launch 还是 attach 会话，系统 SHALL 返回当前线程已经接管的页面上下文摘要，至少包括当前页面 URL、标题和会话来源模式。若 open 后当前浏览器没有现成可操作页面，系统 SHALL 为该 session 建立一个可操作页面，并把对应上下文返回给调用方，以便后续动作重新建立观察基线。

#### Scenario: open 成功后调用方可以立刻继续观察或导航
- **WHEN** 调用方成功执行 `browser__open`
- **THEN** 返回结果中包含当前接管页面的 URL、标题和会话来源模式
- **THEN** 调用方可以立刻继续执行 `browser__snapshot` 或 `browser__navigate`
