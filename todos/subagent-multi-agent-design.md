# Subagent 与 Multi-Agent 编排设计

## 1. 背景

当前仓库已经具备以下基础：

- 线程级真相已经收口到 `Thread`
- 线程级 toolset 显隐已经收口到 `ToolRegistry + Thread`
- `CommandSessionManager` 和 `BrowserSessionManager` 已经提供了“按线程持有长生命周期运行态”的模式
- hook 和配置已经预留了 `subagent_start`、`subagent_stop`、`task_completed`、`permission_request`、`worktree_create`、`worktree_remove`
- 仓库里已经有一个进行中的 OpenSpec：`add-bubblewrap-execution-runtime`

但当前实现仍然不能直接支撑：

1. 主 agent 阻塞式委派 subagent 并等待完成
2. 多个 agent 异步并发执行并被主 agent 编排
3. 通过沙箱限制“某个 agent 只能写哪些文件”

原因不是缺一个工具，而是当前执行模型、事件模型、运行态隔离模型都还不够。

## 2. 当前问题

### 2.1 Worker 还是单入口串行执行器

当前 `AgentWorker` 自带 inbox，并在一个循环里串行消费所有 `AgentRequest`。  
这对 top-level router 请求是够用的，但对 subagent 会有两个问题：

- 父 agent 在工具里再投递 subagent 请求并阻塞等待时，容易和当前单 worker 模型互相卡住
- `AgentWorker` 同时负责 router 事件转发、线程初始化、loop 执行、commit 回报，职责太重，不适合作为“可重入执行内核”

结论：

- 不能直接在现有 `AgentWorker` 上叠一个 `spawn_agent`
- 必须先拆出一个可直接调用的执行器层

### 2.2 当前 tool 上下文只有 thread_id

当前 `ToolCallContext` 只有 `thread_id`。  
这在单 agent 模式下问题不大，但一旦引入 subagent，就会出现资源隔离问题：

- browser session 按 `thread_id` 复用
- command session 按 `thread_id` 复用
- 未来 memory/mcp/其他长生命周期运行态也会走类似模式

如果子 agent 和父 agent 共用同一个 `thread_id`：

- 子 agent 可能复用父 agent 已打开的浏览器
- 子 agent 可能读到父 agent 的命令会话
- 多个 agent 之间的工具副作用会互相污染

结论：

- 后续必须把“逻辑线程 id”和“本次运行实例的隔离 scope”拆开

### 2.3 沙箱还是占位

当前 `sandbox` 还是 `DummySandboxContainer`，而且：

- `read/write/edit` 直接访问宿主文件系统
- `bash` 直接启动宿主机 shell
- browser sidecar 与 stdio MCP server 也直接启动宿主进程

这意味着当前没有任何可信的 per-agent 文件权限边界。  
即使逻辑上给某个 subagent 配一个“允许写这些文件”的列表，也只能算业务校验，不算安全隔离。

结论：

- “按 agent 限定文件写权限”必须建立在统一执行层之上
- `add-bubblewrap-execution-runtime` 不是旁支，而是主前置

## 3. 设计目标

本设计只讨论本地单进程内的 agent orchestration，不讨论远程 A2A / ACP 协议。

本次目标：

- 支持 blocking subagent：主 agent 发起一个子任务并阻塞等待结果
- 支持 async multi-agent：主 agent 发起多个子任务，后续轮询或等待结果
- 支持 `fork_context` 和 `blank_context`
- 支持按 agent 绑定独立沙箱策略
- 支持按 agent 限定可写文件、可读文件、网络权限
- 保持当前 `Thread` 仍然是线程级真相

本次不做：

- 不在第一阶段引入远程 agent 协议
- 不在第一阶段引入跨进程分布式调度
- 不在第一阶段把 router 改成多层消息总线
- 不在第一阶段强制引入 git worktree

## 4. 核心设计

### 4.1 执行层拆分为三层

