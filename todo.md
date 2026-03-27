- [ ] Session的postgralsql落盘
- [ ] agent的Outgoing返回
- [ ] 飞书发送的限流阀（延时 消息缓冲层？
- [ ] hook改为agenthook 因为可能有其他种hook后面要添加
- [x] session manager是全局的 
- [ ] 外部user传入消息 通过名字对content序列化一下
- [ ] session/turn/thread移动到一个模块中
- [ ] 目前turn是全量的 后续需要改成增量吗？
- [ ] 全面异步落盘的日志库
- [ ] /? 列出全部Commands
- [ ] command 能够解析用户输入转换为格式化的schema从而进行execute
- [ ] tiktoken + 占用图
- [ ] llm provider掉线重试方式？
- [ ] searching tool tavily brave metaso
- [ ] 需要有一个thread级别的全局状态来管理auto-compact 这种feature的开放 另外这个可以被Command打开 /auto-compact enable
- [ ] 飞书不是先react再回复的
- [ ] 压缩上下文的prompt注入

# clean
- [ ] sidecar