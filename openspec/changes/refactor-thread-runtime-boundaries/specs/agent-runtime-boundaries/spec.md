## MODIFIED Requirements

### Requirement: Agent worker SHALL 在 live loop 前初始化 `Thread`
系统 SHALL 在 `AgentWorker` 把请求交给 live agent loop 前完成线程初始化。初始化 SHALL 负责把稳定 system messages 写入 `Thread`；`AgentLoop` SHALL 不再拥有 bootstrap / init ownership。

#### Scenario: worker 在进入 loop 前初始化线程
- **WHEN** AgentWorker 收到一个线程请求
- **THEN** 它会先检查线程是否需要初始化
- **THEN** 如需初始化，会先把稳定 system messages 写入 `Thread`
- **THEN** 之后才把线程交给 `AgentLoop`

### Requirement: 初始化结果 SHALL 在进入 live loop 前同步给 session
如果线程初始化修改了 `Thread`，系统 SHALL 在继续 live loop 之前把更新后的线程快照同步给 session。`AgentLoop` 本身 SHALL NOT 直接依赖 `SessionManager`。

#### Scenario: 初始化后立即写回 session
- **WHEN** AgentWorker 初始化线程且线程快照发生变化
- **THEN** worker 会把更新后的 `Thread` 同步给 router/session
- **THEN** live loop 使用的是已经同步过的线程快照

### Requirement: AgentLoop SHALL 只消费 `Thread + current user input`
系统 SHALL 让 `AgentLoop` 的主执行边界只围绕 `Thread + current user input`。loop SHALL 只维护 request-time working set、调用 LLM、执行工具和触发 compact，而 SHALL NOT 再负责线程初始化。

#### Scenario: loop 不再 bootstrap 线程
- **WHEN** AgentLoop 开始处理某轮请求
- **THEN** 它拿到的是已经初始化好的 `Thread`
- **THEN** loop 内不会再出现 bootstrap / init thread 步骤

## ADDED Requirements

### Requirement: compact 主链路 SHALL 不再依赖 turn-based strategy
系统 SHALL 让 agent 主链路上的 compact 直接基于消息序列工作，而不是依赖 turn-based strategy / plan / source slice。

#### Scenario: loop 直接把消息序列交给 compact
- **WHEN** AgentLoop 判断需要执行 compact
- **THEN** 它直接把当前 working set 中的消息序列交给 compact
- **THEN** compact 不要求 loop 先构造 turn-based source plan
