## ADDED Requirements

### Requirement: 系统 SHALL 提供统一的 `ThreadContext` 作为线程运行时宿主
系统 SHALL 为每个内部线程提供统一的 `ThreadContext` 对象，并将其作为线程级运行时状态的主入口。`ThreadContext` SHALL 至少持有线程定位信息、`ThreadConversation` 和 `ThreadState`，供 Session、Command、AgentWorker 和 AgentLoop 共享同一条线程事实来源。

#### Scenario: 已解析线程被装配为统一上下文
- **WHEN** Router 或 Session 层为某个传入消息解析出目标内部线程
- **THEN** 系统会构造该线程对应的 `ThreadContext`
- **THEN** 后续线程级操作会围绕这一个 `ThreadContext` 继续执行

### Requirement: 系统 SHALL 使用 `thread_key` 与稳定的 internal thread id 定义线程身份
系统 SHALL 使用 `IncomingMessage.external_thread_id` 表示上游聊天平台提供的外部线程标识。系统 SHALL 以 `thread_key = user:channel:external_thread_id` 作为线程规范化键，并由该 key 稳定派生唯一的 internal thread id。系统 SHALL NOT 再引入独立的 conversation id。

#### Scenario: 同一外部线程始终映射到同一个内部线程
- **WHEN** 同一 `user + channel + external_thread_id` 下重复到来多条消息
- **THEN** 系统会产出相同的 `thread_key`
- **THEN** 系统会为这些消息解析出同一个 internal thread id
- **THEN** `ThreadConversation` 只保存历史与审计信息，不再额外拥有独立 conversation id

### Requirement: `ThreadContext` SHALL 统一管理线程级 feature、工具和审批状态
系统 SHALL 将线程级 feature 开关、工具加载状态、可见性决策输入以及审批权限状态统一放在 `ThreadContext` 的 `ThreadState` 中管理。系统 SHALL NOT 再以独立的全局 override 容器作为这些线程状态的唯一事实来源。

#### Scenario: 同一线程状态由同一宿主维护
- **WHEN** 某个线程开启 `auto_compact` 并加载一个可选 toolset
- **THEN** 这些线程状态会记录在该线程的 `ThreadContext`
- **THEN** 后续 AgentLoop、Command 和工具调用都读取同一份线程状态

### Requirement: Agent loop SHALL 以 `ThreadContext` 作为线程输入
系统 SHALL 让 AgentLoop 在执行线程级 ReAct 循环时直接接收 `ThreadContext`，并通过它完成历史读取、工具可见性计算、工具调用分发、feature 状态读取和事件记录。

#### Scenario: 同一轮循环通过 ThreadContext 更新线程态
- **WHEN** 模型在某一轮 ReAct 循环中加载 toolset 或触发线程级 feature 变化
- **THEN** 下一次生成前的线程工具可见性和 feature 判断会基于同一个 `ThreadContext` 重新计算
- **THEN** AgentLoop 不需要再额外查找分散的线程状态容器

### Requirement: 所有 Command SHALL 通过 `ThreadContext` 读写目标线程状态
系统 SHALL 将所有命令视为线程级命令。系统 SHALL 在执行命令前先解析目标线程，再通过该线程的 `ThreadContext` 读取或修改线程状态。

#### Scenario: 线程命令只修改当前线程上下文
- **WHEN** 用户在某个线程上执行 `/auto-compact on`
- **THEN** 系统会修改该线程 `ThreadContext` 中的 compact 相关状态
- **THEN** 其他线程的 `ThreadContext` 不会受到影响

### Requirement: 系统 SHALL 为旧线程运行时 API 提供 deprecated 兼容层
系统 SHALL 在迁移期内保留现有 thread-scoped runtime API 的兼容入口，并使用 Rust `#[deprecated]` 标记这些旧 API。兼容入口 SHALL 转发到 `ThreadContext` 新路径，直到调用点完成迁移。

#### Scenario: 旧入口在兼容期内仍然可用
- **WHEN** 现有调用点仍然使用旧的 thread-scoped runtime API
- **THEN** 系统行为仍然保持可用并转发到 `ThreadContext` 对应实现
- **THEN** 调用方可以明确看到该入口已经进入 deprecated 迁移期
