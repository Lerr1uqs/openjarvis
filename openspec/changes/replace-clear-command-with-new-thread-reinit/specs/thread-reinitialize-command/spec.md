## ADDED Requirements

### Requirement: 系统 SHALL 提供 `/new` 线程重初始化命令
系统 SHALL 提供线程级命令 `/new`，用于把当前 thread 的会话历史与线程级运行态重置为“一个新的当前线程”，并在命令执行完成前立即重新初始化该 thread。系统 SHALL NOT 再提供 `/clear` 作为把 thread 清成空状态的命令入口。

#### Scenario: `/new` 对当前线程执行立即重初始化
- **WHEN** 当前线程已经存在稳定 `System` 前缀和若干后续历史消息，且用户执行 `/new`
- **THEN** 命令返回成功
- **THEN** 当前线程的普通历史消息与线程级运行态会被清空
- **THEN** 当前线程会在命令返回前重新写入稳定初始化前缀，而不是停留在空 thread 状态

#### Scenario: `/clear` 不再可用
- **WHEN** 用户执行 `/clear`
- **THEN** 系统将其视为未注册命令
- **THEN** 返回内容为 unknown command，而不是继续清空当前线程

### Requirement: `/new` SHALL 保留当前 thread 的 agent truth 与 child identity
`/new` SHALL 基于当前 thread 已拥有的 `ThreadAgentKind` 重新初始化，而不是把线程降级成默认 main 空线程。若当前 thread 是 child thread，系统 SHALL 保留其 `child_thread` identity，并继续把它视为原来的 child thread profile。

#### Scenario: Main thread 执行 `/new` 后仍然是 Main
- **WHEN** 一个 `Main` thread 执行 `/new`
- **THEN** 重初始化后的 thread 仍然使用 `Main` 的 system prompt、feature truth 和 tool 边界
- **THEN** 系统不会把该线程改造成其他 `ThreadAgentKind`

#### Scenario: Browser child thread 执行 `/new` 后仍然保留 child identity
- **WHEN** 一个 `Browser` child thread 执行 `/new`
- **THEN** 重初始化后的 thread 仍然是 `Browser` kind
- **THEN** 原有 `child_thread` identity 继续保留
- **THEN** 线程不会退化成没有 child identity 的默认 main thread

### Requirement: `/new` SHALL 通过命令路径完成而不触发 agent dispatch
`/new` SHALL 和其他线程命令一样通过 command 路径直接执行，并在目标 thread 空闲时完成。系统 SHALL NOT 因为执行 `/new` 额外发起一次 agent dispatch。

#### Scenario: `/new` 命令不会进入 agent worker
- **WHEN** 用户在空闲线程上执行 `/new`
- **THEN** router 会直接返回命令结果
- **THEN** 当前线程完成重初始化
- **THEN** 不会向 agent worker 派发新的用户请求
