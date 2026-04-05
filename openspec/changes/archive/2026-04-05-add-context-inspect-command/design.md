## Context

当前仓库中线程命令能力集中在 `src/command.rs`，内建命令只有 `test`、`equal`、`echo`、`clear`。与此同时，上下文容量估算已经存在于 `compact/budget.rs` 与 AgentLoop 的 auto-compact 逻辑中，但它只在发起模型请求时临时计算，并通过 `<context capacity ...>` prompt 暴露给模型，不直接暴露给用户。

这次需求本质上不是“新增一套 budget 机制”，而是把已有的估算能力以线程级只读命令的形式公开出来。实现时需要遵守两个现有约束：

- 命令路径当前只持有 `IncomingMessage` 与 `Thread`，不直接持有 `ToolRegistry`
- 很多测试不会安装 global config，因此命令实现不能强依赖 `global_config()`

## Goals / Non-Goals

**Goals:**

- 提供 `/context`，让用户查看当前 thread 的总体上下文占比和 bucket 拆分。
- 提供 `/context role`，让用户查看当前 thread 中每条 persisted message 的估算占用。
- 与现有 `ContextBudgetEstimator` 共用同一套 deterministic 估算口径，避免两套结果打架。
- 命令保持只读，不改变线程消息或线程状态。

**Non-Goals:**

- 不提供 provider 真实 tokenizer 级别的精确 token 结果。
- 不在首版把运行时可见工具 schema token 一并纳入 `/context` 命令结果。
- 不新增新的 compact 控制命令或自动化压缩行为。

## Decisions

### 1. `/context` 直接复用 `ContextBudgetEstimator`，但只基于当前线程 persisted messages 估算

`/context` 摘要直接调用现有 `ContextBudgetEstimator`。输入消息使用当前线程 `thread_context.messages()`，可见工具列表先传空切片，因此结果会包含：

- persisted system messages
- persisted non-system chat history
- reserved output tokens

但不会把运行时工具 schema 的 `visible_tool_tokens` 纳入首版命令结果。

这样做的原因是：当前命令路径没有 `ToolRegistry`，而用户明确要看的是“当前 thread 上下文容量占比”。如果为了补 `visible_tool_tokens` 在命令侧额外构造工具运行时，会把一个只读命令做成跨模块耦合点。首版先把 thread 本身持有的 message 压力暴露出来，口径清晰且易于验证。

备选方案：

- 在命令路径中临时构造 `ToolRegistry` 后再估算完整 request budget。
  Rejected，因为这会把命令层和 agent runtime 进一步耦合，而且需要异步初始化更多运行时对象。
- 在命令侧重新实现一套 message token 估算逻辑。
  Rejected，因为会导致 `/context` 与 compact/auto-compact 的估算口径漂移。

### 2. `/context role` 输出逐条 message 明细，而不是只按 role 聚合

虽然子命令名叫 `role`，但实际输出按 persisted message 顺序逐条展开，每条至少包含：

- message 序号
- message role
- estimated tokens
- 占 `context_window_tokens` 的百分比
- 截断后的内容预览

原因是用户明确要看“各个 message 的容量占比”。如果只按 role 聚合，无法定位到底是哪条 user/assistant message 变重。

备选方案：

- 只返回按 role 聚合的 bucket 统计。
  Rejected，因为无法回答“哪一条 message 最重”这个核心排障问题。

### 3. 命令参数只支持空参数和 `role`

`/context` 支持两种模式：

- `/context`
- `/context role`

任何其他参数都直接返回 usage 错误。这样可以保持命令行为稳定，也为未来扩展 `/context <subcommand>` 留出明确边界。

### 4. 配置读取优先使用已安装的 global config，缺失时回退到默认配置

生产启动路径已经会安装 global config，但大量单元测试不会。命令实现通过 `try_global_config()` 探测是否已安装配置：

- 若已安装，则使用真实 `llm.context_window_tokens` 和 `max_output_tokens`
- 若未安装，则回退到 `AppConfig::default()` 构造估算器

这样既保证线上口径正确，也让命令层可以在纯单测环境下稳定运行。

## Risks / Trade-offs

- [首版不统计 `visible_tool_tokens`，结果会低估完整 request budget] → 在命令输出中显式展示 bucket 明细，首版固定表现为 `visible_tool_tokens=0`，避免误解成“全请求精确值”。
- [逐条 message 明细在线程很长时会比较长] → 每条只保留短 preview，并以单行摘要输出，避免把整个历史再次回显。
- [preview 里可能包含换行或过长文本] → 输出前统一压平空白并做固定长度截断，保持命令返回可读。
