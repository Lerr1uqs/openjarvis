## ADDED Requirements

### Requirement: Bubblewrap sandbox SHALL communicate through a JSON-RPC proxy
系统 SHALL 在宿主 agent 与 Bubblewrap 内 helper 之间建立基于 JSON-RPC 的桥接层，用于传递文件和进程操作请求。

#### Scenario: Host sends a JSON-RPC request to the sandbox proxy
- **WHEN** 宿主侧 Bubblewrap 后端发起一次沙箱操作
- **THEN** 系统 SHALL 通过结构化 JSON-RPC 请求把该操作发送给沙箱 proxy，并返回匹配 request id 的结果

#### Scenario: Proxy returns a structured error
- **WHEN** 沙箱内操作失败
- **THEN** proxy SHALL 返回结构化 JSON-RPC 错误对象，使宿主侧能够保留失败原因

### Requirement: Internal sandbox proxy SHALL be invocable as a hidden CLI helper
系统 SHALL 提供隐藏的内部命令入口来启动沙箱 proxy，以便 Bubblewrap 后端像现有 internal helpers 一样复用统一 bootstrap 方式。

#### Scenario: Bubblewrap backend launches the proxy helper
- **WHEN** Bubblewrap 后端初始化一个新沙箱会话
- **THEN** 系统 SHALL 启动隐藏的 `internal-sandbox` helper 命令作为 JSON-RPC proxy 入口

### Requirement: JSON-RPC file operations SHALL preserve host-visible workspace synchronization
系统 SHALL 保证通过 JSON-RPC 在沙箱同步工作区内执行的文件修改，会直接反映到宿主机工作区根目录上。

#### Scenario: Proxy writes a file inside the synchronized workspace
- **WHEN** 宿主侧通过 JSON-RPC 请求 proxy 写入 `demo.txt`
- **THEN** 沙箱写入完成后，宿主机 SHALL 能在工作区根目录下的 `demo.txt` 读取到同样内容

#### Scenario: Proxy writes a file inside /tmp
- **WHEN** 宿主侧通过 JSON-RPC 请求 proxy 写入显式 `/tmp/demo.txt`
- **THEN** 沙箱写入完成后，宿主机 SHALL 能在 `/tmp/demo.txt` 读取到同样内容

#### Scenario: Proxy request targets a disallowed path
- **WHEN** JSON-RPC 请求路径不在允许同步目录内、也不在 `/tmp` 内，或命中敏感目录限制
- **THEN** proxy SHALL 拒绝该请求并返回明确错误，而不是对宿主机其他路径做写入
