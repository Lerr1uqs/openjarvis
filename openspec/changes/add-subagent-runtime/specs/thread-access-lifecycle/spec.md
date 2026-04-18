## MODIFIED Requirements

### Requirement: SessionManager SHALL 暴露显式的线程访问生命周期入口
系统 SHALL 在 `SessionManager` 中继续暴露职责不重叠的线程访问入口，并 SHALL 把这套访问生命周期同时扩展到 child thread。对 child thread 来说，`create_thread` SHALL 负责准备可直接服务的子线程并显式声明 `ThreadAgentKind`，`load_thread` SHALL 负责纯读取已有 child thread 快照，`lock_thread` SHALL 负责获取已有 child thread 的可变句柄。系统 SHALL NOT 为 child thread 额外引入绕开 `create/load/lock` 的第二套线程访问入口。

#### Scenario: child thread 继续走显式 `create_thread`
- **WHEN** 系统在某个父线程下首次准备一个 `browser` child thread
- **THEN** 系统会通过显式 `create_thread(..., ThreadAgentKind::Browser)` 路径创建并初始化该 child thread
- **THEN** 初始化完成前，系统不会把该 child thread 视为可直接服务

#### Scenario: 已存在的 child thread 继续走 `load_thread` / `lock_thread`
- **WHEN** 某个父线程下的 `browser` child thread 已经存在于 cache 或持久化 store 中
- **THEN** 后续读取该 child thread 时继续通过 `load_thread` 恢复
- **THEN** 后续修改该 child thread 时继续通过 `lock_thread` 获取 live 句柄

### Requirement: `create_thread` SHALL 支持基于 child-thread identity 准备 subagent child thread
系统 SHALL 允许 `create_thread` 在既有主线程 identity 之外，也支持基于 child-thread identity 准备 subagent child thread。对于 child thread，系统 SHALL 基于父线程真相与 child-thread identity 解析目标线程；若目标 child thread 已存在，则 SHALL 复用；若不存在，则 SHALL 创建并初始化。

#### Scenario: child thread 首次 create 时创建并初始化
- **WHEN** 当前父线程下还不存在目标 `browser` child thread
- **AND** 调用方请求准备这个 `browser` child thread
- **THEN** 系统会解析出该 child thread 的稳定内部 identity
- **THEN** 系统会创建新的 child thread handle
- **THEN** 系统会在返回成功前完成该 child thread 的初始化

#### Scenario: child thread 重复 create 时复用既有线程
- **WHEN** 当前父线程下已经存在目标 `browser` child thread
- **AND** 调用方再次请求准备这个 `browser` child thread
- **THEN** 系统会复用或恢复这个既有 child thread
- **THEN** 系统不会因为重复 create 再创建第二个同 profile child thread

### Requirement: 系统 SHALL 提供底层 thread `remove` 能力以删除已落盘 child thread 记录
系统 SHALL 在 `Thread`、`SessionStore` 或同等底层线程访问层提供正式的 `remove` 能力，用于删除已经落盘的 child thread 记录。该能力 SHALL 属于底层线程生命周期管理，而 SHALL NOT 直接作为普通 agent 工具暴露。

#### Scenario: `yolo` child thread 成功返回后触发底层 `remove`
- **WHEN** 一个 `yolo` 模式的 child thread 已经完成本次请求并成功返回工具结果
- **THEN** 系统会调用底层 thread `remove` 能力删除该 child thread 的已落盘记录
- **THEN** 这次删除不需要通过额外的高层 agent 工具触发

#### Scenario: 底层 `remove` 删除一个已存在的 child thread
- **WHEN** 某个 child thread 当前存在于 session cache 或持久化 store 中
- **AND** 系统调用底层 thread `remove`
- **THEN** 系统会移除该 child thread 的已落盘记录
- **THEN** 后续 `load_thread` 或 `lock_thread` 不再返回这个已删除 child thread

### Requirement: 系统 SHALL 允许 child thread 的 `remove` 与高层 `ToolResult` commit 解耦
系统 SHALL 允许 `yolo` child thread 的底层 `remove` 在本次 subagent 工具结果成功返回后立即执行，而不必与父线程后续 `ToolResult` commit 做事务绑定。系统 SHALL 接受这种 best-effort cleanup 语义作为首版约束。

#### Scenario: `yolo` child thread 先 remove 再等待父线程后续提交
- **WHEN** `send_subagent` 已经成功拿到 `yolo` child thread 的工具结果
- **THEN** 系统可以先执行该 child thread 的底层 `remove`
- **THEN** 系统不要求等待父线程对应的 `ToolResult` 完成正式提交后才允许回收这个 child thread
