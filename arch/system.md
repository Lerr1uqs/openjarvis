
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
    /// 上游聊天平台提供的外部线程 ID。
    pub external_thread_id: Option<String>,
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
        - ThreadContext: 一个用户和一个agent在单条 internal thread 上的统一运行时宿主
            - ThreadConversation: 当前线程的全部聊天记录
                - Commit批次: 用户输入驱动的一轮原子消息提交，底层历史仍按 turn 边界保存
            - ThreadState: 当前线程的 feature / tool / approval 等运行时状态

这里没有单独的 conversation ID。

- `thread_key = user:channel:external_thread_id`
- `internal thread id` 由 `thread_key` 稳定派生，作为线程唯一内部标识
- `ThreadConversation` 只保存历史和审计信息，不再自带一份独立 conversation id

注意 thread 第一次遇到会创建，后续始终复用同一个 internal thread id。

当前 SessionManager 是 “内存热缓存 + SessionStore 持久化后端” 模型：

- 进程内缓存当前命中过的 Session 索引，以及每个 thread 对应的 live `ThreadContext`
- 每个 live `ThreadContext` 都有独立 thread-level mutex，允许按 locator 锁定并修改线程运行态
- cache miss 时会从 `SessionStore` 懒加载恢复 `ThreadContext`
- 线程写入走 write-through，会先完成 `ThreadContext` 快照持久化，再更新热缓存
- `ThreadContext` 快照写入带 revision/CAS，避免旧快照覆盖新状态

`SessionStore` 首版默认实现是 SQLite：

- 默认数据库路径: `data/openjarvis/session.sqlite3`
- 持久化内容: session 元数据、thread 快照 JSON、`external_message_id` 去重记录
- 未来如果切 PostgreSQL，只需要补一个新的 store backend，不改 Router / Command / AgentLoop 的调用路径

## ThreadContext

`ThreadContext` 是当前线程的统一事务边界。

它至少持有：

- `locator`: 当前线程定位信息
- `conversation`: `ThreadConversation`
- `state`: `ThreadState`

其中：

- `ThreadConversation` 只负责 turn / message / tool_event 等对话与审计历史
- `ThreadState` 负责线程级 feature、工具状态、权限审批状态以及后续其他 thread runtime metadata
- 当前线程身份统一来自 `locator.thread_id`，而不是 `ThreadConversation` 自己的 id 字段

线程级能力都应该优先挂在 `ThreadContext` 上，而不是散落到 `ToolRegistry`、`CompactRuntimeManager` 或各个工具内部。

持久化边界也以 `ThreadContext` 为准：

- 会持久化: `locator`、`conversation(turns)`、`state`
- 不会持久化: `pending_tool_events`、Router 排队状态、兼容缓存、live browser session 等纯运行时 attachment
- compact turn 必须按 `ConversationTurn` 原样保存，不能只存扁平 messages 再反推 turn 边界
- `external_message_id` 去重记录会单独落盘，保证重启后仍能识别重复消息

session message实现两个接口：一个是 `load_messages`，一个是 `commit_messages`。

1. commit：存储新增的消息批次。
2. load：加载当前传入的 history。

- 如果启用了 `compact`，runtime 会在真正请求 LLM 前基于完整请求预算判断是否压缩 `chat`

TODO:
- [ ] 后面要加入对初始加入的审批

## Agent

### AgentWorker
Worker: 包装沙箱容器 + AgenticLoop

### AgenticLoop
负责Agent整个react循环和执行 还有事件外放给Router
详细情看: hook.md

- `AgentLoop` 在进入线程执行时应该直接接收一个 `ThreadContext`
- 循环内部通过 `ThreadContext` 完成历史读取、工具可见性计算、工具调用、compact 状态读取和事件记录

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
- 所有 Command 都是线程级命令，必须先 resolve 目标线程，再通过对应 `ThreadContext` 读写线程状态
- `/auto-compact on|off|status` 是线程级 runtime command
- 它按 `channel + user_id + external_thread_id` 生效，不会全局影响其他线程
- 它可以覆盖 YAML 里的默认 compact/auto_compact 状态
- `on` 后后续轮次会开启 auto-compact 提示并暴露 `compact`
- `off` 后后续轮次会关闭这条线程上的 auto-compact 提示
- 这类 thread-scoped command 修改的是当前线程 `ThreadContext` 中的状态 而不是额外的全局 override 容器

兼容缓存如果仍存在，也只能从持久化后的 `ThreadContext` 单向重建，不能反向覆盖已经保存的线程快照。

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

`ToolRegistry` 是全局工具池 / 目录层，不是线程事务管理器。

它负责：

- 注册 builtin tools
- 注册 program-defined toolsets
- 管理 MCP server 与 skill registry
- 提供全局 tool / toolset / handler 的解析入口

它不负责长期持有线程自己的：

- loaded toolsets
- tool visibility projection
- compact/auto_compact 的线程开关
- 工具权限与审批状态

这些 thread-scoped 状态统一由 `ThreadContext` 管理，然后再由 `ThreadContext` 去调用全局 `ToolRegistry`。

### load_toolset/mcp
我想设计一个渐进式加载的 tool set（或者是MCP，它本质也是一个工具集）。

