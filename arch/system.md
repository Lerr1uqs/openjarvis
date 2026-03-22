# openJarvis整体架构设计

第一层外部输入层：
- channels 封装(飞书机器人 tg discord)
- gateway: ws/http/sse 管理员控制面板
- 管理员 cli 命令管理工具 可以控制任何配置更改 另外配置是热更新的TODO

都变成 IncomingMessage
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

## 外部输入层

### channel
channel是独立线程 通过配置文件注册 config.yaml
```
channel:
    feishu:
        appid:
        secret:
        admin
```

ChatChannel trait:
- on_start
- on_message_incoming -> IncomingMessage
- on_message_outgoing(OutgoingMessage)
- on_end
- check_health

所有外部channel的消息走到统一的ChannelRouter组件进行路由 ChannelRouter和channel进行双向通信 agent也和这个去通信


channels -> router -> worker -> SessionManager 拼历史 -> MessageContext -> AgentLoop

## 组件
- agent沙箱环境 AgenticWorker 封装沙箱和agent context (沙箱实现待定 先实现一个空壳) agent的组件只有一个实例 所有user的消息都会走入这个 但是有权限管控
sessions[channel:userid] -> Session

- SessionManager: 变量 包装下面这些概念 [单一变量 一般不会出现多次]
    - Session: 一个用户的上下文回话空间
        - Thread: 一个用户和一个agent单次交互的全部聊天记录
            - Turn: 用户输入 + AgentLoop循环完成的一轮Message


所有agent先走默认的配置 也是在配置文件里
```
llm_provider:
    agent:
        default:
            protocol: anthropic/openai
            base_url: ...
            api_key: ...
```

Worker: 包装沙箱容器 + AgenticLoop 
```
struct Worker {
    pub sandbox;
    struct AgenticLoop {
        tools: List[Tool],
        mcp: List[mcp],
        hook: AgentHook
    }
    pub router_tx; // 发送数据回给router
}
```

AgentHook: 从配置文件中加载hook
```
agent:
    hook:
        pre_tool_use: ["echo", "hello"] # 后期支持参数注入 现在先只支持脚本
```
- worker.run(messages)
- agenticloop: agentic_loop.run(user_context{info+messages}) # 记得对tools的权限做检查
    - 绑定配置里的provider
    - 存在hook：比如使用工具前使用工具后的自定义hook 查看hook.md
```
async def run(user_context, ):

    user, messages = user_context
    while True:
    
        response, tool_calls = llm.generate(messages)
        self.router_tx.send(// 对方有这个tx和user_info进行绑定的数据结构
            AgentEvent(
                type="text_output",
                response
            ),
            ...
        )
        
        messages = messages + response
        if not tool_calls:
            break
            
        for tcall in tool_calls:
            self.router_tx.send(// 对方有这个tx和user_info进行绑定的数据结构
                AgentEvent(
                    type="tool_call",
                    tcall
                ),
                ...
            )
            tool_res = tool_register.call(tool_use, user)
            self.router_tx.send(// 对方有这个tx和user_info进行绑定的数据结构
                AgentEvent(
                    type="tool_result",
                    tool_res
                ),
                ...
            )

```

Context: 对Message的组织概念
- agent只接受ChatMessage(asistant/system/tool_result这些)
- 对各种类型的ChatMessage做上下文组织
```
MessageContext = order{
    SYSTEM: List[ChatMessage],
    MEMORY: List[ChatMessage],
    CHAT: List[ChatMessage], 
}
```

Command:
    - 用户传入的消息会先被Command组件截取 如果发现是注册的Command比如 `/approve` 开头 就会执行对应的命令而不需要执行相关上下文 Command的使用方式也能转换为docs暴露给agent 从而让agent 通过 openjarvis命令行去执行比如 `openjarvis command approve --channel feishu --username sakiko` 但是要通过某种方式确认当前cli是哪个agent执行的 agent有当前用户交互的userid来判断有没有权限

Cron: 定时器

memory: 本地内存crud 长短期 记忆命中机制
- 提供memory_search + memory_write两套逻辑 当前只支持文本匹配或者bm25改进版 未来支持qmd 
- 写在当前的./.memory/... 下面
- 其中MEMORY.md是永久性记忆 要加载到上下文里 其他是搜索性质的记忆

LLMProvider: 兼容openai和anthropic协议 允许配置不同的提供者

CLIAbility: 注册使用的cli能力工具

ToolRegistry: 基本工具的使用 bash memory等等

## 记忆


## 安全
安全认证 + 工具审批 + 防注入能力？

限制agent在群聊的能力

通过admin_token进行单人管理 登录后端界面

## 评估

## 其他功能

### auto-compact

### compact

参考claude和codex？再提供一点点机制