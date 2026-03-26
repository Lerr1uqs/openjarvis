
# 整体架构
- 外部输入层: channel 对飞书 telegram等的封装 对服务器建立长连接并返回IncomingMessage
    - channel运行在各自的异步任务中 多个channel被包装为StreamMap
- 路由层: Router 是连接 Agent 跟多个 Channel 的桥梁，它们是双向连接 Agent也是运行在一个独立的异步任务中
    - Router是运行在主线程中 作为持续loop一直运行
    - Router接收到消息后 通过SessionManager进行处理
    - Router可以和Agent建立不同信道的链接 不局限于两个 用于处理不同的任务
- 处理层: AgentWorker接收到消息 开始串行执行处理任务

# 组件
## Router
主要负责各类事件 消息的转换和转发(transform+forward) 并不直接进行进行逻辑执行

## Channel
外部输入点 向外部服务建立长连接 接收消息并发送出去
```rs
pub struct IncomingMessage {
    /// Unique message ID.
    pub id: Uuid,
    /// Channel this message came from. (e.g. cli, feishu, telegram)
    pub channel: String,
    /// User identifier within the channel.
    pub user_id: String,
    /// Optional display name.
    pub user_name: Option<String>,
    /// Message content.
    pub content: String,
    /// Thread/conversation ID for threaded conversations.
    pub thread_id: Option<String>,
    /// When the message was received.
    pub received_at: DateTime<Utc>,
    /// Channel-specific metadata.
    pub metadata: serde_json::Value,
    /// File or media attachments on this message.
    pub attachments: Vec<IncomingAttachment>,
}
```

## SessionManager

用户和Agent交流的Message的上下文件都放在这个里面 因为Agent是无状态的 会话历史需要从中获取

- SessionManager: 变量 包装下面这些概念 [单一变量 一般不会出现多次]
    - Session: 一个用户的上下文回话空间
        - Thread: 一个用户和一个agent单次交互的全部聊天记录
            - Turn: 用户输入 + AgentLoop循环完成的一轮Message

通过 uuid:channel:external_thread_id来定位已有的thread
注意thread第一次遇到会创建 后续会使用创建的thread (单一uuid)
后期这些都会落盘到postgres sql中去(TBD)

另外有一个SessionStrategy 负责做会话保存的策略 比如turn只保留五个 多余进行丢弃(暂时)
session message实现两个接口：一个是 load_turn，一个是 store_turn。

1. store：存储新增的消息。
2. load：加载当前传入的 history。

目前的策略是：message 只存储最新的 5 个，多余的就丢弃。

TODO:
- [ ] 后面要加入对初始加入的审批

## Agent

### AgentWorker
Worker: 包装沙箱容器 + AgenticLoop

### AgenticLoop
负责Agent整个react循环和执行 还有事件外放给Router
详细情看: hook.md
### AgentHook: 从配置文件中加载hook
```yaml
agent:
    hook:
        pre_tool_use: ["echo", "hello"] # 后期支持参数注入 现在先只支持脚本
```

### Context: 对Message的组织概念

agent只接受ChatMessage(asistant/system/tool_result这些)
对各种类型的ChatMessage做上下文组织 可以导出为Vec<ChatMessage>
MessageContext = order{
    SYSTEM: List[ChatMessage],
    MEMORY: List[ChatMessage],
    CHAT: List[ChatMessage], 
}

## Command
用户传入的消息会先被Command组件截取 如果发现是注册的Command比如 /approve 开头 就会执行对应的命令而不需要执行相关上下文 Command的使用方式也能转换为docs暴露给agent 从而让agent 通过 openjarvis命令行去执行比如 openjarvis command approve --channel feishu --username sakiko 但是要通过某种方式确认当前cli是哪个agent执行的 agent有当前用户交互的userid来判断有没有权限
- CommandMessage不会进入Session
- Command返回消息 `[Command][${name}][SUCCESS/FAILED]: ${对应的执行结果}`

## Cron: 定时器

## memory: 本地内存crud 长短期 记忆命中机制