采取这种方式的原因是，如果一次性把所有工具（非基本工具）都加进去，会导致上下文膨胀。所以我想采用这种渐进式加载和下放的机制：用完就去掉，不占位。

具体实现思路如下：
1. 先提供一个 prompt，告知其中包含哪些工具集以及相关的基本描述。
2. 将这些描述放在里面，并提供一个 loader 工具。
3. 如果需要使用，就通过该 loader 工具进行加载。
4. 用完了unload

这里的加载状态属于当前线程自己的 `ThreadContext`，不是 `ToolRegistry` 自己的线程 map。


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
- MCP server 的生命周期仍然归属全局 `ToolRegistry`
- 但某个线程当前是否加载某个 MCP toolset 以及是否对模型可见 由该线程的 `ThreadContext` 决定

## skill
- 支持用户配置skill 选择skill 下载skill

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


## agent context容量
对最终送给 LLM 的完整请求做容量估算，而不是只看 chat。  
容量估算至少拆成下面几部分：

- `system_tokens`
- `memory_tokens`
- `chat_tokens`
- `visible_tool_tokens`
- `reserved_output_tokens`
- `total_estimated_tokens`
- `context_window_tokens`
- `utilization_ratio`

这里的 `visible_tool_tokens` 只统计当前线程当前时刻真正对模型可见的工具，不统计已经注册但当前不可见的工具。

对外可以提供当前线程的 context budget 查询接口，用于：

- runtime 判断是否需要 compact
- `auto_compact` 开启后给模型注入容量信息
- 后续给用户或管理端展示当前线程的上下文占用情况


## compact
`Compact` 本身是线程级上下文管理器，不是天然的 tool。  
它的职责是在每次真正请求 LLM 之前，先根据当前线程的上下文预算判断是否要压缩 `chat`。

几个边界先明确：

- compact 只作用于 `chat`
- `system` 和 `memory` 不参与 compact
- `chat` 本身天然包含 user / assistant / tool_call / tool_result
- compact 结果必须写回 message 历史，而不是放进 memory

压缩流程：

1. runtime 在发送下一次 LLM 请求前估算完整请求 token。
2. 如果达到 compact 阈值，则触发线程级 compact。
3. compact provider 基于当前线程全部历史 `chat` 生成一个 compacted turn。
4. 这个 compacted turn 由两条普通 message 组成：
   - compacted `assistant` message：明确说明“这是压缩后的上下文”，并记录任务目标、用户约束、当前背景、当前规划、已完成、未完成、关键事实
   - follow-up `user` message：固定写入“继续”，用于把对话重新续接回正常节奏
5. 首版直接把被 compact 的旧 `chat` 从 active history 中移除，并用这个新的 compacted turn 替换。
6. 这个 compacted turn 后续仍然作为普通 chat history 参与继续对话，也会参与下一次 compact。

Compact 的核心不是“少一段文本”，而是“把当前任务恢复所需的最小上下文重新写回对话”。

首版策略：

- 使用 `CompactStrategy` 抽象统一管理压缩策略
- 默认策略先用 `CompactAllChatStrategy`
- 也就是到阈值后直接把当前线程全部历史 `chat` 压成一个 compacted turn

历史存储首版先采用直接替换：

- active history 中不保留被替换的旧 chat
- 但 compact 逻辑需要预留未来扩展点，后面可以切成：
  - archive source
  - keep shadow copy
  - keep recent turns 等其他策略

也就是说 V1 先做“替换”，但接口和策略层不要把“只能替换”写死。

### auto-compact
`AutoCompact` 是基于 `Compact` 的可选增强能力。  
不开启 `AutoCompact` 时，runtime 仍然会在需要时自动 compact，只是模型本身无感知。

开启 `AutoCompact` 后才做两件事：

1. 每次 generate 都给模型注入当前线程的上下文容量提示
2. 始终暴露 compact tool，让模型自己决定是否提前 compact

所以这里要区分两层：

- `Compact`: runtime-managed，上下文过大时系统自己压
- `AutoCompact`: model-assisted，让模型提早决定压缩时机

`AutoCompact` 的提示语义是固定存在的，不是“到了阈值才出现”：

- `auto_compact = false` 时，对模型不可见
- `auto_compact = true` 时，compact tool 对当前线程始终 visible
- 每次 generate 都会追加一段运行时提示，至少暴露当前上下文使用率，例如 `<context capacity 42.3% used>`
- 提示里会明确说明 `compact` 工具当前可用，并告知 `compact` 只压缩 `chat`
- `tool_visible_threshold_ratio` 不再控制是否可见，而是控制提示是否升级为“应尽快 compact”的提前告警
- 如果已经达到硬阈值，则 runtime 仍然可以直接先 compact；这和模型侧是否主动调用是两层机制
- 用户也可以通过 `/auto-compact on|off|status` 对当前线程做 runtime 级开关，并收到命令回复确认

为了支持这件事，线程级 compact 特性状态和工具可见性判断都应该收口到 `ThreadContext`。`ToolRegistry` 只提供全局 `compact` 工具定义或其他工具定义的解析能力，不再自己保存 thread-scoped compact projection。

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
