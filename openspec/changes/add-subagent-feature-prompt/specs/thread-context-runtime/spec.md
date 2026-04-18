## MODIFIED Requirements

### Requirement: 系统 SHALL 在线程初始化时将稳定 `System` 前缀直接写入 `Thread`
系统 SHALL 在创建或恢复某个线程并准备初始化时，先解析该线程当前启用的 `Features` 与运行时环境感知结果，再把基础 system prompt、运行时环境感知 prompt，以及所有已启用 feature 的稳定 usage prompt 直接写入 `Thread.messages()` 的 `System` 前缀。对于主线程来说，若启用了 subagent feature，系统 SHALL 把基于当前可用 subagent catalog 构建的 subagent feature prompt 一并写入该前缀；对于 child thread，系统 SHALL NOT 写入这段父线程 subagent 管理说明。

#### Scenario: 主线程初始化时写入 subagent feature prompt
- **WHEN** 某个主线程启用了 subagent feature 并完成初始化
- **THEN** 该线程稳定 `System` 前缀中包含 subagent feature prompt
- **THEN** 这段 prompt 与基础 system prompt 一样属于该线程自己的持久化初始化前缀

#### Scenario: child thread 初始化时不写入父线程 subagent 管理说明
- **WHEN** 系统初始化一个 `browser` child thread
- **THEN** 该 child thread 的稳定 `System` 前缀只包含自己的 profile prompt 与已启用 feature prompt
- **THEN** 该 child thread 不会包含“当前有哪些 subagent 可调用”的父线程管理说明
