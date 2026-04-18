## ADDED Requirements

### Requirement: `spawn_subagent` SHALL 承担 subagent 的首轮启动与首个任务执行
系统 SHALL 要求调用方在 `spawn_subagent` 中同时提供 `subagent_key`、`spawn_mode` 与首个 `content`。工具 SHALL 负责创建或复用目标 child thread，并将该 `content` 作为子线程的一条 incoming user message 立即执行，再把聚合后的结果直接返回给主线程。

#### Scenario: `persist` 模式通过 `spawn_subagent` 完成首轮创建与执行
- **WHEN** 主线程第一次调用 `spawn_subagent(browser, persist, "执行首个任务")`
- **THEN** 系统会创建或初始化当前 parent 名下的 `browser` child thread
- **THEN** 系统会立刻把 `"执行首个任务"` 送入该 child thread 执行
- **THEN** 工具调用直接返回该 child thread 的聚合结果

#### Scenario: 已存在的 `persist` child thread 被 `spawn_subagent` 复用
- **WHEN** 当前 parent 名下已经存在一个 `persist` 模式的 `browser` child thread
- **AND** 主线程再次调用 `spawn_subagent(browser, persist, "继续首轮风格的任务")`
- **THEN** 系统会复用该既有 child thread
- **THEN** 工具调用仍会直接返回本次执行结果，而不是只返回“已准备好”

### Requirement: `send_subagent` SHALL 只允许对已存在的 `persist` child thread 做后续交互
系统 SHALL 将 `send_subagent` 约束为 `persist` child thread 的后续交互入口。`send_subagent` SHALL NOT 在 child thread 缺失时隐式创建新 child thread，也 SHALL NOT 用于 `yolo` 模式的首轮执行。

#### Scenario: `send_subagent` 对已存在 `persist` child thread 发送后续消息
- **WHEN** 当前 parent 名下已经存在一个可用的 `persist` 模式 `browser` child thread
- **AND** 主线程调用 `send_subagent(browser, "继续处理后续任务")`
- **THEN** 系统会把该消息送入既有 `browser` child thread
- **THEN** 工具调用返回本次后续交互的聚合结果

#### Scenario: `send_subagent` 不会替代 `spawn_subagent` 做首次创建
- **WHEN** 当前 parent 名下不存在目标 `browser` child thread
- **AND** 主线程直接调用 `send_subagent(browser, "第一次任务")`
- **THEN** 系统不会隐式创建新的 child thread
- **THEN** 工具调用会明确返回“需要先通过 `spawn_subagent` 启动 persist child thread”之类的失败信息

#### Scenario: `send_subagent` 不接受 `yolo` 后续交互
- **WHEN** 调用方尝试把 `yolo` child thread 当作 `send_subagent` 的目标继续交互
- **THEN** 系统会拒绝该请求
- **THEN** 系统不会把 `yolo` child thread 暴露成可持续复用的后续会话

### Requirement: `close_subagent` SHALL 只关闭 `persist` child thread
系统 SHALL 将 `close_subagent` 约束为 `persist` child thread 的高层结束动作。调用后，系统 SHALL 清空该 child thread 的运行历史，但保留其稳定 child identity，供后续再次 `spawn_subagent(..., persist, ...)` 重新初始化使用。

#### Scenario: `close_subagent` 关闭一个既有 `persist` child thread
- **WHEN** 当前 parent 名下存在一个 `persist` 模式的 `browser` child thread
- **AND** 主线程调用 `close_subagent(browser)`
- **THEN** 系统会清空该 child thread 的历史与运行态
- **THEN** 系统仍然保留该 child thread 的稳定 identity
- **THEN** 后续需要再次使用时，调用方必须重新执行 `spawn_subagent(..., persist, ...)`

### Requirement: `yolo` 模式 SHALL 通过一次 `spawn_subagent` 完成单次执行并结束生命周期
系统 SHALL 将 `yolo` 模式定义为单次执行生命周期。主线程在 `spawn_subagent(..., yolo, content)` 成功创建或复用 child thread 后，系统 SHALL 立刻执行该 `content`，并在本次调用完成后对该 child thread 执行 best-effort 回收。`yolo` 模式 SHALL NOT 依赖 `send_subagent` 或 `close_subagent` 才能结束生命周期。

#### Scenario: `yolo` 模式一次 `spawn_subagent` 即完成执行和结束
- **WHEN** 主线程调用 `spawn_subagent(browser, yolo, "只执行一次的任务")`
- **THEN** 系统会创建或复用一次性 child thread 并立刻执行任务
- **THEN** 工具调用直接返回结果
- **THEN** 本次调用结束后系统会 best-effort 回收该 child thread

### Requirement: `list_subagent` SHALL 只反映当前 parent 名下仍有持久状态的 child thread 视图
系统 SHALL 允许主线程通过 `list_subagent` 查看当前 parent 名下 child thread 的持久状态视图。对于 `persist` child thread，结果 SHALL 反映其是否当前可用；对于已被回收的 `yolo` child thread，结果 SHALL NOT 继续把它视为可后续交互对象。

#### Scenario: `list_subagent` 能看到关闭后的 `persist` child identity
- **WHEN** 一个 `persist` 模式 child thread 已被 `close_subagent`
- **THEN** `list_subagent` 仍会返回该 child thread 的稳定 identity
- **THEN** 返回结果中的 `available` 会明确标记为不可直接继续发送

#### Scenario: 已回收的 `yolo` child thread 不再作为可交互实体出现
- **WHEN** 一个 `yolo` child thread 已在 `spawn_subagent` 后完成回收
- **THEN** `list_subagent` 不会要求调用方再通过 `send_subagent` 或 `close_subagent` 管理它
- **THEN** 系统不会把它视为可持续复用的 child session
