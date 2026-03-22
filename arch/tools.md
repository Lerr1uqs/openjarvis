遵循pi-agent的四个工具调用 read/write/edit/bash

- read(path, start_line, end_line)
- write(path, content)
全量覆盖写
自动创建目录
- edit(path, old_text, new_text)
是“字符串完全匹配替换”
- bash(command, timeout) 
后期可能会加上后台任务


抽象层：

```
type Tool = {
  name: string
  description: string
  input_schema: JSONSchema
  execute: (args) => Promise<string>
}
```