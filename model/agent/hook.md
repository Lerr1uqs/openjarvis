# Hook

## 定位

- `hook` 是 Agent 生命周期观测层。
- 它允许在不改主执行流程抽象的前提下，对关键事件挂接脚本或其他处理器。

## 边界

- 负责事件定义、handler 注册、顺序触发。
- 不负责业务决策，不负责线程持久化，不应该替代工具调用。

## 关键概念

- `HookEventKind`
  统一事件名，如 `pre_tool_use`、`post_tool_use`、`pre_compact`、`notification`。
- `HookEvent`
  一次事件实例，包含 kind 和 payload。
- `HookHandler`
  事件处理接口。
- `HookRegistry`
  Hook 注册和分发中心。

## 核心能力

- 从 `agent.hook` 配置加载脚本型 hook。
- 按注册顺序依次触发 handler。
- 通过环境变量把事件名和 payload 传给外部脚本。
- 对超时和非零退出码做失败上抛。

## 使用方式

- Hook 适合做观测、通知、外部联动。
- 如果能力本质上需要被模型主动调用，应做成 tool，而不是 hook。
