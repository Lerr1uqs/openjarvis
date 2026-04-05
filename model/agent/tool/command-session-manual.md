# Command Session Manual

## 定位

- `scripts/command_session_manual.rs` 是命令会话工具族的手工验收入口。
- 它的职责不是提供新的模型工具，而是给开发者一个稳定的本地验证面，用真实的 `exec_command` 和 `write_stdin` 串起后台命令轮询链路。

## 边界

- 负责启动一个持续输出 `A` 的命令，并把人工输入转换成一次空写 `write_stdin` 轮询。
- 负责复用真实 `ToolRegistry` builtin tools 验证 `exec_command / write_stdin` 的协同行为。
- 不负责生产流量接入，不负责成为 Router 或 AgentLoop 的正式入口。
- 不负责扩展命令会话协议本身；协议语义仍以 `agent/tool/command/*` 和对应 OpenSpec 为准。

## 关键概念

- `thread_id`
  手工验证使用的 internal thread 标识，用来保证这组工具调用在同一线程上下文里发生。
- `exec_yield_time_ms`
  首次 `exec_command` 的等待窗口，用来决定启动后多久返回首个 chunk 或 `session_id`。
- `poll_yield_time_ms`
  每次空写 `write_stdin` 的等待窗口，用来决定单次人工轮询愿意等待多久。
- 手工轮询
  终端里每输入一次任意字符并回车，都会触发一次 `write_stdin(chars = "")`，读取该 session 自上次成功读取以来的新输出。

## 验收标准

- 该脚本必须能以独立二进制方式运行，而不是依赖测试框架内部入口。
- 启动后必须先通过 `exec_command` 拿到一个仍在运行的命令会话 `session_id`。
- 用户输入任意字符后，脚本必须能成功发起一次空写 `write_stdin` 并打印返回摘要。
- 用户输入 `q` 或 `quit` 后，脚本必须能正常退出，不留下需要手工清理的失控后台命令。

## 调用入口

- 推荐调用方式：`cargo run --bin command_session_manual -- --exec-yield-time-ms 50 --poll-yield-time-ms 600`
- 如果只想确认参数面是否可用，可执行：`cargo run --bin command_session_manual -- --help`

## 使用约束

- 这个脚本面向人工验收，不替代自动化测试。
- 当命令会话工具语义变化时，应同步检查这个脚本是否仍然符合最新的手工验证路径。
