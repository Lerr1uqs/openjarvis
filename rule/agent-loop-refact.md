

```
fn agent_run(&mut thread, user_input) {
	
	if is_empty(&thread) {
		init_thread(&mut thread);
	}


	while True { // 这里是多轮turn 

		if Compactor::should_compact(&thread) {
            // 压缩完继续
			Compactor::compact_thread(&mut thread);
		}

        thread.begin_turn();
        thread.push_message(user_input);

		messages = thread.messages()
		tools = thread.runtime.tools()

		response, toolcalls = llm.generate(messages, tools)

		thread.push(response);

		for tc in toolcalls {
			thread.push(tc);
			res = toolcall(tc.args, &thread);
			thread.push(res);
		}

        turn_events = thread.finalize_turn_events();
        session.commit_thread_turn(thread.snapshot(), ...); // 持久化边界按 turn
        router.send_turn_events(turn_events); // router 负责发 event，但不负责拼 message

		if toolcalls.is_empty() {
			break;
		}
	}

}
```
1. messages 必须让 thread 来管理，而不是手动设定一堆 live 临时变量
2. system message 全部在 `init_thread()` 中注入，并且永远位于 thread 开头
3. turn 是“一次输入及其对应的全部输出”，不是内部单次 loop 迭代
4. 对外发送消息和 thread 持久化按 turn 为间隔进行
5. loop 产 turn 级 event batch，router 负责发送 event，但不能参与 thread messages 序列处理
6. turn的本质就是一个input输入 对应一个输出和多个toolcall的一轮llm的交互
