## ADDED Requirements

### Requirement: 系统 SHALL 在线程初始化时注入稳定的运行时环境感知 prompt
系统 SHALL 在每个线程初始化时生成一段稳定的运行时环境感知 prompt，并将其作为 `Thread.messages()` 开头 `System` 前缀的一部分写入线程。该 prompt SHALL 至少披露当前宿主的 OS family 与默认 shell/命令执行 shell 信息，供模型决定命令、路径和 shell 语法。

#### Scenario: Linux + zsh 环境在线程初始化时被注入
- **WHEN** 某个线程在 Linux 宿主上初始化，且当前默认 shell 为 `zsh`
- **THEN** 该线程的稳定前缀中会包含当前 OS 为 Linux、shell 为 `zsh` 的环境感知信息
- **THEN** 后续模型请求可以基于这段稳定环境事实生成对应命令和路径风格

### Requirement: 环境感知 SHALL 只暴露可验证事实，并对未知值显式标注
系统 SHALL 只把可可靠探测的环境事实写入环境感知 prompt。若某个关键字段无法确定，例如默认 shell 无法可靠探测，系统 SHALL 显式标注为 `unknown` 或等价未知值，而 SHALL NOT 猜测一个可能错误的环境。

#### Scenario: shell 无法探测时显式标注 unknown
- **WHEN** 某个线程初始化时系统无法可靠确定当前默认 shell
- **THEN** 写入线程前缀的环境感知 prompt 会把 shell 字段标注为 `unknown` 或等价未知值
- **THEN** 系统不会默认把该线程环境描述成 `bash`、`zsh` 或 `powershell`
