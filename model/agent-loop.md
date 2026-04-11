# AgentLoop

这个文件是快速总览版，详细边界以 ` model/agent/loop.md ` 为准。

## 定位

- `AgentLoop` 是一次 agent turn 的执行器。
- 它是一个执行框架 身份、工具、能力和上下文都来自：`Thread`
- 接受incoming message 进行loop 循环执行
- 它不拥有线程初始化和稳定 system prompt 注入的 ownership。

## 核心边界

- worker 负责 `init_thread()`。
- loop不要进行临时messages管理 所有message都commit到thread中 messages从thread中取出
- 负责对外发送event 在运行时执行hook

## 主流程

1. 外部传入状态 `Thread` + 用户输入消息 incoming message
2. commit消息到thread中
3. 调用 LLM
4. 执行工具并把 assistant/tool messages commit回 thread
5. 必要时触发 runtime compact
6. 其他feature执行
