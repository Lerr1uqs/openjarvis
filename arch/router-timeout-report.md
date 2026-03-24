# Router 命令测试超时报告

审计时间：2026-03-24

## 现象

在实现 ` Command ` 前置拦截后，运行 ` cargo test --test router ... ` 时多次出现 120 秒超时。

后续连续重跑时，在 Windows 上又触发了 ` LNK1104 `，原因是上一次卡住的测试进程没有退出，仍然占用 ` target/debug/deps/router-*.exe `。

## Root Cause

这次超时实际有两层原因：

### 第一层：测试构造问题

超时最先暴露出来的直接原因不是命令逻辑本身，而是测试构造方式：

- 新增的 command-only 路由测试里手工创建了 ` AgentWorkerHandle `
- 其中 ` event_rx ` 被交给了 ` ChannelRouter `
- 但配对的 ` event_tx ` 仍然活着，没有被 drop
- 同时这些测试不会真的向 ` event_rx ` 发送任何 agent event

于是 ` ChannelRouter::run ` 会一直阻塞在：

```rust
agent_event_rx.recv().await
```

因为：

- channel 输入已经处理完
- agent 事件永远不会到来
- 但 ` event_tx ` 还活着，所以 ` recv() ` 也不会返回 ` None `

这就导致测试进程本身不退出，最终被外层 120 秒超时杀掉。

### 第二层：Router 关闭路径缺陷

排查过程中又发现一个独立但真实存在的运行时问题：

- ` Router ` 原先在 ` tokio::select! ` 里只匹配 ` Some(...) `
- 当 ` agent_event_rx.recv() ` 返回 ` None ` 后，分支虽然被执行了，但 loop 没有把这个 channel 标记为 closed
- 下一轮 ` select! ` 会继续立即命中这个已经关闭的分支，形成空转，导致 ` Router ` 不能自然退出

这意味着：

- sender 没 drop 时，` Router ` 会一直 pending
- sender 已经 drop 时，旧实现也可能因为关闭分支处理不完整而无法正常退出

这部分现在已经修复：` Router ` 会在检测到 ` agent_event_rx ` 关闭后，把对应分支禁用掉，再让 ` else ` 分支接管退出。

## 最小复现

见：

- ` tests/router_timeout_root_cause.rs `

其中当前保留的测试分别说明：

1. ` router_run_stays_pending_while_service_is_healthy `
   在健康服务语义下，` router.run() ` 会持续 pending
2. ` router_run_until_shutdown_exits_when_shutdown_signal_arrives `
   显式发出 shutdown 后，` router.run_until_shutdown(...) ` 会正常退出
3. ` router_returns_error_when_agent_event_channel_closes `
   下游 ` agent_event ` 通道断开时，` router.run() ` 会直接报错退出

这个复现已经足够小：

- 不依赖 Feishu
- 不依赖真实 AgentWorker
- 不依赖 command 逻辑
- 只保留 ` Router + mpsc sender 生命周期 ` 这一条因果链

## 为什么 Windows 上会出现 LNK1104

当超时的 ` cargo test ` 被外部杀掉时，卡住的测试子进程可能还没被完全清理。

Windows 的可执行文件锁比较严格，导致下一次链接同名测试二进制时，` link.exe ` 无法覆盖旧文件，于是出现：

```text
LINK : fatal error LNK1104: cannot open file '...router-xxxx.exe'
```

本质上这不是新的编译错误，而是“上一次 hang 的测试进程还占着 exe”。

## 解决方案讨论

### 方案 1：测试里显式 drop 不再需要的 sender

这是这次采用的一部分方案。

优点：

- 能消除测试自己制造出来的 pending
- 不污染生产逻辑
- 测试退出语义明确

适用场景：

- 手工拼 ` mpsc::channel ` 做 mock handle 的测试

### 方案 2：修正 Router 的关闭状态机

这部分后来又按服务语义收了一次口。

优点：

- 能让 ` Router ` 在 ` agent_event_rx ` 关闭后明确报错
- 修正的是生产逻辑，不只是测试技巧

缺点：

- 只能解决关闭路径问题，不能替代正确的 sender 生命周期管理
- 当前只留了报错退出的 slot，还没有实现自动重启 worker

### 方案 3：测试外层加 ` timeout `

这是调试阶段的临时兜底手段，不建议作为最终方案。

优点：

- 能快速止血，避免 CI/本地无限挂住

缺点：

- 会掩盖 sender 生命周期没有收干净的问题
- 会掩盖 ` Router ` 自己的退出条件缺陷

### 方案 4：给 Router 增加显式 shutdown 机制

例如：

- shutdown token
- broadcast channel
- ` run_until_cancelled `

优点：

- 适合生产环境优雅退出
- 不依赖内部 channel 是否都被正确 drop

缺点：

- 这是运行时设计增强，不是这次测试超时的最小修复
- 需要额外 API 和生命周期设计

### 方案 5：统一封装测试用 RouterHarness

把：

- request/event channel 创建
- sender drop
- router spawn / join

统一塞进一个测试 helper。

优点：

- 后续写集成测试不容易再踩同样的坑

缺点：

- 需要额外维护测试基建

## 建议

当前建议分两步：

1. 保持现在的修复方式：
    - 测试里显式 drop 无用 sender
    - ` Router ` 正确处理 ` agent_event_rx ` 关闭
    - ` Router ` 的正常停止通过 ` run_until_shutdown(...) ` 驱动
2. 后续如果 Router 生命周期会越来越复杂，再补一个正式的 worker 重启机制

这样能把“测试构造问题”和“运行时关闭缺陷”两件事分开处理，不会再把它们混成同一个超时症状。
