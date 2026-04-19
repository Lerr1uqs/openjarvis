## ADDED Requirements

### Requirement: sandbox 开启时 core 文件工具 SHALL 通过 JSON-RPC proxy 执行
当当前 worker 安装了非 `disabled` 的 sandbox 后，`read`、`write`、`edit` SHALL 通过 sandbox proxy 完成文件读取与写回，而 SHALL NOT 继续直接操作宿主文件系统。

#### Scenario: read 通过 sandbox proxy 读取 workspace 文件
- **WHEN** 当前 worker 使用 `bubblewrap` sandbox，且线程调用 `read`
- **THEN** 系统 SHALL 通过 JSON-RPC proxy 读取目标文件内容
- **THEN** 返回内容 SHALL 与 sandbox 内实际文件内容一致

#### Scenario: write 通过 sandbox proxy 写入后宿主机同步可见
- **WHEN** 当前 worker 使用 `bubblewrap` sandbox，且线程调用 `write`
- **THEN** 系统 SHALL 通过 JSON-RPC proxy 写入 sandbox workspace 内的目标文件
- **THEN** 宿主机的同步目录中 SHALL 可以直接看到写入结果

#### Scenario: edit 通过 sandbox proxy 完成读写闭环
- **WHEN** 当前 worker 使用 `bubblewrap` sandbox，且线程调用 `edit`
- **THEN** 系统 SHALL 通过 sandbox proxy 读取原文件内容并通过 sandbox proxy 写回更新结果
- **THEN** `edit` 的匹配计数、首个匹配替换和未找到目标文本错误语义 SHALL 保持不变

### Requirement: sandbox 开启时 command session SHALL 通过 JSON-RPC proxy 执行
当当前 worker 安装了非 `disabled` 的 sandbox 后，`exec_command` 以及与其绑定的 `write_stdin`、`list_unread_command_tasks` SHALL 通过 sandbox proxy 内的命令会话运行时执行，而 SHALL NOT 继续直接在宿主机创建命令进程。

#### Scenario: exec_command 在 sandbox 内启动后台命令会话
- **WHEN** 当前 worker 使用 `bubblewrap` sandbox，且线程调用 `exec_command`
- **THEN** 系统 SHALL 通过 JSON-RPC proxy 在 sandbox 内启动命令进程
- **THEN** 返回结果 SHALL 保持现有的 `session_id`、`running`、`output`、`wall_time_seconds` 等结构化语义

#### Scenario: write_stdin 续写 sandbox 内会话
- **WHEN** 当前线程持有一个由 sandbox `exec_command` 返回的 `session_id`
- **THEN** `write_stdin` SHALL 通过 JSON-RPC proxy 把输入写入同一个 sandbox 会话
- **THEN** 会话隔离与退出状态 SHALL 保持和当前 command session 运行时一致

#### Scenario: list_unread_command_tasks 只返回当前线程在 sandbox 内的未读会话
- **WHEN** 当前 worker 使用 `bubblewrap` sandbox，且线程调用 `list_unread_command_tasks`
- **THEN** 系统 SHALL 通过 JSON-RPC proxy 查询 sandbox 内 command session 状态
- **THEN** 返回列表 SHALL 继续只包含当前线程仍有未读输出的会话

### Requirement: sandbox 关闭时 core tool SHALL 保持当前宿主机语义
当当前 worker 使用 `disabled` sandbox 时，`read`、`write`、`edit`、`exec_command`、`write_stdin`、`list_unread_command_tasks` SHALL 继续保持当前宿主机执行语义，而 SHALL NOT 强制要求 JSON-RPC proxy 存在。

#### Scenario: disabled sandbox 不改变现有工具行为
- **WHEN** 当前 worker 的 sandbox backend 是 `disabled`
- **THEN** core 文件工具 SHALL 继续直接访问宿主文件系统
- **THEN** command session 工具 SHALL 继续直接使用宿主机命令运行时

### Requirement: sandbox 路由失败 SHALL 显式报错
当 sandbox 路径策略、proxy 协议或 sandbox 内命令执行失败时，系统 SHALL 把错误显式返回给对应工具调用，而 SHALL NOT 静默回退到宿主机直接执行。

### Requirement: sandbox 路径编码 SHALL preserve agent-visible path semantics
当宿主侧把工具路径转发到 sandbox proxy 时，系统 SHALL 保持 agent 在 sandbox 内观察到的路径语义可直接复用，而 SHALL NOT 因内部传输转换导致 `ls`/`pwd` 看到的路径无法继续被 `read`、`write`、`edit` 或 `exec_command.workdir` 使用。

#### Scenario: Agent reuses a path observed under /workspace
- **WHEN** agent 先在 sandbox 内观察到 `/workspace/demo.txt`
- **THEN** 后续 `read`、`write`、`edit` 或 `exec_command.workdir` 使用 `/workspace/demo.txt` 或 `/workspace` 时 SHALL 继续成功路由到同一工作区位置

#### Scenario: Agent reuses an explicit /tmp path
- **WHEN** agent 先在 sandbox 内观察到显式 `/tmp/demo.txt`
- **THEN** 后续工具调用 SHALL 继续把该路径路由到宿主机 `/tmp/demo.txt`，而不是改写成某个 workspace 相对路径

#### Scenario: sandbox 路径越界被显式拒绝
- **WHEN** sandbox 模式下的 `read`、`write`、`edit` 或 `exec_command.workdir` 请求访问同步目录之外的未授权路径
- **THEN** 工具调用 SHALL 返回显式失败
- **THEN** 系统 SHALL NOT 回退到宿主机直接访问该路径

#### Scenario: proxy 命令执行失败向上透传
- **WHEN** sandbox proxy 返回命令执行失败、会话不存在或协议错误
- **THEN** `exec_command`、`write_stdin` 或 `list_unread_command_tasks` SHALL 返回显式错误
- **THEN** 系统 SHALL NOT 改为在宿主机重新创建同名命令
