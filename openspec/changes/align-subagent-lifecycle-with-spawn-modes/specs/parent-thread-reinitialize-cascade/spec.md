## ADDED Requirements

### Requirement: parent `/new` SHALL 级联 reset/reinit 当前 parent 名下全部 `persist` child thread
系统 SHALL 在 parent thread 执行 `/new` 时，同时枚举并重置该 parent 名下全部 `persist` child thread。每个目标 child thread SHALL 复用既有 reset + initialize 逻辑回到“仅保留稳定初始化前缀”的状态，而 SHALL NOT 继续保留旧会话历史。

#### Scenario: parent `/new` 同时重置自己的 `persist` child thread
- **WHEN** 当前 parent thread 已经挂载一个或多个 `persist` child thread
- **AND** 用户在该 parent thread 上执行 `/new`
- **THEN** 系统会先后对这些 `persist` child thread 执行 reset + reinitialize
- **THEN** 这些 child thread 的普通历史会被清空
- **THEN** 这些 child thread 的稳定初始化前缀会被重新建立

### Requirement: parent `/new` SHALL NOT 级联到 `yolo` child thread
系统 SHALL 将 `/new` 的级联范围限制在 `persist` child thread。`yolo` child thread 不属于长期可复用会话对象，因此 SHALL NOT 被 parent `/new` 视作必须一起重初始化的目标。

#### Scenario: parent `/new` 不会把 `yolo` child thread 纳入级联范围
- **WHEN** 当前 parent thread 名下存在 `yolo` child thread 的历史残留或临时记录
- **AND** 用户在该 parent thread 上执行 `/new`
- **THEN** 系统不会把这些 `yolo` child thread 当作长期 child session 一起 reinitialize
- **THEN** `/new` 的级联语义仍只覆盖 `persist` child thread

### Requirement: child thread 自己执行 `/new` SHALL 只重置自己
系统 SHALL 保持 child thread 的自重初始化边界。若当前执行 `/new` 的线程本身就是 child thread，系统 SHALL 只重置该 child thread 自己，并保留它的 child identity，而 SHALL NOT 继续向下枚举其他 thread。

#### Scenario: browser child thread 执行 `/new` 只重置自己
- **WHEN** 一个 `browser` child thread 自己执行 `/new`
- **THEN** 系统会保留它的 `child_thread_identity`
- **THEN** 系统只重置并重新初始化该 `browser` child thread 自己
- **THEN** 系统不会把它当作 parent 再去级联其他 child thread
