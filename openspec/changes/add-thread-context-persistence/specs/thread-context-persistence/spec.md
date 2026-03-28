## ADDED Requirements

### Requirement: 系统 SHALL 通过统一 store 抽象持久化线程上下文
系统 SHALL 提供统一的线程持久化 store 抽象，并由 `SessionManager` 通过该抽象读写线程持久化数据。该抽象 SHALL 屏蔽具体数据库实现细节，使同一组线程持久化语义可以由 SQLite 首版实现，并在未来由 PostgreSQL 等其他后端实现。

#### Scenario: `SessionManager` 不依赖具体数据库类型
- **WHEN** 系统使用 SQLite backend 或兼容的其他 store backend
- **THEN** `SessionManager` 通过同一组 store 接口完成线程读取与保存
- **THEN** Router、Command 和 AgentLoop 不需要感知底层数据库类型

### Requirement: 系统 SHALL 将 `ThreadContext` 作为线程持久化聚合根
系统 SHALL 以 `ThreadContext` 作为线程持久化的聚合根，并持久化其 `locator`、`conversation` 和 `state`。持久化后的线程快照 SHALL 作为线程历史和线程级状态的重启恢复事实来源。

#### Scenario: 重启后恢复同一线程快照
- **WHEN** 某个线程已经持久化了 `ThreadContext`，且服务进程重启
- **THEN** 系统在再次命中该线程时可以恢复该线程最近一次保存的 `ThreadContext`
- **THEN** 该线程的 conversation 历史和 thread state 与重启前最后一次成功保存的状态保持一致

### Requirement: 系统 SHALL 以 `Vec<ConversationTurn>` 持久化线程会话历史
系统 SHALL 以 turn 结构持久化线程会话历史，并保留每个 `ConversationTurn` 的边界、标识和审计字段。系统 SHALL NOT 只持久化扁平 messages 后再以消息形态反推 turn。

#### Scenario: compact turn 的边界在重载后保持不变
- **WHEN** 某个线程历史中已经包含 compact 生成的 turn
- **THEN** 持久化记录会保留该 compact turn 的原始 turn 边界
- **THEN** 线程重载后不会把该 compact turn 错误拆解为普通扁平 messages 推断结果

### Requirement: 系统 SHALL 将运行时 attachment 排除在持久化快照之外
系统 SHALL 只持久化可声明式恢复的线程 conversation 与 thread state。`pending_tool_events`、Router 排队状态、兼容缓存和 live browser session 等纯运行时 attachment SHALL NOT 直接进入线程持久化快照。

#### Scenario: 运行时 attachment 不会被错误恢复为持久化状态
- **WHEN** 服务在某个线程存在 live runtime attachment 时重启
- **THEN** 系统只从持久化快照恢复 conversation 与 thread state
- **THEN** live runtime attachment 会在恢复后按运行时规则重新创建，而不是从旧进程对象直接恢复

### Requirement: 系统 SHALL 在 cache miss 时从持久化层恢复线程上下文
系统 SHALL 允许 `SessionManager` 在内存 cache miss 时按线程定位信息从持久化层恢复 `ThreadContext`，并在恢复后继续以该上下文服务后续命令与 agent turn。

#### Scenario: 首次命中已存在线程时执行懒加载恢复
- **WHEN** 某个线程已存在于持久化层，但当前进程内 cache 中不存在该线程
- **THEN** `SessionManager` 会在首次命中该线程时从持久化层加载该线程快照
- **THEN** 后续同一线程访问可以继续使用该进程内热缓存

### Requirement: 系统 SHALL 持久化线程级外部消息去重记录
系统 SHALL 持久化与线程相关的外部消息去重记录，使系统在重启后仍能识别某个 `external_message_id` 是否已经被处理过，并避免重复追加 turn 或重复回复。

#### Scenario: 重启后遇到上游重复投递仍保持幂等
- **WHEN** 某个 `external_message_id` 对应的消息在服务重启后被上游重复投递
- **THEN** 系统可以根据持久化去重记录识别该消息已处理
- **THEN** 系统不会再次为该消息重复创建 turn 或重复发送同一轮回复
