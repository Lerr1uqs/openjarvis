# AgentLoop

这个文件是快速总览版，详细边界以 ` model/agent/loop.md ` 为准。

## 定位

- `AgentLoop` 是一次 agent turn 的执行器。
- 它消费“已经初始化好的 `Thread` + 当前 incoming message”。
- 它不拥有线程初始化和稳定 system prompt 注入的 ownership。

## 核心边界

- worker 负责 `init_thread()`。
- loop 负责 request-time runtime state。
- loop 内的临时 system messages 和 live chat messages 只存在于本轮局部变量。
- loop 结束时只把需要持久化的消息回收到 `Thread`。

## 主流程

1. 读取持久化 `Thread`
2. 准备 request-time tools 与预算
3. 追加当前用户消息
4. 调用 LLM
5. 执行工具并把 assistant/tool messages 追加回 working set
6. 必要时触发 runtime compact
7. 产出 commit messages 与更新后的 `Thread`
