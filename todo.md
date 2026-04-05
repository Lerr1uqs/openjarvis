- [x] Session的postgralsql落盘
- [ ] agent的Outgoing返回
- [ ] 飞书发送的限流阀（延时 消息缓冲层？
- [ ] hook改为agenthook 因为可能有其他种hook后面要添加
- [x] session manager是全局的 
- [ ] 外部user传入消息 通过名字对content序列化一下
- [ ] session/turn/thread移动到一个模块中
- [ ] 目前turn是全量的 后续需要改成增量吗？
- [ ] 全面异步落盘的日志库
- [ ] context子命令
- [ ] [openjarvis][tool_call] write 返回内容太多了 需要做truncate
```
  └ {
      "changeName": "add-command-session-tools",
    … +12 lines
    }
    - Generating instructions...
```
- [ ] jarvis还在处理的时候用户发消息 接收到用户消息了才应该react
- [ ] /? 列出全部Commands
- [ ] command 能够解析用户输入转换为格式化的schema从而进行execute
- [ ] tiktoken + 占用图
- [ ] llm provider掉线重试方式？
- [ ] searching tool tavily brave metaso
- [ ] 需要有一个thread级别的全局状态来管理auto-compact 这种feature的开放 另外这个可以被Command打开 /auto-compact enable
- [ ] 飞书不是先react再回复的
- [ ] 压缩上下文的prompt注入
- [ ] 如何发送image
- [ ] 旧的 CompactRuntimeManager 兼容缓存 取出
- [ ] /clear 会把该线程的 ChatMessages、tool events、loaded toolsets、/auto-compact 等线程级状态 ??? events是什么？
- [ ] mcp server as toolset?
- [ ] 及时的阻断命令
- [ ] 返回给用户图片
- [x] 主动记忆的keyword必须是非常专用的名字 不能瞎生成 比如JJJ喜欢xxx 会生成三个keyword 用户没说明的情况下需要先询问
codex resume 019d511c-18c5-70a1-9636-87c66f63bbb5

# clean
- [ ] sidecar

# tmp

› AGENTS.md 创建一个model文件夹 里面存放着当前各个组件模型的：边界 职责 概念 和 能力等等 其实就是告诉别人这个组件是怎么用负责什么的。只对关键概念进行说  
  明 并且要求精简不要废话 不要对builder helper之类的辅助函数去说明