## MODIFIED Requirements

### Requirement: Bubblewrap sandbox SHALL communicate through a JSON-RPC control-plane proxy
系统 SHALL 在宿主 agent 与 Bubblewrap 内 helper 之间建立基于 JSON-RPC 的桥接层，并让该 helper 作为控制面 proxy 负责路径解析、请求校验、策略快照生成与 executor 调度，而 SHALL NOT 继续直接执行 workspace 文件读写或最终用户命令。

#### Scenario: Host sends a JSON-RPC request to the sandbox proxy
- **WHEN** 宿主侧 Bubblewrap 后端发起一次沙箱操作
- **THEN** 系统 SHALL 通过结构化 JSON-RPC 请求把该操作发送给沙箱 proxy
- **THEN** proxy SHALL 解析该请求、选择对应 executor 类型并调度后续执行

#### Scenario: Proxy returns a structured error
- **WHEN** proxy 在请求校验、策略快照生成、executor 启动或 executor 结果回传过程中失败
- **THEN** proxy SHALL 返回结构化 JSON-RPC 错误对象，使宿主侧能够保留失败原因

### Requirement: JSON-RPC file operations SHALL preserve host-visible workspace synchronization
系统 SHALL 保证通过 JSON-RPC 在沙箱同步工作区内发起的文件读取与修改，由受限 file executor 完成并直接反映到宿主机同步目录，而 SHALL NOT 由 proxy 自己直接碰 workspace 文件。

#### Scenario: Proxy routes a write request into a file executor
- **WHEN** 宿主侧通过 JSON-RPC 请求 proxy 写入 `demo.txt`
- **THEN** proxy SHALL 启动一个带策略快照的 file executor 执行该写入
- **THEN** 沙箱写入完成后，宿主机 SHALL 能在工作区根目录下的 `demo.txt` 读取到同样内容

#### Scenario: Proxy routes a file request into /tmp
- **WHEN** 宿主侧通过 JSON-RPC 请求 proxy 访问显式 `/tmp/demo.txt`
- **THEN** proxy SHALL 只在策略快照允许的情况下启动 file executor 处理该路径
- **THEN** 沙箱写入完成后，宿主机 SHALL 能在 `/tmp/demo.txt` 观察到同样结果

#### Scenario: Proxy request targets a disallowed path
- **WHEN** JSON-RPC 请求路径不在允许同步目录内、也不在 `/tmp` 内，或命中敏感目录限制
- **THEN** proxy SHALL 拒绝该请求并返回明确错误，而不是直接由 proxy 或某个宽权限 executor 对宿主机其他路径做读写
