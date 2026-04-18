## ADDED Requirements

### Requirement: SessionManager SHALL 暴露显式的线程访问生命周期入口
系统 SHALL 在 `SessionManager` 中暴露职责不重叠的线程访问入口：`create_thread` SHALL 负责准备可直接服务的线程并显式声明 `ThreadAgentKind`，`load_thread` SHALL 负责读取已有线程快照，`lock_thread` SHALL 负责获取已有线程的可变句柄。系统 SHALL NOT 再通过混合语义入口把创建、加载和初始化隐藏在同一次调用中。

#### Scenario: `create_thread` 首次准备一个全新线程
- **WHEN** 某条 incoming message 解析出的线程既不在 session cache 中，也不存在于持久化 store 中
- **THEN** 系统会解析稳定 thread identity 并创建新的 thread handle
- **THEN** 系统会在返回成功前以调用方提供的 `ThreadAgentKind` 完成线程初始化
- **THEN** 初始化后的线程会进入 cache，并可被后续 `load_thread` 或 `lock_thread` 访问

#### Scenario: `create_thread` 准备一个已存在线程
- **WHEN** 某条 incoming message 解析出的线程已经存在于 session cache 或持久化 store 中
- **THEN** 系统会复用或恢复该 thread handle
- **THEN** 系统会在返回成功前补齐该线程缺失的初始化
- **THEN** 调用方可以把该线程视为已经可直接对外服务

#### Scenario: `create_thread` 再次访问已初始化线程时不改写既有 agent 类型
- **WHEN** 某个已初始化线程已经持久化了自己的 `ThreadAgent`
- **AND** 调用方再次以不同的 `ThreadAgentKind` 调用 `create_thread`
- **THEN** 系统继续以该线程已持久化的 `ThreadAgent` 作为真相
- **THEN** 系统不会在重复 create 时改写已有稳定 `System` 前缀
- **THEN** 系统会通过日志暴露这次 agent 类型不一致

### Requirement: `load_thread` SHALL 保持纯读取语义
系统 SHALL 允许调用方读取一个已有线程，而不创建新的空线程，也不在读取过程中触发线程初始化、副作用持久化或稳定 `System` 前缀补写。

#### Scenario: `load_thread` 在 cache miss 时恢复已有线程
- **WHEN** `load_thread` 访问的目标线程当前不在 session cache 中，但存在于持久化 store 中
- **THEN** 系统会恢复并缓存该线程快照
- **THEN** 返回的线程状态与调用前持久化快照保持一致
- **THEN** 读取路径不会追加初始化消息或更新 initialized lifecycle state

#### Scenario: `load_thread` 访问不存在的线程
- **WHEN** `load_thread` 访问的目标线程既不在 session cache 中，也不存在于持久化 store 中
- **THEN** 系统会向调用方返回线程不存在
- **THEN** 系统不会创建新的空线程 handle
- **THEN** 系统不会触发线程初始化副作用

### Requirement: `lock_thread` SHALL 只锁定已有线程
系统 SHALL 只让 `lock_thread` 返回一个已经存在的线程句柄。`lock_thread` SHALL NOT 在 miss 时偷偷创建空线程，也 SHALL NOT 作为隐式 create-or-initialize 入口。

#### Scenario: `lock_thread` 锁定一个恢复出的已有线程
- **WHEN** `lock_thread` 访问的目标线程当前不在 session cache 中，但存在于持久化 store 中
- **THEN** 系统会先恢复并缓存该线程
- **THEN** 调用方会拿到该已有线程的可变句柄
- **THEN** 加锁路径不会触发线程初始化

#### Scenario: `lock_thread` 访问不存在的线程
- **WHEN** 调用方在未执行 `create_thread` 的前提下对一个不存在的线程调用 `lock_thread`
- **THEN** 系统会显式报告该线程不存在
- **THEN** 系统不会创建新的空线程 handle
- **THEN** 调用方若需要可直接服务的线程，必须切换到显式 create 路径
