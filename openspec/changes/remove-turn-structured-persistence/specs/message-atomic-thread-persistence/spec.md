## ADDED Requirements

### Requirement: `Thread.push_message(...)` SHALL 成为正式消息的唯一原子持久化边界
系统 MUST 让 `Thread.push_message(...)` 成为正式消息写入的唯一正式边界。某条消息在 `push_message(...)` 成功返回前，系统 SHALL 已经完成该消息对应线程快照的持久化；系统 SHALL NOT 再要求额外的 `Session`/`Router`/`Worker` 收尾提交来让这条消息变成正式历史。

#### Scenario: push_message 成功返回即表示消息已落盘
- **WHEN** AgentLoop 或命令路径调用 `Thread.push_message(...)` 并收到成功结果
- **THEN** 该消息已经成为线程正式消息序列的一部分
- **THEN** 系统不需要再调用任何 turn/finalized snapshot 提交接口来补做持久化

### Requirement: 线程初始化消息 SHALL 作为普通正式消息在创建时原子落盘
系统 MUST 将 feature/system 生成的线程初始化消息视为普通正式消息，并在新线程创建时通过 thread-owned 原子持久化入口写入。系统 SHALL NOT 依赖单独的“初始化完成时间戳”或后续请求中的补初始化流程来让这些消息变成正式历史。

#### Scenario: 新线程返回前已持久化初始化消息
- **WHEN** `SessionManager` 首次解析并派生某个线程且系统生成初始化消息
- **THEN** 这些消息会在 thread handle 对外可用前写入正式消息序列并完成持久化
- **THEN** 后续请求看到的是已经初始化完成的线程
- **THEN** 系统不需要 `request_context_initialized_at` 一类额外持久化字段来标记初始化完成

### Requirement: 与新 schema 冲突的旧数据库 SHALL 可以被直接替换
系统 MUST 允许在 thread-first schema 落地时直接删除与之冲突的旧数据库并重建新库。系统 SHALL NOT 要求保留旧 turn/session 持久化模型的兼容读取、兼容写回或自动迁移，作为本次重构的前置条件。

#### Scenario: 旧数据库不兼容时直接清库重建
- **WHEN** 旧数据库中的 turn/session schema 与新的 thread-first schema 冲突
- **THEN** 实现可以直接删除旧数据库文件并按新 schema 重建
- **THEN** 主链路实现不需要承担旧数据自动迁移兼容

### Requirement: thread 级正式状态变更 SHALL 与消息共用同一个原子持久化模型
系统 MUST 让 compact 写回、toolset load/unload、tool audit 追加、feature override 更新等正式 thread state 变更，与 `push_message(...)` 共用同一个 thread-owned 原子持久化模型。任何正式状态变更成功返回前，系统 SHALL 已经完成对应 snapshot 的持久化。

#### Scenario: toolset 变更成功返回后已完成持久化
- **WHEN** 当前线程成功加载或卸载一个 toolset
- **THEN** 该变更已经被写入线程正式状态并完成持久化
- **THEN** 后续请求或进程重启后的恢复会看到一致的 toolset 状态

### Requirement: `SessionManager` SHALL 只负责线程解析与 handle 管理
系统 MUST 将 `SessionManager` 收敛为线程身份解析、live handle registry 与 cache miss 恢复边界。`SessionManager` SHALL NOT 再承担 `commit_finalized_turn(...)`、finalized snapshot 提交、平台入口 dedup 或其他 turn-based 提交职责。

#### Scenario: SessionManager 不再负责最终 turn 提交
- **WHEN** 某个线程已经通过 `Thread.push_message(...)` 或其他 thread-owned mutator 写入正式状态
- **THEN** `SessionManager` 不需要也不会再被调用来执行 turn/finalized snapshot 提交
- **THEN** 线程正式状态的持久化 owner 只有目标 `Thread`

### Requirement: 持久化线程快照 SHALL 只包含 flat message 序列与 thread state
系统 MUST 以 thread 为核心聚合持久化正式状态。持久化快照 SHALL 只包含线程身份、稳定 request context、正式消息序列、thread state、revision 与必要时间戳；系统 SHALL NOT 以 `ConversationTurn`、`ThreadCurrentTurn`、`ThreadFinalizedTurn` 或任何 turn 结构持久化线程正式状态。

#### Scenario: 重启恢复后不会看到 turn 结构
- **WHEN** 系统从持久化层恢复一个线程
- **THEN** 恢复结果只包含正式消息序列、稳定 request context 和 thread state
- **THEN** 恢复结果中不会出现可被继续消费的 turn 结构或 turn 快照

### Requirement: thread store SHALL 使用 compare-and-swap revision 保护原子写入
系统 MUST 为每个线程快照维护 revision，并在写入时执行 compare-and-swap。若某个旧快照尝试覆盖更新版本，store SHALL 显式拒绝该写入，而 SHALL NOT 静默 merge 或覆盖。

#### Scenario: 旧版本线程快照写入被拒绝
- **WHEN** 两个 live handle 先后基于不同 revision 改写同一个线程
- **THEN** 后到达的旧 revision 写入会收到显式冲突错误
- **THEN** 已成功写入的新版本线程状态不会被旧版本覆盖

### Requirement: persistent Session 聚合 SHALL NOT 成为 thread 恢复前提
系统 MUST 允许线程持久化层直接基于稳定 thread identity 读写线程，而 SHALL NOT 要求先恢复一个持久化 `Session` 聚合才能恢复线程。`SessionKey` 可以继续作为运行时解析辅助，但 SHALL NOT 成为必须持久化的数据聚合。

#### Scenario: 线程恢复不依赖 session 元数据行
- **WHEN** 系统根据 `channel + user_id + external_thread_id` 解析出目标线程
- **THEN** store 可以直接加载该线程的正式快照
- **THEN** 恢复流程不需要额外依赖独立的 session 持久化记录

### Requirement: Session 与 ThreadStore SHALL NOT 持有平台入口 dedup 状态
系统 MUST 让平台入口 dedup 与 `Session`、`Thread`、`ThreadStore` 解耦。`Session`、thread 持久化快照和 store schema SHALL NOT 保存 `Feishu` 或其他 channel 的入口 dedup 状态。

#### Scenario: 线程快照中不出现入口 dedup 字段
- **WHEN** 系统持久化或恢复某个线程
- **THEN** 线程快照中不会出现 channel 入口 dedup 的状态字段
- **THEN** `SessionManager` 与 `ThreadStore` 不需要为入口 dedup 维护任何读写接口
