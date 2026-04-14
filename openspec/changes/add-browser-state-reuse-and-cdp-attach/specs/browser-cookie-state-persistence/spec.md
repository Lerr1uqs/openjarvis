## ADDED Requirements

### Requirement: 系统 SHALL 允许通过配置文件开启 browser cookies 自动复用
系统 SHALL 允许在配置文件中为 browser runtime 声明显式的 cookies 状态文件路径，以及“open 时自动注入”“close 时自动导出”两个独立 flag。该能力 SHALL 作用于 `browser__open(mode=launch)` 建立的 browser session，以及与其等价的默认 launch 初始化路径；系统 SHALL 在会话打开时优先从显式状态文件恢复 cookies，并在会话关闭时按配置把当前 session cookies 回写到同一路径，而 SHALL NOT 通过复用系统默认 Chrome Profile 隐式保留登录态。

#### Scenario: 配置开启后 browser open 自动注入 cookies
- **WHEN** 配置文件已经为 browser runtime 打开 cookies 自动注入 flag，并提供一个有效的状态文件路径
- **AND** 当前线程通过 `browser__open(mode=launch)` 或等价默认 launch 路径建立 browser session
- **THEN** 系统会在该 session 可供后续 `navigate / snapshot / click / type / screenshot` 使用前先注入状态文件中的 cookies

#### Scenario: 配置开启后 browser close 自动导出 cookies
- **WHEN** 配置文件已经为 browser runtime 打开 cookies 自动导出 flag，并提供一个状态文件路径
- **AND** 调用方在当前 browser session 中完成登录或刷新 cookies 后执行 `browser__close`
- **THEN** 系统会在释放当前 session 前把 cookies 导出到该显式状态文件
- **THEN** 返回给调用方的关闭结果可以明确这是一次已执行自动导出的 close

### Requirement: 自动注入缺少状态文件时 SHALL 不阻塞首次 launch
当配置文件开启 cookies 自动注入，但目标状态文件尚不存在时，系统 SHALL 继续创建当前 launch session，而 SHALL NOT 因为“第一次还没有 cookies 文件”直接让 `browser__open` 或默认 launch 初始化失败。首次会话后续若成功登录并执行 close，系统 SHALL 可以按自动导出配置写出该文件，供下一次 launch 复用。

#### Scenario: 首次 launch 时还没有 cookies 文件
- **WHEN** 调用方第一次使用已开启自动 cookies 复用的 launch 配置
- **AND** 配置中的 cookies 状态文件路径当前还不存在
- **THEN** 系统仍然成功建立一个空 cookies 基线的 launch session
- **THEN** 调用方后续可以在该 session 登录，并在 close 时得到自动导出的 cookies 文件

### Requirement: cookies 自动复用 SHALL NOT 通过新增常驻 browser 工具暴露
为了支持 cookies 自动复用，系统 SHALL NOT 在 `browser` toolset 的可见工具列表中额外暴露 `browser__export_cookies` 或 `browser__load_cookies` 这类常驻 agent 工具。cookies 复用 SHALL 体现为 browser open/close 生命周期上的配置驱动行为，以及显式的 Command / helper 导出入口，而不是新的默认浏览器动作工具。

#### Scenario: 加载 browser 后不会看到独立 cookies 工具
- **WHEN** 当前线程成功加载 `browser` toolset
- **THEN** 当前线程可见工具列表不会因为 cookies 自动复用而新增 `browser__export_cookies` 或 `browser__load_cookies`
- **THEN** cookies 复用行为由配置文件和 open/close 生命周期控制

### Requirement: 系统 SHALL 提供显式 Command / helper 用于手动导出当前 session cookies
除自动导出外，系统 SHALL 提供一个显式 Command 或等价 helper 入口，用于把当前线程正在使用的 browser session cookies 手动导出到调用方指定的状态文件。导出结果 SHALL 至少返回写入路径与导出的 cookies 数量；导出文件 SHALL 使用稳定的结构化格式，至少保留每条 cookie 的 `name`、`value`、`domain`、`path`、`expires`、`httpOnly`、`secure` 和 `sameSite` 信息，以便后续自动注入路径复用。显式导出的状态文件 SHALL NOT 因为 browser session 关闭而被系统自动删除。

#### Scenario: 用户通过 Command 手动导出 cookies 到指定文件
- **WHEN** 当前线程已经持有一个可用的 browser session，且用户显式触发 cookies 导出 Command / helper
- **THEN** 系统会把当前 session cookies 写入调用方指定的状态文件
- **THEN** 返回结果中包含导出文件路径和导出数量
- **THEN** 该状态文件在 session 关闭后仍然保留，可供后续 launch 自动注入
