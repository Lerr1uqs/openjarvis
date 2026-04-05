```python
• def web_run(
      *,
      open: list[dict] | None = None,
      click: list[dict] | None = None,
      find: list[dict] | None = None,
      screenshot: list[dict] | None = None,
      image_query: list[dict] | None = None,
      sports: list[dict] | None = None,
      finance: list[dict] | None = None,
      weather: list[dict] | None = None,
      time: list[dict] | None = None,
      response_length: str = "medium",
      search_query: list[dict] | None = None,
  ) -> object: ...

  def shell_command(
      *,
      command: str,
      justification: str | None = None,
      login: bool = True,
      prefix_rule: list[str] | None = None,
      sandbox_permissions: str | None = None,
      timeout_ms: int | None = None,
      workdir: str,
  ) -> object: ...

  def update_plan(
      *,
      explanation: str | None = None,
      plan: list[dict],
  ) -> object: ...

  def request_user_input(
      *,
      questions: list[dict],
  ) -> object: ...
  # 当前会话模式下不可用（Default mode）

  def view_image(
      *,
      path: str,
  ) -> object: ...

  def spawn_agent(
      *,
      agent_type: str | None = None,
      fork_context: bool | None = None,
      items: list[dict] | None = None,
      message: str | None = None,
      model: str | None = None,
      reasoning_effort: str | None = None,
  ) -> object: ...

  def send_input(
      *,
      target: str,
      interrupt: bool = False,
      items: list[dict] | None = None,
      message: str | None = None,
  ) -> object: ...

  def resume_agent(
      *,
      id: str,
  ) -> object: ...

  def wait_agent(
      *,
      targets: list[str],
      timeout_ms: int | None = None,
  ) -> object: ...

  def close_agent(
      *,
      target: str,
  ) -> object: ...

  def apply_patch(patch: str) -> object: ...

  def parallel(
      *,
      tool_uses: list[dict],
  ) -> object: ...
```

  - web_run(...)
    用来访问互联网和在线信息源。它本身是一个总入口，支持网页搜索、打开网页、点击链接、页面内查找文本、查看
    PDF 截图、图片搜索，以及查询体育赛程/排名、金融价格、天气、时区时间等。适合“查最新信息”“找官网文档”“验证
    事实”“看网页内容”。
  - shell_command(...)
    用来在当前工作区执行终端命令。适合查看代码、运行测试、编译项目、执行脚本、读取 git 状态、搜索文件内容等。
  - update_plan(...)
    用来维护任务计划。可以把当前任务拆成几个步骤，并标记每一步是 pending、in_progress 还是 completed。适合多
    步骤任务时同步进度。
  - request_user_input(...)
    用来向你发起结构化追问，让你在 1 到 3 个短问题里做选择。
    当前会话模式下这个工具不可用，所以这轮里基本不会用它。
  - view_image(...)
    用来看本地图片文件。适合你给我一个图片路径时，我直接读取并分析图片内容。
  - spawn_agent(...)
    用来启动一个子 agent，把某个独立子任务分出去处理。适合你明确要求“并行代理/子代理协作”时使用，比如拆分实
    现、并行调查不同模块。
  - send_input(...)
    用来给已经创建的子 agent 继续发送消息，补充说明、追加任务或中断后重定向它的工作。
  - resume_agent(...)
    用来恢复一个之前关闭或暂停的 agent，让它重新可接收任务。
  - wait_agent(...)
    用来等待一个或多个子 agent 完成任务。适合我需要拿到它们的结果后再继续主流程时使用。
  - close_agent(...)
    用来关闭不再需要的子 agent，避免一直占着上下文和资源。
  - apply_patch(patch: str)
    用来直接修改文件内容。它接收标准补丁格式，适合精确地新增、删除、修改代码，是我做手工代码编辑的主要工具。
  - parallel(...)
    用来并行调用多个“开发者工具”。适合多个互不依赖的读取类操作一起做，比如同时 ls、rg、git status、sed 几个文
    件，提高效率。