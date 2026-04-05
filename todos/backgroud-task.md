在执行项目的时候，有时候需要把它挂在后端，等它的结果返回。在此期间去做其他的事，最后再检查这个结果。

这就是所谓的后台任务。

```json
{  
  "name": "exec_command",  
  "description": "Runs a command in a PTY, returning output or a session ID for ongoing interaction.",  
  "parameters": {  
    "type": "object",  
    "properties": {  
      "cmd":              { "type": "string",  "description": "Shell command to execute." },  
      "workdir":          { "type": "string",  "description": "Optional working directory..." },  
      "shell":            { "type": "string",  "description": "Shell binary to launch." },  
      "tty":              { "type": "boolean", "description": "Whether to allocate a TTY..." },  
      "yield_time_ms":    { "type": "number",  "description": "How long to wait (ms) for output before yielding." },  
      "max_output_tokens":{ "type": "number",  "description": "Maximum number of tokens to return." },  
      ...
    },  
    "required": ["cmd"]  
  },  
  "output_schema": {  
    "type": "object",  
    "properties": {  
      "chunk_id":             { "type": "string" },  
      "wall_time_seconds":    { "type": "number" },  
      "exit_code":            { "type": "number" },  
      "session_id":           { "type": "number", "description": "Session identifier to pass to write_stdin when the process is still running." },  
      "original_token_count": { "type": "number" },  
      "output":               { "type": "string" }  
    },  
    "required": ["wall_time_seconds", "output"]  
  }  
}
```

write_stdin 工具（配合 exec_command 使用） output_schema 和exec_command 完全一致 主要是为了和后台任务交互和获取结果
```json
{  
  "name": "write_stdin",  
  "parameters": {  
    "properties": {  
      "session_id":       { "type": "number", "description": "Identifier of the running unified exec session." },  
      "chars":            { "type": "string", "description": "Bytes to write to stdin (may be empty to poll)." },  
      "yield_time_ms":    { "type": "number" },  
      "max_output_tokens":{ "type": "number" }  
    },  
    "required": ["session_id"]  
  },
  "output_schema": {
    ...
  }
}
```

另外还要提供一些接口以防备用。即使暂时用不到，也要预留这些能力，比如：
1. 能够列出所有任务
2. 提供任务完成的状态（例如 Doing 或 Done）
3. 其他后台任务相关信息

总之要预留这些接口，和他们在内存中的持久化能力。

验收方面可以这样做：

在 resource 里面写一个 TUI 程序。你可以随便用什么语言，但最好还是用 Rust，因为当前的项目就是 Rust。

用这个语言写一个命令行交互工具，并想办法利用 command 序列完成交互，最终达到一个状态。例如：
1. 第一次你输入 A，它返回给你一个数字
2. 第二次你输入 B，它再返回给你一个数字
3. 你把两次得到的数字提取出来并相加，再返回给程序

程序会计算结果是否一致，最后返回给你一个 OK（表示成功）或者失败。

这样做的话，虽然不是 A 键的操控，但你至少可以验证这两个工具使用的正确性。

然后这个可以代替现有的 Bash 工具

这种tui程序可以多设计几种场景 比如各种超时结束没结束之类的