提供memory_search + memory_write两套逻辑 当前只支持文本匹配或者bm25改进版 未来支持qmd
写在当前的./.memory/... 下面
其中MEMORY.md是永久性记忆 要加载到上下文里 其他是搜索性质的记忆

## LLMProvider
兼容openai和anthropic协议 允许配置不同的提供者

## CLIAbility
注册使用的cli能力工具

## ToolRegistry
基本工具的使用 bash memory等等

目前是tool提供了thread级别的工具集加载 (后面可能会改？)

### load_toolset/mcp
我想设计一个渐进式加载的 tool set（或者是MCP，它本质也是一个工具集）。

采取这种方式的原因是，如果一次性把所有工具（非基本工具）都加进去，会导致上下文膨胀。所以我想采用这种渐进式加载和下放的机制：用完就去掉，不占位。

具体实现思路如下：
1. 先提供一个 prompt，告知其中包含哪些工具集以及相关的基本描述。
2. 将这些描述放在里面，并提供一个 loader 工具。
3. 如果需要使用，就通过该 loader 工具进行加载。
4. 用完了unload


### builtin tools
遵循pi-agent的四个工具调用 read/write/edit/bash

- read(path, start_line, end_line)
- write(path, content)
全量覆盖写
自动创建目录
- edit(path, old_text, new_text)
是“字符串完全匹配替换”
- bash(command, timeout) 
后期可能会加上后台任务


抽象层：

```
type Tool = {
  name: string
  description: string
  input_schema: JSONSchema
  execute: (args) => Promise<string>
}
```

### MCP
MCP 归属在 ToolRegistry 内部统一管理 配置入口是 `agent.tool.mcp.servers`

- 当前只支持 `stdio` 和 `streamable_http`
- MCP tool 会以 `mcp__<server>__<tool>` 的形式暴露给模型
- 只有 healthy + enabled 的 MCP server 才会把 tools 挂进 ToolRegistry
- 运行时启停/刷新/查询通过 `runtime.tools().mcp()` 暴露给其他组件
- demo-only MCP server 也放在 tool 模块下 通过内部子命令启动用于测试
- ./config/openjarvis/mcp.json 作为目前默认的配置文件

## skill
- 支持用户配置skill 选择skill 下载skill

## compact

## memory

除非用户说主动记忆 否则都强制是被动记忆 这个写入记忆调用的prompt
### active
当用户输入里出现keyword的时候自动注入记忆
add_active_memory("keyword", "memory....") 注意一个messages中只能有一个对应记忆(不需要多次注入 这个需要ut 确保多次注入不会多次出现在messages中)
如何防止多次注入？

我觉得可能就是遍历这个 messages，然后看它匹配前几个。如果说要简单的话，你就直接开一个这种遍历，或者说多线程去直接匹配前面的那个字符串是否相等。

只要第一个字符不相等，那其实就不是了。然后如果前面那个字符串匹配相等，且它整个长度也相等，那就是对的。

不知道有没有这样现成的库或者 API？

remove_active_memory 这个是写入 ~/.openjarvis/memory/active/memory.json 中的
另外可能还得记录一下时间什么的

### passive
~/.openjarvis/memory/passive/{daily,history,perference}
- daily是每日总结
- history是用户要求agent记住的内容
- preference是任务结束后提炼出来的用户偏好 
这三种记忆都是可开关的 也可以使用格外的provider来做 
下面api都是对: ~/.openjarvis/memory/passive/里面的查询
- search_memory("kw1,kw2,kw3") 
- memory_get("daily/2025-12-01.md") # 返回文档内容

memory后端搜索有两种 一种是关键词匹配 + 基于词频的搜索 另外一种是qmd 需要有qmd支持(未来)

# TODO
记忆
安全
安全认证 + 工具审批 + 防注入能力？

限制agent在群聊的能力

通过admin_token进行单人管理 登录后端界面

## Agent通信协议

A2A ACP (TBD)

## 浏览器使用

- 浏览器和playwright使用和剪裁协议

## 评估
## 其他功能 TBD
auto-compact
compact
参考claude和codex？再提供一点点机制






## TODO
打印飞书消息