推荐把当前 agent 执行链拆成三层：

```text
Router / Command
    ->
AgentWorkerAdapter
    ->
AgentExecutor
    ->
AgentLoop
```

职责划分：

- `AgentWorkerAdapter`
  - 面向 router 的长生命周期 inbox
  - 负责 top-level 请求接入
  - 负责把执行结果翻译成现有 `AgentWorkerEvent`

- `AgentExecutor`
  - 负责 `initialize_thread + AgentLoop`
  - 是真正可被复用的执行内核
  - top-level agent 和 subagent 都复用它

- `AgentLoop`
  - 继续只负责单轮 ReAct
  - 不直接感知“这个请求来自 router 还是来自 subagent supervisor”

这样做之后：

- top-level 请求继续走 `AgentWorkerAdapter`
- subagent 直接调 `AgentExecutor`
- 阻塞等待不需要再把任务塞回同一个 worker inbox

### 4.2 引入 AgentExecutionContext

当前线程模型够用，但运行实例模型不够。  
需要新增一个请求级执行上下文，建议至少包含：

```text
AgentExecutionContext
- run_id
- parent_run_id
- root_run_id
- logical_thread_id
- tool_session_scope_id
- sandbox_profile_id
- context_seed_mode
- execution_mode
```

说明：

- `logical_thread_id`
  - 仍然来自当前 `Thread.locator.thread_id`
  - 表示这次执行属于哪个线程

- `tool_session_scope_id`
  - 表示这次运行实例的工具隔离域
  - browser / command / 未来其他长生命周期 runtime 应该按它隔离
  - 父 agent 和子 agent 默认不能共享

- `context_seed_mode`
  - `fork_context`
  - `blank_context`

- `execution_mode`
  - `top_level`
  - `subagent_blocking`
  - `subagent_async`

### 4.3 引入 SubagentSupervisor

需要一个专门的运行时组件统一管理 agent task。

推荐增加：

```text
SubagentSupervisor
- spawn(task_spec) -> task_id
- wait(task_id) -> task_result
- list(parent_run_id) -> task_summaries
- cancel(task_id)
- collect_finished(parent_run_id)
```

其中 `task_spec` 应至少包含：

- task 文本
- 是否 fork 当前上下文
- model / reasoning_effort override
- sandbox profile
- visible tool policy
- write scope policy

Supervisor 的职责：

- 创建 subagent 对应的 `AgentExecutionContext`
- 准备线程快照或空白上下文
- 调用 `AgentExecutor`
- 缓存任务状态与最终结果
- 触发 `subagent_start / subagent_stop / task_completed` hook

### 4.4 多代理能力做成 toolset，而不是 builtin

不建议把多代理工具做成 always-visible builtin。  
更合理的做法是新增一个按线程加载的 `multi_agent` toolset。

第一版工具建议：

- `spawn_agent`
- `wait_agent`
- `list_agents`
- `cancel_agent`

语义建议：

- `spawn_agent(wait=true)` 等价于 blocking subagent
- `spawn_agent(wait=false)` 等价于 async subagent
- `wait_agent` 支持单个或多个 task id
- `list_agents` 返回当前线程下与当前父运行相关的子任务摘要

这样可以复用当前 thread-managed toolsets 模型，也避免默认增加全部请求的工具可见体积。

### 4.5 事件出口从 Router 强绑定改成抽象 sink

当前 `AgentLoop` 使用的 `AgentEventSender` 是为“聊天回发”设计的，里面绑定了：

- channel
- reply target
- source message id
- session 维度元数据

这套结构不适合作为 subagent 运行期事件出口。  
建议抽象出统一事件 sink：

```text
trait AgentEventSink {
    emit(event)
}
```

第一版至少准备三种实现：

- `RouterEventSink`
  - 当前 top-level 请求继续回发聊天

- `BufferedTaskSink`
  - subagent 内部事件只进入任务缓冲区，不直接回聊

