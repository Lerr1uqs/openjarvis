## 1. 配置与知识库骨架

- [ ] 1.1 为 `obswiki` 增加独立运行时配置结构、路径解析与 preflight 校验。
- [ ] 1.2 创建默认 `./.openjarvis/obswiki/` vault 骨架和知识库内 `AGENTS.md` / `index.md` / `schema` 说明文件。
- [ ] 1.3 为 `obswiki` 配置解析与 vault preflight 增加单元测试与失败场景覆盖。

## 2. Obsidian 与检索运行时

- [ ] 2.1 实现 `obswiki` runtime，对 Obsidian CLI 做统一封装，并提供受控的读取、搜索、写回与 Raw 摄入接口。
- [ ] 2.2 实现 `obswiki_import_raw`、`obswiki_search`、`obswiki_read`、`obswiki_write`、`obswiki_update` 五个核心工具，并保证 `raw/` 不可改写。
- [ ] 2.3 接入 `QMD CLI 纯文本匹配优先 / Obsidian 搜索回退` 的检索策略，并实现 `index.md` 自动刷新逻辑。
- [ ] 2.4 为工具契约、Raw 不可变、QMD 纯文本匹配回退与索引自动更新补齐单元测试。

## 3. 子代理与独立调试入口

- [ ] 3.1 新增 `obswiki` thread agent profile、系统 prompt 与子代理目录描述。
- [ ] 3.2 在 `thread init` 中为 `obswiki` child thread 注入 vault 状态、`AGENTS.md` 正文和 `index.md` 链接索引。
- [ ] 3.3 新增独立于 main agent 的 `obswiki` 隐藏调试入口或脚本执行路径。
- [ ] 3.4 为 `obswiki` 子线程初始化、上下文注入与独立调试入口补齐测试。
