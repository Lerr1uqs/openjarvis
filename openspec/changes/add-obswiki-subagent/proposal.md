## Why

当前仓库已经具备 `subagent + toolset + thread init prompt` 这套基础设施，但还没有一个可插拔的本地 Wiki 知识库线程。用户希望把一个现成的 Obsidian vault 挂到 OpenJarvis 上，由专用 subagent 通过 Obsidian CLI 管理知识库，而不是让主线程直接读写散落的 markdown 文件，或者在运行时临时初始化仓库。

## What Changes

- 新增配置驱动的 `obswiki` 知识库绑定：程序启动或 `obswiki` subagent 初始化时读取配置中的 vault 路径；当路径不存在或 vault 缺少必需骨架时直接报错。
- 定义标准 vault 骨架与分层约束：`raw/`、`wiki/`、`schema/`、`index.md`、`AGENTS.md`。
- 新增 `obswiki` subagent profile 与独立调试入口，先支持直接验证该 subagent 与工具行为，再决定何时接入 main agent。
- 新增受控 `obswiki` 工具集，不提供 `init/status/log/sync_index` 这类外部管理工具；首版只定义 `obswiki_import_raw`、`obswiki_search`、`obswiki_read`、`obswiki_write`、`obswiki_update` 五个核心动作。vault 受管文档的读写、移动、搜索都必须走 Obsidian CLI 或其封装层，且每次变更后自动更新 `index.md`。
- 将 QMD 纳入本次 change，但首版只要求纯文本匹配检索，不要求 embedding；后续可在不改工具契约的前提下升级为 embedding 检索。
- 在线程初始化时为 `obswiki` child thread 注入 vault 约束、运行状态与知识库说明，而不是暴露单独的 `status` 工具。

## Capabilities

### New Capabilities
- `obswiki-subagent`: 定义配置驱动的 Obsidian vault、受控工具契约、Raw/Wiki/Schema 分层、QMD 纯文本检索接入，以及独立的 obswiki subagent 调试入口。

### Modified Capabilities
- `thread-context-runtime`: 为 `obswiki` child thread 增加稳定初始化上下文，要求在子线程启动时注入 vault `AGENTS.md`、`index.md` 链接索引与运行状态说明。

## Impact

- Affected code: `src/config.rs`、`src/thread/agent.rs`、`src/thread.rs`、`src/cli.rs`、`src/cli_command/internal.rs`、新增 `src/agent/tool/obswiki/**` 或等价模块，以及对应测试。
- Affected runtime data: 工作区默认会新增 `./.openjarvis/obswiki/` vault 骨架与知识库说明文件。
- API impact: 新增 `obswiki` 子代理画像与其内部工具契约；工具集合不对主线程默认平铺暴露。
- Dependency impact: 运行时要求本机可调用 Obsidian CLI；当配置了 QMD CLI 时，首版通过 QMD 执行纯文本匹配检索，不要求 embedding。
