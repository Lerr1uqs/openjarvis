

```
fn agent_run(&mut thread, user_input) {
	
	if is_empty(&thread) {
		init_thread(&mut thread);
	}

    thread.begin_turn();
    thread.push_message(user_input);

	while True { // 这里是一轮turn 

		if Compactor::should_compact(&thread) {
            // 压缩完继续
			Compactor::compact_thread(&mut thread);
		}


		messages = thread.messages()
		tools = thread.runtime.tools()

		response, toolcalls = llm.generate(messages, tools)
		thread.push(response,  callback = ||{send event});

		for tc in toolcalls {
			res = toolcall(tc.args, &thread);
			thread.push(tc,  callback = ||{send event});
			thread.push(res, callback = ||{send event});
		}

		if toolcalls.is_empty() {
			break;
		}
	}

    thread.commit_turn(...);

}
```
1. messages 必须让 thread 来管理，并且不允许在多个变量里进行管理，messages就是一个Vec，不能拆分成什么system_prefix，另外也不能否手动设定一堆 xxx_messages 临时变量在loop里
2. system message 全部在 `init_thread()` 中注入，并且永远位于 thread 开头
3. turn 是一次用户输入后的loop内的多轮调用全部结束
4. 对外发送消息和 thread 持久化按 message 为单位进行 (codex官方就是这样的 参考官方的来) 产出一个message对外发送一个
5. router 负责发送 event，但不能参与 thread messages 序列处理

最严格的必须遵守的：
- messages就是一整个vec![message] 不允许在thread中设立什么system_messages memory_messages
