# 项目目标
OpenJarvis，一个能在多平台中远程调用、交流、执行任务的智能助手

# 项目设计
外部平台(feishu/telegram/discord等) 作为channel接入项目的中转router
router将`IncomingMessage` 找到对应的 `Thread` 聊天记录上下文 分发给agent agent执行完后返回结果`OutgoingMessage` 转发给用户

Agent具备以下能力
- Skill：可以动态加载skill去执行专门的任务
- MCP：通过MCP配置文件调用对应的mcp服务
- compact：模型上下文压缩机制
- browser：浏览器操控能力
- acp：和其他agent进行通信 指派任务的能力
- Command：能通过command在飞书中执行对应的命令
- Memory：长周期记忆模式
- 知识图谱：支持上传文档 进行知识图谱的记录