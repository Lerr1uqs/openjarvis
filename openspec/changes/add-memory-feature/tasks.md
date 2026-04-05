## 1. Memory Repository 与文档模型

- [x] 1.1 新增独立的本地 `MemoryRepository` 模块，定义 active/passive memory 文档、frontmatter metadata 与仓库扫描边界
- [x] 1.2 实现 `./.openjarvis/memory/active/**/*.md` 与 `./.openjarvis/memory/passive/**/*.md` 的 load / parse / write 流程，并补齐相对路径、`.md` 后缀与目录逃逸校验
- [x] 1.3 实现 active memory keyword catalog 派生逻辑，覆盖 `keywords` 必填、重复 keyword 拒绝与 `keyword -> relative path` 索引生成

## 2. Thread 初始化接线

- [x] 2.1 新增 active memory feature provider，在 thread 初始化或重初始化时从本地仓库构建 active memory catalog system prompt
- [x] 2.2 调整线程初始化接线，确保 active memory catalog 作为稳定 system snapshot 持久化进 `Thread`，而不是作为 request-time live memory 注入
- [x] 2.3 移除或收口现有面向 active memory 的自动 recall / live memory 注入路径，保持“舍弃主动注入、采用渐进式披露”的行为一致

## 3. Memory Toolset

- [x] 3.1 在 `agent/tool` 体系下新增可线程加载的 `memory` toolset，并注册 `memory_get`、`memory_search`、`memory_write`、`memory_list`
- [x] 3.2 为四个 memory 工具实现稳定 schema 与执行逻辑，覆盖 `type + path` 读取、默认 `passive` 写入、`active` 写入必须携带 `keywords` 等约束
- [x] 3.3 实现首版结构化 list / search 返回，确保正文只通过 `memory_get` 返回，避免 search/list 退化为隐式正文注入

## 4. 测试与文档

- [x] 4.1 在对应 `tests/` 目录下补齐 memory repository UT，覆盖 active/passive 文档解析、frontmatter 校验、重复 keyword 与非法路径场景
- [x] 4.2 补齐 thread 初始化与 AgentLoop 相关测试，验证 active memory catalog 只在初始化或重初始化时生效，且不会在请求期自动注入正文
- [x] 4.3 补齐 memory toolset UT，覆盖 toolset 加载、`memory_get/search/write/list` 正常路径与边界错误
- [x] 4.4 更新 `model/thread.md`、`model/agent/loop.md`、`README.md` 或相关文档，说明 memory 采用本地持久化与渐进式披露，而不是主动 recall
