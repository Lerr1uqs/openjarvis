## 1. Child Thread Identity 与底层生命周期

- [x] 1.1 为线程状态与 locator 增加 child-thread identity 所需字段，至少覆盖 `parent_thread_id`、`subagent_key` 和 `spawn_mode`
- [x] 1.2 实现基于“父线程真相 + child-thread identity”的稳定 child thread id 派生逻辑，并保证同一父线程下同 profile 单实例
- [x] 1.3 在 `Thread`、`SessionStore` 或同等底层线程生命周期层新增正式 `remove` 能力，用于删除已落盘的 child thread 记录

## 2. 线程初始化与访问接线

- [x] 2.1 扩展 `SessionManager` 的 `create_thread / load_thread / lock_thread`，使其支持 child thread 的 create/load/lock 路径
- [x] 2.2 扩展 `ThreadAgentKind` 与线程初始化链路，使 subagent profile 继续通过既有 prompt markdown 与默认工具绑定完成初始化
- [x] 2.3 补充 child thread 的恢复与重复 create 约束，确保已存在 child thread 不会因为重复 prepare 再生成第二个同 profile 实例

## 3. Subagent Runner 与内部事件模式

- [x] 3.1 新增独立的 `SubagentRunner` 与 subagent worker 池，避免主线程在主 worker 请求队列上同步等待自己
- [x] 3.2 复用现有 `AgentLoop` 执行 child thread，并新增 subagent 场景需要的兼容层 `IncomingMessage` 构造
- [x] 3.3 为 `AgentEventSender` 增加 `for_subagent_thread(...)`，并接入“只记录不发送”的 committed event 处理路径

## 4. Subagent 工具与生命周期语义

- [x] 4.1 新增 `spawn_subagent`、`send_subagent`、`close_subagent`、`list_subagent` 四个工具，并注册到主线程可见工具集合
- [x] 4.2 让 `send_subagent` 首版只支持同步阻塞语义，返回单次聚合 `ToolCallResult`，不支持后台异步任务或流式用户可见子结果
- [x] 4.3 实现 `persist` 与 `yolo` 生命周期：`persist` 保留 child thread，`yolo` 在成功返回后 best-effort 调用底层 `remove`

## 5. 测试与回归验证

- [x] 5.1 在对应 `tests/` 目录下补齐 child-thread identity 与单 profile 单实例测试，覆盖重复 create、不同父线程隔离和恢复后命中既有 child thread
- [x] 5.2 补齐 subagent runner / agent loop 集成测试，覆盖独立 worker 池执行、同步阻塞返回，以及 `for_subagent_thread` 不进入 Router/channel 发送面的行为
- [x] 5.3 补齐 `persist / yolo` 生命周期测试，覆盖 `list_subagent` 视图、`close_subagent` 行为，以及 `yolo` 成功返回后底层 `remove` 删除已落盘记录
