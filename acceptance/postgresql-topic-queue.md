# PostgreSQL Topic Queue 验收标准

本文档定义 PostgreSQL Topic Queue 改造的验收标准。

这份验收只关注当前变更的外部可观测行为和职责边界，不把实现细节、内部 trait 设计或 SQL 组织方式当作验收目标。

## 范围说明

本次验收只覆盖普通消息传递链路：

- 普通消息入队
- 按 `thread_key` 串行消费
- domain worker 生命周期
- worker 过期恢复
- queue 与 thread sqlite 的职责边界

本次验收不覆盖：

- thread sqlite 本身的数据模型重构
- LLM/tool 内部执行细节
- command 功能扩展
- outbound 分发链路重构

## 1. 入站与串行

### 1.1 普通消息必须进入 PostgreSQL queue

对于非 command 的普通消息，系统必须：

1. 先解析对应的 `ThreadLocator`
2. 若当前进程尚未加载该 thread，则先完成 thread 准备动作
3. 将 `locator + message` 写入 PostgreSQL queue
4. 触发对应 `thread_key` 的 domain worker

验收观察点：

- 普通消息不会再依赖 router 内存态队列作为事实来源
- queue 中可以观测到对应 message 记录
- queue payload 中包含 `ThreadLocator` 与消息内容，而不是 live thread handle

### 1.2 command 不得进入 queue

对于 `/xxx` 命令消息，系统必须继续走 command 前置处理路径。

验收观察点：

- command 可以正常返回结果
- command 不会写入 PostgreSQL queue
- command 不会触发普通消息 worker 消费链路

### 1.3 同一 `thread_key` 必须严格串行

同一个 `thread_key` 下的多条普通消息，必须按顺序逐条处理。

验收观察点：

- 前一条消息未完成前，后一条消息不能进入同一 thread 的并发执行
- 同一时间最多只有一个活跃 worker 持有该 `thread_key`
- 同一个 `thread_key` 的多条消息最终完成顺序与入队顺序一致

### 1.4 不同 `thread_key` 允许并发

不同 `thread_key` 的普通消息不应被无谓串行化。

验收观察点：

- 不同 thread 的消息可以由不同 worker 并发推进
- 不会因为同 user 或同 channel 被错误收敛成一条串行链

### 1.5 已加载 thread 不得重复 create

普通消息进入 router 后，如果对应 thread 当前进程已经加载，系统必须直接复用，而不是再次走 create 路径。

验收观察点：

- 已加载 thread 再次来消息时，不会重复初始化
- 未加载 thread 才允许先准备再入队
- loaded-path 与 cold-path 的行为可以通过测试和日志区分

## 2. Worker 生命周期

### 2.1 worker 必须按 domain 懒创建

worker 不应该是全局常驻扫描器，而应该由消息驱动按 `thread_key` 懒创建。

验收观察点：

- 某个 `thread_key` 首次出现待处理消息时，会启动对应 worker
- 没有消息时，不会为该 domain 常驻维持无意义 worker

### 2.2 同一 domain 同时最多一个活跃 worker

系统必须通过 worker lease 表表达 domain ownership。

验收观察点：

- 同一 `thread_key` 同时最多存在一个活跃 lease
- 已有活跃 lease 时，不能再次为该 domain 创建第二个 worker

### 2.3 worker 空闲后必须可退出

worker 在完成当前 domain 的待处理消息后，不应永久占用运行时资源。

验收观察点：

- 待处理消息清空后，worker 会在空闲超时后退出
- worker 退出时会释放对应 lease

## 3. 崩溃恢复

### 3.1 pending 消息必须具备重启后可见性

普通消息一旦成功入 queue，就不能再因为原来的内存队列消失而丢失。

验收观察点：

- 进程重启后，未处理的 `pending` 消息仍然存在
- 重启后仍可继续被新 worker 消费

### 3.2 过期 worker 必须被清理

如果 worker 在 claim 后异常退出，系统必须能通过 lease 过期机制识别并清理该 worker。

验收观察点：

- 过期 lease 会被回收
- 清理过程会留下明确日志
- 对应 domain 后续可以重新启动新 worker

### 3.3 stranded active message 必须恢复为可继续处理

若消息已进入 `active`，但所属 worker 已过期，这条消息不能永久卡死。

验收观察点：

- 过期清理后，该消息会恢复到可再次处理状态
- 后续新 worker 可以重新 claim 并完成该消息

### 3.4 系统显式接受至少一次交付

由于 queue 与 thread sqlite 不做跨库联合事务，本次验收接受至少一次交付，而不是精确一次。

验收观察点：

- 文档、日志或测试中明确体现该边界
- 崩溃恢复后允许消息再次处理
- 系统不会错误宣称自己已经达到精确一次

## 4. 职责边界

### 4.1 queue 只管传递状态

PG queue 只能管理：

- add
- claim
- complete
- worker lease / heartbeat / expire / reap

验收观察点：

- queue 表中不要求保存 thread sqlite snapshot
- queue 表中不要求保存 thread revision
- queue 表中不要求保存 LLM/tool 执行过程事实

### 4.2 thread 正式事实仍然只走现有 sqlite/session store

thread 的正式消息、system 前缀、state 和 revision 仍然必须由现有 thread/session 持久化链路负责。

验收观察点：

- queue 改造后，thread sqlite 落盘和恢复语义保持成立
- worker 处理过程中对 thread 的正式写入仍然走现有 thread-owned 持久化路径

### 4.3 command 语义不得被本次改造顺手改变

本次 PG queue 改造不能把 command 也改成 queue message，也不能改变 command 当前的职责边界。

验收观察点：

- command 仍由 router 前置执行
- command 的回复语义与 thread 修改语义保持现状

## 5. 可观测性

系统至少必须为这些关键动作提供调试日志：

- 普通消息入队
- worker lease 获取
- message claim
- message complete
- worker heartbeat
- worker release
- worker expire / reap
- stranded message 恢复

验收观察点：

- 可以从日志定位一条消息属于哪个 `thread_key`
- 可以从日志定位是谁持有该 worker lease
- 可以从日志追踪该消息经历了入队、claim、complete 或恢复

## 6. 一票否决项

出现以下任一情况，视为本次验收不通过：

1. command 消息进入 PostgreSQL queue
2. 同一 `thread_key` 出现并发执行
3. 已加载 thread 再来普通消息时仍重复走 create 路径
4. queue 开始持久化 thread sqlite 的正式事实
5. worker 过期后 active message 永久卡死
6. 系统对外宣称精确一次，但实际只实现了至少一次
