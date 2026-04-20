## 1. Queue 基础设施

- [x] 1.1 新增 PostgreSQL queue 配置、schema 初始化和独立 queue 模块，落地 `queue_message` 与 `queue_worker` 两张事实表
- [x] 1.2 实现 queue 专用 raw SQL 事务引擎，以及 `add / claim / complete / lease acquire / heartbeat / release / reap` 操作
- [x] 1.3 增加 worker/message 过期清理与关键调试日志，明确至少一次交付恢复路径

## 2. Router 与 Worker 改造

- [x] 2.1 保持 command 前置绕过 queue，把普通消息改成“先解析 locator，若未加载则准备 thread，再 `queue.add(topic=thread_key, payload=locator+message)`”
- [x] 2.2 删除 router 侧 `pending_threads/queued_messages` 内存排队机制，改为 `ensure_worker(thread_key)` 触发 domain worker
- [x] 2.3 直接改造 `AgentWorker`，移除 request `mpsc` 消费模型，改成按 domain 懒创建、空闲退出的 worker task

## 3. 验证与回归

- [x] 3.1 新增 queue 层测试，覆盖原子 add/claim/complete、同 domain 单 worker lease、过期恢复与清理日志
- [x] 3.2 新增 router/worker 集成测试，覆盖 command 绕过 queue、已加载 thread 直接入队、未加载 thread 先准备再入队、同 thread 顺序处理与 worker 空闲退出
- [x] 3.3 回归现有 thread sqlite 持久化链路，确认 queue 改造不改变 thread 正式事实的持久化边界
