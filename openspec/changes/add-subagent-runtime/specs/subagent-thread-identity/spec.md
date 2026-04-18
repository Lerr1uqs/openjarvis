## ADDED Requirements

### Requirement: 系统 SHALL 使用独立 child-thread identity 表达 subagent 线程
系统 SHALL 为 subagent 线程保存独立的 child-thread identity。该 identity SHALL 至少记录 `parent_thread_id` 和 `subagent_key`，并 MAY 记录 `spawn_mode` 等子线程元数据。系统 SHALL NOT 通过拼接、改写或污染 channel 侧 `external_thread_id` 来表达 subagent 身份。

#### Scenario: subagent 身份不再写回 channel `external_thread_id`
- **WHEN** 系统在某个主线程下创建一个 `browser` subagent
- **THEN** 主线程与子线程对应的 channel `external_thread_id` 语义保持为外部会话标识本身
- **THEN** subagent 身份通过独立 child-thread identity 表达
- **THEN** 系统不会把 `browser`、`subagent` 或同类内部标记拼回 `external_thread_id`

### Requirement: 系统 SHALL 基于父线程真相与 child-thread identity 派生稳定 child thread id
系统 SHALL 基于父线程自己的稳定 thread truth 与 child-thread identity 派生 subagent child thread 的内部 `thread_id`。对同一个父线程、同一个 `subagent_key`，系统 SHALL 派生出稳定且可重复解析的同一个 child thread id；不同父线程之间，即使 `subagent_key` 相同，也 SHALL 派生出不同的 child thread id。

#### Scenario: 同一父线程重复解析同一个 subagent profile
- **WHEN** 调用方在同一个父线程下两次解析 `browser` subagent 的 child-thread identity
- **THEN** 系统两次都会得到同一个 child thread id
- **THEN** 后续 `create/load/lock` 都会指向同一个 child thread

#### Scenario: 不同父线程下的同名 subagent 互相隔离
- **WHEN** 父线程 A 和父线程 B 都创建 `browser` subagent
- **THEN** 系统会为它们派生不同的 child thread id
- **THEN** 两个父线程下的 `browser` subagent 状态互不影响

### Requirement: 同一父线程下同一种 subagent profile SHALL 只允许一个 child thread 实例
系统 SHALL 将“同一父线程下的同一种 subagent profile”收敛为唯一 child thread。该唯一性约束 SHALL 由 `parent_thread_id + subagent_key` 定义，而 SHALL NOT 因 `spawn_mode` 不同而派生出第二个同 profile child thread。

#### Scenario: 同一父线程下重复创建同 profile subagent
- **WHEN** 调用方在同一个父线程下重复创建 `browser` subagent
- **THEN** 系统会复用或返回该父线程下已经存在的 `browser` child thread
- **THEN** 系统不会因为重复创建再生成第二个 `browser` child thread

#### Scenario: `spawn_mode` 不会绕过同 profile 单实例约束
- **WHEN** 某个父线程下已经存在一个 `browser` child thread
- **AND** 调用方再次以不同的 `spawn_mode` 请求同一个 `browser` subagent
- **THEN** 系统仍然继续指向这个既有 `browser` child thread
- **THEN** 系统不会因为 `persist` / `yolo` 不同而并存两个同 profile child thread

### Requirement: 系统 SHALL 持久化 child-thread identity 并在恢复后保留父子关系
系统 SHALL 将 child-thread identity 持久化为 child thread 自己的正式线程真相之一。线程恢复后，系统 SHALL 仍然能够判断该线程属于哪个父线程、对应哪个 `subagent_key`，并继续按照相同 identity 解析到这个 child thread。

#### Scenario: 已落盘 child thread 恢复后仍保留父子关系
- **WHEN** 某个 `browser` subagent child thread 已经写入持久化 store
- **AND** 系统在后续流程中从 store 恢复该 child thread
- **THEN** 恢复后的线程仍然保留原始 `parent_thread_id`
- **THEN** 恢复后的线程仍然保留原始 `subagent_key`
- **THEN** 后续同一父线程再次解析 `browser` subagent 时会继续命中这个既有 child thread
