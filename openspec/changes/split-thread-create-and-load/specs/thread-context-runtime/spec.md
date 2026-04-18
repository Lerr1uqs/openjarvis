## MODIFIED Requirements

### Requirement: 系统 SHALL 在线程初始化时将稳定 `System` messages 直接写入 `ThreadContext`
系统 SHALL 只在显式 `create_thread`、线程重初始化或清空后重建路径中，通过唯一的 `initialize_thread(thread, thread_agent_kind)` 入口，将基础 system prompt 与其他稳定初始化提示直接写入 `ThreadContext.messages()` 的开头前缀并持久化。系统 SHALL NOT 在 `load_thread`、`lock_thread` 或其他纯读取恢复路径中隐式补写这些稳定前缀。

#### Scenario: `create_thread` 首次准备线程时写入稳定 `System` 前缀
- **WHEN** SessionManager 首次为某个 internal thread 执行 `create_thread`
- **THEN** 系统会先构造该线程的稳定 `System` messages
- **THEN** 这些消息会直接写入 `ThreadContext.messages()` 的开头前缀
- **THEN** 后续同一线程的普通 user / assistant / tool 消息都会位于该前缀之后

#### Scenario: 纯加载路径不会补写稳定 `System` 前缀
- **WHEN** `load_thread` 或 `lock_thread` 恢复到一个尚未完成初始化的线程快照
- **THEN** 系统返回当前线程快照本身
- **THEN** 系统不会在读取过程中追加稳定 `System` messages
- **THEN** 调用方必须通过显式 create 或 reinitialize 路径完成初始化后再对外服务

### Requirement: 系统 SHALL 根据 `ThreadAgentKind` 选择预定义角色 prompt 和默认工具绑定
系统 SHALL 在 `initialize_thread(thread, thread_agent_kind)` 中根据 `ThreadAgentKind` 构造线程自己的 `ThreadAgent` 真相。`ThreadAgent` SHALL 至少记录线程 agent 类型以及该类型绑定的默认工具集合。系统 SHALL 使用这个 `ThreadAgent` 选择预定义 system prompt，并将其默认工具绑定并入线程初始化后的可用工具状态。

#### Scenario: `Main` 线程按主助手角色完成初始化
- **WHEN** 调用方以 `ThreadAgentKind::Main` 创建一个新线程
- **THEN** 系统会写入主助手对应的稳定 system prompt
- **THEN** 线程初始化后的默认工具状态包含该 agent 类型绑定的工具集合

#### Scenario: `Browser` 线程按浏览器角色完成初始化
- **WHEN** 调用方以 `ThreadAgentKind::Browser` 创建一个新线程
- **THEN** 系统会写入浏览器线程对应的稳定 system prompt
- **THEN** 线程初始化后的默认工具状态包含 `browser` 等该 agent 类型绑定的工具集合
- **THEN** 这些工具在首轮请求前就可按线程状态参与可见性投影

### Requirement: 系统 SHALL 从随程序打包的 markdown 文件加载预定义 thread-agent prompts
系统 SHALL 将 `Main` / `Browser` 这类预定义 thread-agent system prompt 模板存放在随程序打包的 markdown 文件中，并在初始化阶段通过编译期打包资源加载。系统 SHALL NOT 把这些长 prompt 模板直接硬编码为 Rust 源码字符串常量。系统 SHALL NOT 接受 runtime 或 config 传入自定义 thread system prompt 覆盖这些模板。

#### Scenario: Browser 预定义 prompt 来自打包的 markdown
- **WHEN** 系统以 `ThreadAgentKind::Browser` 初始化一个线程
- **THEN** 写入线程稳定前缀的 browser system prompt 内容来自随程序打包的 markdown 文件

#### Scenario: Main 预定义 prompt 始终来自打包的 markdown
- **WHEN** 系统以 `ThreadAgentKind::Main` 初始化一个线程
- **THEN** 写入线程稳定前缀的 main system prompt 内容来自随程序打包的 markdown 文件

#### Scenario: runtime/config 不接受自定义 thread prompt
- **WHEN** 调用方尝试通过 runtime builder 或配置传入自定义 thread system prompt
- **THEN** 系统拒绝该覆盖入口
- **THEN** 线程初始化仍只允许根据 `ThreadAgentKind` 选择随程序打包的 markdown 模板