- `NoopEventSink`
  - 某些静默任务不关心过程事件

这样子 agent 可以：

- 不污染当前聊天
- 仍然保留完整事件轨迹供调试或总结

### 4.6 线程状态增加 agent task 子域

当前 `ThreadState` 只有：

- `features`
- `tools`
- `approval`

建议新增：

```text
ThreadAgentState
- task_summaries
- active_parent_runs
- last_completed_tasks
- policy_overrides
```

注意：

- 不建议把 live `JoinHandle` 或 live runtime object 直接塞进 `Thread`
- `Thread` 只保留可持久化的 declarative state
- live task handle 放在 `SubagentSupervisor`

同时必须同步修改 `SessionManager` 的冲突合并逻辑。  
当前 CAS 恢复只考虑了 `features` 和 `approval`，如果加入 `agent state` 却不扩展 merge，会出现状态覆盖。

## 5. 上下文继承策略

### 5.1 fork_context

`fork_context` 不是简单 clone 全部对象，而是：

- 复制当前 `Thread` 的持久化消息快照
- 复制当前轮已确定需要继承的 system prompt snapshot
- 不复用父运行的 live tool session
- 不复用父运行的 pending tool events

也就是说，fork 的是“语义上下文”，不是“live runtime”。

### 5.2 blank_context

`blank_context` 推荐保留：

- 当前 workspace / skill / tool catalog
- 当前 sandbox profile

不继承：

- 当前线程聊天历史
- 当前轮 live chat
- 当前 tool session

如果 blank subagent 仍然需要归属到当前线程，应在执行上下文里记录：

- 逻辑线程属于哪个 `Thread`
- 但实际请求消息不带历史

## 6. 沙箱与文件权限设计

### 6.1 必须建立在统一执行层上

per-agent 文件权限设计必须依赖 `ToolExecutionRuntime`。  
不能让各个 tool handler 自己做 allowlist 判断。

正确边界应该是：

```text
ToolHandler
    ->
ToolExecutionRuntime
    ->
LocalBackend / BubblewrapBackend
```

只有这样：

- `read/write/edit`
- memory repository
- browser sidecar
- stdio MCP
- command / shell

才会走同一套路径策略。

### 6.2 SandboxProfile

建议引入 `SandboxProfile`：

```text
SandboxProfile
- name
- readable_paths
- writable_paths
- executable_paths
- network_mode
- env_allowlist
- workdir_policy
```

第一版最关键的是：

- `readable_paths`
- `writable_paths`
- `network_mode`

### 6.3 文件权限策略建议

第一版建议只做显式白名单，不做复杂 deny/priority 规则。

例如：

```text
profile: planner
- readable_paths: [workspace/**]
- writable_paths: []

profile: worker-a
- readable_paths: [workspace/**]
- writable_paths: [workspace/src/agent/**, workspace/tests/agent/**]

profile: worker-b
- readable_paths: [workspace/**]
- writable_paths: [workspace/src/session/**, workspace/tests/session/**]
```

这能直接支撑：

- 主 agent 只负责规划，不允许写文件
- 不同 subagent 只改自己分配的目录

### 6.4 是否需要 worktree

第一版不强制需要 worktree。

因为当前需求的核心是：

- 限制谁能写哪些文件
- 支持多个 agent 并行执行

如果各 agent 写集不重叠，共享 workspace + per-agent writable allowlist 已经够用。

worktree 适合第二阶段：

- 当多个 agent 可能改同一路径
- 当需要更强隔离
- 当需要独立 git 状态与 diff 汇总

## 7. 推荐执行流程

### 7.1 blocking subagent

```text
Parent Agent
    ->
spawn_agent(wait=true)
    ->
SubagentSupervisor::spawn(...)
    ->
AgentExecutor::execute(...)
    ->
wait result
    ->
return summarized result to parent loop
```

要点：

- 父 agent 不再通过当前 worker inbox 再次投递请求
- supervisor 直接调 executor
- 结果以 tool result 形式回到父 loop

