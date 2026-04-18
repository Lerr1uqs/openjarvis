# subagent改造 
要求：
- 复用agent loop
- locator 重构 从现在的 `user:channel:external_thread_id` 变为 `user:channel:external_thread_id:subagent:browser` 其中subagent和后面都是可选的
    - 对于mainagent 保持 `user:channel:external_thread_id` 不变即可
- 增加Agent的Kind::{MAIN,SUB}
- 增加 AgentRegister 注册mainagent subagent 对应的system prompt和工具
	- 各个agent的system prompt用md文件存放 load进来
- SubagentRunner负责处理 mainagent调用subagent 和 返回值结果
- 添加对应的spawn_subagent工具 `send_subagent` `close_subagent` `list_subagent`

- Thread要加一个 `ThreadAgent` 字段 来表示这个thread run的是什么agent

# subagent 生命周期
`yolo`/`persist` 
yolo是spawn一次 发送完一个任务就结束了 比如 search agent 查找一个东西 send之后的返回结果里就包含了已经结束
persist是spawn一次 上下文持久化保存 可以多次send_subagent去交互


