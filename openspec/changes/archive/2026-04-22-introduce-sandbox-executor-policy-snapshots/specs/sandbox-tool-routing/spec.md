## MODIFIED Requirements

### Requirement: sandbox 开启时 core 文件工具 SHALL 通过 executor 执行
当当前 worker 安装了非 `disabled` 的 sandbox 后，`read`、`write`、`edit` SHALL 通过 proxy 调度的 file executor 完成文件读取与写回，而 SHALL NOT 继续由 proxy 自己直接操作 workspace 或宿主文件系统。

#### Scenario: read 通过 file executor 读取 workspace 文件
- **WHEN** 当前 worker 使用 `bubblewrap` sandbox，且线程调用 `read`
- **THEN** 系统 SHALL 通过 proxy 启动一个 file executor 读取目标文件内容
- **THEN** 返回内容 SHALL 与 sandbox 内实际文件内容一致

#### Scenario: write 通过 file executor 写入后宿主机同步可见
- **WHEN** 当前 worker 使用 `bubblewrap` sandbox，且线程调用 `write`
- **THEN** 系统 SHALL 通过 proxy 启动一个 file executor 写入 sandbox workspace 内的目标文件
- **THEN** 宿主机的同步目录中 SHALL 可以直接看到写入结果

#### Scenario: edit 通过 file executor 完成读写闭环
- **WHEN** 当前 worker 使用 `bubblewrap` sandbox，且线程调用 `edit`
- **THEN** 系统 SHALL 通过同一个 file executor 完成读取、首个精确匹配替换与写回
- **THEN** `edit` 的匹配计数、首个匹配替换和未找到目标文本错误语义 SHALL 保持不变

### Requirement: sandbox 开启时 command session SHALL 通过 executor 执行
当当前 worker 安装了非 `disabled` 的 sandbox 后，`exec_command` 以及与其绑定的 `write_stdin`、`list_unread_command_tasks` SHALL 通过 proxy 调度的 command executor 执行，而 SHALL NOT 继续由 proxy 直接持有最终用户命令进程。

#### Scenario: exec_command 通过 one-shot command executor 启动单次命令
- **WHEN** 当前 worker 使用 `bubblewrap` sandbox，且线程调用一次无需后续交互的 `exec_command`
- **THEN** 系统 SHALL 通过 proxy 启动一个 one-shot command executor
- **THEN** 返回结果 SHALL 保持现有的结构化命令结果语义

#### Scenario: exec_command 通过 session executor 启动后台或交互式命令
- **WHEN** 当前 worker 使用 `bubblewrap` sandbox，且线程调用需要后续续写或后台收集输出的 `exec_command`
- **THEN** 系统 SHALL 通过 proxy 启动一个 session executor
- **THEN** 后续 `write_stdin` 与 `list_unread_command_tasks` SHALL 路由到同一个 session executor

#### Scenario: write_stdin 续写 sandbox 内 session executor
- **WHEN** 当前线程持有一个由 sandbox `exec_command` 返回的 `session_id`
- **THEN** `write_stdin` SHALL 通过 proxy 把输入写入同一个 session executor
- **THEN** 会话隔离与退出状态 SHALL 保持和当前 command session 运行时一致

#### Scenario: list_unread_command_tasks 只返回当前线程在 sandbox 内的未读会话
- **WHEN** 当前 worker 使用 `bubblewrap` sandbox，且线程调用 `list_unread_command_tasks`
- **THEN** 系统 SHALL 通过 proxy 查询 sandbox 内 session executor 状态
- **THEN** 返回列表 SHALL 继续只包含当前线程仍有未读输出的会话
