# 组件模型

这组文档只讲当前系统里真正有运行时语义的组件，只回答四件事：它是什么、负责什么、边界在哪、怎么接入。

主链路：

`channel -> router -> session -> thread -> agent -> tool/compact/llm`

文档索引：

- `channel.md`
  外部平台接入模型。
- `router.md`
  总编排与转发模型。
- `session.md`
  会话解析与持久化边界。
- `thread.md`
  线程级事务宿主与运行时状态。
- `memory.md`
  工作区级本地长期记忆模型。
- `context.md`
  送给 LLM 的消息组织模型。
- `command.md`
  线程级斜杠命令模型。
- `agent.md`
  Agent 执行模型。
- `agent/README.md`
  Agent 子模块索引。
- `compact.md`
  线程级上下文压缩模型。
- `llm.md`
  模型协议适配边界。
- `agent/tool.md`
  工具注册与可见性模型。
- `agent/tool/toolset.md`
  渐进式工具集模型。

不单独展开的内容：

- builder、helper、兼容层
- 纯测试类型
- 还没有形成稳定语义的 TODO 设计

配置约定：

- `AppConfig` 是启动期事实，不是线程级运行时组件。
- 启动主链路可以把最终配置安装成进程级只读快照，再由顶层装配入口读取。
- 单测、嵌入式调用和局部隔离场景优先继续使用显式配置构造入口，不依赖全局单例。