### 7.2 async multi-agent

```text
Parent Agent
    ->
spawn_agent(wait=false) x N
    ->
SubagentSupervisor creates N tasks
    ->
tasks run concurrently
    ->
Parent Agent later calls wait_agent / list_agents
```

要点：

- 父 agent 当前轮不必等全部完成
- 任务状态由 supervisor 持有
- 线程里只落 declarative summary

## 8. 配置改造建议

建议在 `agent` 配置下增加：

```yaml
agent:
  orchestration:
    enabled: true
    max_concurrent_subagents: 4
    default_timeout_ms: 180000
    default_context_seed_mode: "fork_context"
  tool:
    execution:
      environment: "sandbox"
      sandbox_profile: "planner"
      profiles:
        planner:
          readable_paths: ["./**"]
          writable_paths: []
          network: false
        worker_a:
          readable_paths: ["./**"]
          writable_paths: ["./src/agent/**", "./tests/agent/**"]
          network: false
```

这里不追求字段名最终定稿，重点是结构：

- orchestration 配置
- execution backend 配置
- sandbox profiles 配置

## 9. 分阶段落地顺序

### 阶段 1：执行层前置

先完成当前 OpenSpec：

- `add-bubblewrap-execution-runtime`

至少要做到：

- 文件与进程副作用全部收口到统一执行层
- `Local` / `Sandbox` backend 可切换
- sandbox 失败时显式报错

### 阶段 2：执行器解耦

新增：

- `AgentExecutor`
- 抽离 `AgentWorkerAdapter`
- 引入 `AgentEventSink`

完成后：

- top-level 请求仍可正常运行
- subagent 已经有复用执行内核

### 阶段 3：subagent 最小闭环

新增：

- `AgentExecutionContext`
- `SubagentSupervisor`
- `multi_agent` toolset
- `spawn_agent(wait=true)`

目标：

- 先跑通 blocking subagent
- 支持 `fork_context` / `blank_context`

### 阶段 4：异步编排

新增：

- `wait_agent`
- `list_agents`
- `cancel_agent`
- task 状态缓存和线程级摘要

目标：

- 支持多任务并发编排

### 阶段 5：per-agent 文件写权限

在统一执行层基础上补：

- sandbox profile
- per-task profile 选择
- writable/readable path allowlist

### 阶段 6：worktree 与审批

按需要追加：

- worktree 隔离
- 审批流程
- 更复杂的策略覆盖

## 10. 关键风险

### 10.1 不拆执行器就直接做 subagent

风险：

- 父子 agent 与当前单 worker 串行模型互相卡住
- 顶层 adapter 和内部执行器耦合更深

### 10.2 不拆 tool session scope

风险：

- 子 agent 复用父 agent 的 browser/command runtime
- 并发 agent 相互污染运行状态

### 10.3 不先做统一执行层就做权限控制

风险：

- 只有逻辑校验，没有真实安全边界
- 文件工具和 sidecar 仍可越权访问宿主环境

### 10.4 一上来就强推 worktree

风险：

- 改动面过大
- 和当前 bubblewrap 执行层改造互相缠绕
- 会拖慢 subagent 最小闭环落地

## 11. 推荐结论

推荐路线不是“直接新增 `spawn_agent` 工具”，而是：

1. 先完成统一执行层与 bubblewrap 沙箱
2. 再拆 `AgentExecutor`
3. 然后做 blocking subagent
4. 最后扩成 async multi-agent orchestration

如果只做第一版，建议目标收敛为：

- blocking subagent
- `fork_context` / `blank_context`
- 独立 `tool_session_scope_id`
- per-agent sandbox profile
- 显式 writable/readable path allowlist

这版已经足够支撑：

- 主 agent 规划
- 子 agent 并行或串行执行
- 每个 agent 只改自己被授权的文件

而且不会一次性把远程协议、worktree、审批、分布式调度全部拖进来。
