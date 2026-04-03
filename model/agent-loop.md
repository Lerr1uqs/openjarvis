thread_init函数 如果thread没有初始化 就进行初始化 进行system prompt的注入

runtime.tool负责工具管理 包括工具的可见性

```rust
agent loop {

   if not inited {
      thread_init(&mut thread/thread id)
   }

   if Compactor::need_compact(&thread.messages()) {
      Compactor::run(&thread)
   }

   if active_memory_enabled {
      unimplmented!
   }

   if auto_compact_enabled {
      thread.append(ChatMessage(
         // 类似于 "asistant: 当前的上下文已使用 [77%]",
         Context::budget(thread.messages()).into()
      ))
   }

   thread.append(user_input)

   tools = runtime.tools()

   response = llm.generate(messages, tools)

   for tool in tools {
      # runtime能在内部调用 Compactor的服务 而不需要做特化
      runtime.call_tool(..., extra=thread_locator)
   }
}
```

runtime要提供 close_tool, open_tool list_tool等能力