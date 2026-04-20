use async_trait::async_trait;
use chrono::Utc;
use openjarvis::{
    agent::{
        AgentRuntime, FeatureResolver, HookRegistry, MemoryType, MemoryWriteRequest, ShellEnv,
        SubagentRunner, ToolCallRequest, ToolCallResult, ToolDefinition, ToolHandler, ToolRegistry,
        ToolSource, ToolsetCatalogEntry, empty_tool_input_schema,
        tool::obswiki::{
            ObswikiRuntimeConfig, ObswikiVaultLayout, register_obswiki_toolset_with_config,
        },
    },
    config::{AppConfig, LLMConfig},
    llm::MockLLMProvider,
    model::{IncomingMessage, ReplyTarget},
    session::{MemorySessionStore, SessionManager, SessionStore},
    thread::{
        ChildThreadIdentity, DEFAULT_ASSISTANT_SYSTEM_PROMPT, Feature, Features, SubagentSpawnMode,
        Thread, ThreadAgent, ThreadAgentKind, ThreadContextLocator, ThreadRuntime,
        derive_child_thread_id,
    },
};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use uuid::Uuid;

struct BrowserEchoTool;

#[async_trait]
impl ToolHandler for BrowserEchoTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "browser__echo".to_string(),
            description: "Echo from browser toolset regression test".to_string(),
            input_schema: empty_tool_input_schema(),
            source: ToolSource::Builtin,
        }
    }

    async fn call(&self, _request: ToolCallRequest) -> anyhow::Result<ToolCallResult> {
        Ok(ToolCallResult {
            content: "browser-echo-ok".to_string(),
            metadata: json!({
                "event_kind": "browser_echo",
            }),
            is_error: false,
        })
    }
}

struct FeatureRuntimeFixture {
    root: PathBuf,
    skills_root: PathBuf,
}

impl FeatureRuntimeFixture {
    fn new(prefix: &str) -> Self {
        let root = std::env::temp_dir().join(format!("{prefix}-{}", Uuid::new_v4()));
        let skills_root = root.join(".openjarvis/skills");
        fs::create_dir_all(&skills_root).expect("feature runtime fixture should create skill root");
        Self { root, skills_root }
    }

    fn registry(&self) -> Arc<ToolRegistry> {
        Arc::new(ToolRegistry::with_workspace_root_and_skill_roots(
            &self.root,
            vec![self.skills_root.clone()],
        ))
    }

    fn write_skill(&self, skill_name: &str, description: &str) {
        let skill_dir = self.skills_root.join(skill_name);
        fs::create_dir_all(&skill_dir).expect("skill directory should exist");
        fs::write(
            skill_dir.join("SKILL.md"),
            format!(
                "---\nname: {skill_name}\ndescription: {description}\n---\nUse this skill when asked."
            ),
        )
        .expect("skill manifest should be written");
    }
}

impl Drop for FeatureRuntimeFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn compact_enabled_config() -> AppConfig {
    AppConfig::from_yaml_str(
        r#"
feishu:
  mode: "long_connection"
agent:
  compact:
    enabled: true
    auto_compact: true
llm:
  protocol: "mock"
  provider: "mock"
"#,
    )
    .expect("compact-enabled config should parse")
}

fn build_incoming(user_id: &str, external_thread_id: &str) -> IncomingMessage {
    IncomingMessage {
        id: Uuid::new_v4(),
        external_message_id: Some("msg_feature_runtime".to_string()),
        channel: "feishu".to_string(),
        user_id: user_id.to_string(),
        user_name: None,
        content: "hello".to_string(),
        external_thread_id: Some(external_thread_id.to_string()),
        received_at: Utc::now(),
        metadata: json!({}),
        attachments: Vec::new(),
        reply_target: ReplyTarget {
            receive_id: "oc_feature_runtime".to_string(),
            receive_id_type: "chat_id".to_string(),
        },
    }
}

fn build_thread_runtime(
    registry: Arc<ToolRegistry>,
    compact_config: openjarvis::config::AgentCompactConfig,
    feature_resolver: FeatureResolver,
) -> Arc<ThreadRuntime> {
    let memory_repository = registry.memory_repository();
    Arc::new(ThreadRuntime::with_feature_resolver(
        registry,
        memory_repository,
        compact_config,
        feature_resolver,
    ))
}

async fn install_mock_subagent_runner(
    registry: &Arc<ToolRegistry>,
    compact_config: openjarvis::config::AgentCompactConfig,
) -> Arc<SubagentRunner> {
    let runtime = AgentRuntime::with_parts(Arc::new(HookRegistry::new()), Arc::clone(registry));
    let runner = Arc::new(SubagentRunner::new(
        Arc::new(MockLLMProvider::new("feature-runtime-subagent")),
        runtime,
        LLMConfig::default(),
        compact_config,
    ));
    registry.install_subagent_runner(&runner);
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register after subagent runner install");
    runner
}

fn write_mock_obswiki_cli(root: &Path) -> PathBuf {
    let path = root.join("mock-obswiki-cli.py");
    let running_flag = root.join("obsidian-running.flag");
    let script = r##"#!/usr/bin/env python3
import json
import sys
from pathlib import Path

cwd = Path.cwd()
running_flag = Path(r"__RUNNING_FLAG__")

def parse(args):
    data = {}
    flags = set()
    for arg in args:
        if "=" in arg:
            key, value = arg.split("=", 1)
            data[key] = value
        else:
            flags.add(arg)
    return data, flags

if len(sys.argv) < 2:
    sys.exit(0)

command = sys.argv[1]
data, flags = parse(sys.argv[2:])

if command in {"help", "--help"}:
    sys.exit(0)

if not running_flag.exists():
    print("obsidian app not started", file=sys.stderr)
    sys.exit(7)

if command == "create":
    target = cwd / data["path"]
    target.parent.mkdir(parents=True, exist_ok=True)
    if target.exists() and "overwrite" not in flags:
        print("exists", file=sys.stderr)
        sys.exit(2)
    target.write_text(data.get("content", ""), encoding="utf-8")
    print(data["path"])
    sys.exit(0)

if command == "read":
    target = cwd / data["path"]
    if not target.exists():
        print("missing", file=sys.stderr)
        sys.exit(4)
    print(target.read_text(encoding="utf-8"), end="")
    sys.exit(0)

if command == "files":
    folder = data.get("folder", "")
    extension = data.get("ext", "md")
    root = cwd / folder
    results = []
    if root.exists():
        for file in sorted(root.rglob(f"*.{extension}")):
            results.append(file.relative_to(cwd).as_posix())
    print("\n".join(results))
    sys.exit(0)

if command == "search":
    query = data.get("query", "").lower()
    limit = int(data.get("limit", "10"))
    scope = data.get("path")
    search_root = cwd / scope if scope else cwd
    results = []
    if search_root.exists():
        for file in sorted(search_root.rglob("*.md")):
            rel = file.relative_to(cwd).as_posix()
            text = file.read_text(encoding="utf-8")
            if query in rel.lower() or query in text.lower():
                results.append(rel)
                if len(results) >= limit:
                    break
    if data.get("format") == "json":
        print(json.dumps(results))
    else:
        print("\n".join(results))
    sys.exit(0)

print(f"unsupported command: {command}", file=sys.stderr)
sys.exit(9)
"##
    .replace("__RUNNING_FLAG__", &running_flag.display().to_string());
    fs::write(&path, script).expect("mock obswiki cli should be written");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let metadata = fs::metadata(&path).expect("mock obswiki cli metadata should exist");
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("mock obswiki cli permissions should apply");
    }
    path
}

fn mark_obsidian_running(root: &Path) {
    fs::write(root.join("obsidian-running.flag"), "running")
        .expect("obsidian running flag should be written");
}

fn build_obswiki_runtime_config(vault_root: &Path, obsidian_bin: &Path) -> ObswikiRuntimeConfig {
    ObswikiVaultLayout::new(vault_root)
        .ensure_default_skeleton()
        .expect("obswiki feature runtime vault skeleton should exist");
    let config = AppConfig::from_yaml_str(&format!(
        r#"
agent:
  tool:
    obswiki:
      enabled: true
      vault_path: "{}"
      obsidian_bin: "{}"
llm:
  protocol: "mock"
  provider: "mock"
"#,
        vault_root.display(),
        obsidian_bin.display(),
    ))
    .expect("obswiki runtime config should parse");
    ObswikiRuntimeConfig::from_agent_config(config.agent_config().tool_config().obswiki_config())
        .expect("enabled obswiki config should resolve")
}

#[test]
fn feishu_user_feature_override_is_parsed_and_resolved() {
    // 测试场景: channel + user 显式配置的 feature 集合应被 resolver 直接命中；未配置用户走默认全开。
    let config = AppConfig::from_yaml_str(
        r#"
feishu:
  users:
    ou_explicit:
      features: [memory, skill]
agent:
  compact:
    enabled: true
    auto_compact: true
llm:
  protocol: "mock"
  provider: "mock"
"#,
    )
    .expect("feature override config should parse");

    let explicit = config
        .channel_config()
        .feishu_config()
        .user_features("ou_explicit")
        .expect("explicit user features should exist");
    assert_eq!(explicit.names(), vec!["memory", "skill"]);

    let resolver = FeatureResolver::from_app_config(&config, Features::all());
    assert_eq!(
        resolver.resolve("feishu", "ou_explicit").names(),
        vec!["memory", "skill"]
    );
    assert_eq!(
        resolver.resolve("feishu", "ou_missing").names(),
        vec!["memory", "skill", "subagent", "auto_compact"]
    );
}

#[test]
fn shell_env_reports_detected_facts() {
    // 测试场景: 运行时环境感知 prompt 至少要暴露 OS family、shell 和命令执行 shell 事实。
    let shell_env = ShellEnv::detect();
    let prompt = shell_env.render_prompt();

    assert!(prompt.contains("os_family:"));
    assert!(prompt.contains("default_shell:"));
    assert!(prompt.contains("command_execution_shell:"));
    assert!(prompt.contains("path_style:"));
}

#[tokio::test]
async fn thread_runtime_initializes_ordered_feature_prefix() {
    // 测试场景: 初始化顺序应为基础角色 -> 环境感知 -> feature prompts，
    // 同时 memory toolset 要在首轮请求前预加载。
    let fixture = FeatureRuntimeFixture::new("openjarvis-feature-runtime-plan");
    fixture.write_skill("demo_skill", "demo skill from fixture");

    let registry = fixture.registry();
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");
    registry
        .memory_repository()
        .write(MemoryWriteRequest {
            memory_type: MemoryType::Active,
            path: "workflow/notion.md".to_string(),
            title: "Notion workflow".to_string(),
            content: "Use the explicit user template.".to_string(),
            keywords: Some(vec!["notion".to_string(), "workflow".to_string()]),
        })
        .expect("active memory fixture should be written");

    let compact_config = compact_enabled_config()
        .agent_config()
        .compact_config()
        .clone();
    let resolver = FeatureResolver::development_default(Features::from_iter([
        Feature::Memory,
        Feature::Skill,
        Feature::AutoCompact,
    ]));
    let runtime = ThreadRuntime::with_feature_resolver(
        Arc::clone(&registry),
        registry.memory_repository(),
        compact_config,
        resolver,
    );
    let mut thread = Thread::new(
        ThreadContextLocator::new(None, "feishu", "ou_demo", "thread_plan", "thread_plan"),
        Utc::now(),
    );
    runtime
        .initialize_thread(&mut thread, ThreadAgentKind::Main)
        .await
        .expect("thread should initialize");

    assert_eq!(
        thread.enabled_features().names(),
        vec!["memory", "skill", "auto_compact"]
    );
    assert_eq!(thread.load_toolsets(), vec!["memory".to_string()]);

    let messages = thread.messages();
    let environment_index = messages
        .iter()
        .position(|message| {
            message
                .content
                .contains("Runtime environment for this thread")
        })
        .expect("environment perception prompt should exist");
    let memory_index = messages
        .iter()
        .position(|message| {
            message
                .content
                .contains("notion, workflow -> workflow/notion.md")
        })
        .expect("memory prompt should exist");
    let skill_index = messages
        .iter()
        .position(|message| message.content.contains("Available local skills"))
        .expect("skill prompt should exist");
    let auto_compact_index = messages
        .iter()
        .position(|message| message.content.contains("Auto-compact 已开启"))
        .expect("auto-compact prompt should exist");

    assert_eq!(messages[0].content, DEFAULT_ASSISTANT_SYSTEM_PROMPT.trim());
    assert!(environment_index < memory_index);
    assert!(environment_index < skill_index);
    assert!(environment_index < auto_compact_index);
    assert!(messages.iter().any(|message| {
        message
            .content
            .contains("Currently loaded toolsets for this thread: memory")
    }));
}

#[tokio::test]
async fn thread_runtime_persists_features_and_preserves_them_across_restore() {
    // 测试场景: 新线程初始化后要把 enabled_features 与预加载 toolset 持久化；
    // 线程恢复时必须继续使用持久化 truth，而不是被新的默认 resolver 覆盖。
    let fixture = FeatureRuntimeFixture::new("openjarvis-feature-runtime-session");
    let registry = fixture.registry();
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");
    registry
        .memory_repository()
        .write(MemoryWriteRequest {
            memory_type: MemoryType::Active,
            path: "users/demo.md".to_string(),
            title: "Demo memory".to_string(),
            content: "memory body".to_string(),
            keywords: Some(vec!["demo".to_string()]),
        })
        .expect("active memory fixture should be written");

    let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
    let manager_a = SessionManager::with_store(Arc::clone(&store))
        .await
        .expect("session manager a should build");
    manager_a.install_thread_runtime(build_thread_runtime(
        Arc::clone(&registry),
        AppConfig::default().agent_config().compact_config().clone(),
        FeatureResolver::development_default(Features::from_iter([Feature::Memory])),
    ));

    let incoming = build_incoming("ou_feature_runtime", "chat_feature_runtime");
    let locator = manager_a
        .create_thread(&incoming, ThreadAgentKind::Main)
        .await
        .expect("thread should resolve");
    let initialized = manager_a
        .load_thread(&locator)
        .await
        .expect("thread should load")
        .expect("thread should exist");

    assert_eq!(initialized.enabled_features().names(), vec!["memory"]);
    assert_eq!(initialized.load_toolsets(), vec!["memory".to_string()]);

    let runtime = AgentRuntime::with_parts(Arc::new(HookRegistry::new()), Arc::clone(&registry));
    let visible_tools = runtime
        .list_tools(&initialized, false)
        .await
        .expect("visible tools should render");
    assert!(visible_tools.iter().any(|tool| tool.name == "memory_get"));

    {
        let mut locked_thread = manager_a
            .lock_thread(&locator, incoming.received_at)
            .await
            .expect("live thread lock result should resolve")
            .expect("live thread should lock");
        assert!(
            locked_thread
                .unload_toolset("memory")
                .await
                .expect("memory toolset should unload")
        );
        assert!(
            locked_thread
                .load_toolset("memory")
                .await
                .expect("memory toolset should load again")
        );
    }

    let manager_b = SessionManager::with_store(store)
        .await
        .expect("session manager b should build");
    manager_b.install_thread_runtime(build_thread_runtime(
        Arc::clone(&registry),
        compact_enabled_config()
            .agent_config()
            .compact_config()
            .clone(),
        FeatureResolver::development_default(Features::from_iter([Feature::Skill])),
    ));
    let restored = manager_b
        .load_thread(&locator)
        .await
        .expect("restored thread should load")
        .expect("restored thread should exist");

    assert_eq!(restored.enabled_features().names(), vec!["memory"]);
    assert_eq!(restored.load_toolsets(), vec!["memory".to_string()]);
    assert!(
        !restored.enabled_features().contains(Feature::Skill),
        "恢复后的线程不能被新的默认 resolver 覆盖成 skill"
    );
}

#[tokio::test]
async fn browser_thread_agent_preloads_browser_toolset_and_prompt() {
    // 测试场景: Browser kind 的 capability profile 是线程能力真相；
    // 即使 resolver、skill、subagent runtime 和 compact 都已开启，也只能看到浏览器 prompt 与浏览器工具。
    let fixture = FeatureRuntimeFixture::new("openjarvis-feature-runtime-browser");
    fixture.write_skill("browser_demo_skill", "browser-only fixture skill");
    let registry = fixture.registry();
    registry
        .memory_repository()
        .write(MemoryWriteRequest {
            memory_type: MemoryType::Active,
            path: "browser/runtime.md".to_string(),
            title: "Browser runtime".to_string(),
            content: "browser runtime memory".to_string(),
            keywords: Some(vec!["browser".to_string(), "runtime".to_string()]),
        })
        .expect("browser memory fixture should be written");

    let compact_config = compact_enabled_config()
        .agent_config()
        .compact_config()
        .clone();
    let _runner = install_mock_subagent_runner(&registry, compact_config.clone()).await;

    let runtime = ThreadRuntime::with_feature_resolver(
        Arc::clone(&registry),
        registry.memory_repository(),
        compact_config,
        FeatureResolver::development_default(Features::all()),
    );
    let mut thread = Thread::new(
        ThreadContextLocator::new(
            None,
            "feishu",
            "ou_browser",
            "thread_browser",
            "thread_browser",
        ),
        Utc::now(),
    );
    runtime
        .initialize_thread(&mut thread, ThreadAgentKind::Browser)
        .await
        .expect("browser thread should initialize");

    let expected_browser_prompt = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/prompts/thread_agent/browser.md"),
    )
    .expect("browser prompt markdown should load");

    assert_eq!(thread.thread_agent_kind(), ThreadAgentKind::Browser);
    assert!(thread.enabled_features().is_empty());
    assert_eq!(thread.load_toolsets(), vec!["browser".to_string()]);
    assert_eq!(thread.messages()[0].content, expected_browser_prompt.trim());
    assert!(thread.messages().iter().all(|message| {
        !message
            .content
            .contains("browser, runtime -> browser/runtime.md")
            && !message.content.contains("Available local skills")
            && !message.content.contains("当前可用 subagent 数量:")
            && !message.content.contains("Auto-compact 已开启")
            && !message
                .content
                .contains("当 `compact` 工具未出现在当前工具列表中时")
    }));

    let visible_tools = registry
        .list_for_context_with_compact(&thread, true)
        .await
        .expect("browser thread tools should list");
    let visible_tool_names = visible_tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    assert!(
        visible_tool_names
            .iter()
            .all(|tool_name| tool_name.starts_with("browser__"))
    );
    assert!(visible_tool_names.contains(&"browser__navigate"));
    assert!(visible_tool_names.contains(&"browser__snapshot"));
    assert!(!visible_tool_names.contains(&"load_toolset"));
    assert!(!visible_tool_names.contains(&"unload_toolset"));
    assert!(!visible_tool_names.contains(&"load_skill"));
    assert!(!visible_tool_names.contains(&"spawn_subagent"));
    assert!(!visible_tool_names.contains(&"send_subagent"));
    assert!(!visible_tool_names.contains(&"memory_get"));
    assert!(registry.catalog_prompt_for_context(&thread).await.is_none());
}

#[tokio::test]
async fn browser_thread_reconcile_filters_disallowed_feature_and_toolset_state() {
    // 测试场景: 已初始化的 Browser 线程即使带着脏的 feature/toolset 快照再次进入 runtime，
    // 也必须被当前 kind profile 收敛回 browser-only 能力边界。
    let fixture = FeatureRuntimeFixture::new("openjarvis-feature-runtime-browser-reconcile");
    let registry = fixture.registry();
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");

    let runtime = ThreadRuntime::with_feature_resolver(
        Arc::clone(&registry),
        registry.memory_repository(),
        compact_enabled_config()
            .agent_config()
            .compact_config()
            .clone(),
        FeatureResolver::development_default(Features::all()),
    );
    let mut thread = Thread::new(
        ThreadContextLocator::new(
            None,
            "feishu",
            "ou_browser_reconcile",
            "thread_browser_reconcile",
            "thread_browser_reconcile",
        ),
        Utc::now(),
    );
    runtime
        .initialize_thread(&mut thread, ThreadAgentKind::Browser)
        .await
        .expect("browser thread should initialize");

    thread.replace_thread_agent(ThreadAgent::new(
        ThreadAgentKind::Browser,
        vec!["browser".to_string(), "memory".to_string()],
    ));
    thread.replace_enabled_features(Features::from_iter([
        Feature::Memory,
        Feature::Skill,
        Feature::Subagent,
        Feature::AutoCompact,
    ]));
    thread.replace_loaded_toolsets(vec!["browser".to_string(), "memory".to_string()]);

    runtime
        .initialize_thread(&mut thread, ThreadAgentKind::Main)
        .await
        .expect("browser thread reconcile should succeed");

    assert_eq!(thread.thread_agent_kind(), ThreadAgentKind::Browser);
    assert_eq!(
        thread.thread_agent().bound_toolsets,
        vec!["browser".to_string()]
    );
    assert!(thread.enabled_features().is_empty());
    assert_eq!(thread.load_toolsets(), vec!["browser".to_string()]);

    let visible_tools = registry
        .list_for_context_with_compact(&thread, true)
        .await
        .expect("reconciled browser thread tools should list");
    let visible_tool_names = visible_tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    assert!(
        visible_tool_names
            .iter()
            .all(|tool_name| tool_name.starts_with("browser__"))
    );
    assert!(!visible_tool_names.contains(&"memory_get"));
    assert!(!visible_tool_names.contains(&"load_toolset"));
    assert!(registry.catalog_prompt_for_context(&thread).await.is_none());
}

#[tokio::test]
async fn browser_thread_can_call_bound_browser_toolset_tools() {
    // 测试场景: Browser kind 绑定的 browser toolset 工具必须可调用，
    // 不能被 always-visible tool 的 enabled 判定误挡住。
    let registry = ToolRegistry::new();
    registry
        .register_toolset(
            ToolsetCatalogEntry::new("browser", "Browser regression test toolset"),
            vec![Arc::new(BrowserEchoTool)],
        )
        .await
        .expect("browser regression toolset should register");

    let mut thread = Thread::new(
        ThreadContextLocator::new(
            None,
            "feishu",
            "ou_browser_call",
            "thread_browser_call",
            "thread_browser_call",
        ),
        Utc::now(),
    );
    thread.replace_thread_agent(ThreadAgent::from_kind(ThreadAgentKind::Browser));
    thread.replace_loaded_toolsets(vec!["browser".to_string()]);

    let result = registry
        .call_for_context(
            &mut thread,
            ToolCallRequest {
                name: "browser__echo".to_string(),
                arguments: json!({}),
            },
        )
        .await
        .expect("browser bound toolset tool should execute");
    assert_eq!(result.content, "browser-echo-ok");
}

#[tokio::test]
async fn subagent_feature_prompt_is_only_injected_for_main_threads() {
    // 测试场景: 启用 subagent feature 后，主线程要看到基于当前 catalog 的稳定 prompt；
    // child thread 不继承这段父线程管理说明，也不暴露 subagent 管理工具。
    let fixture = FeatureRuntimeFixture::new("openjarvis-feature-runtime-subagent");
    let registry = fixture.registry();
    let compact_config = AppConfig::default().agent_config().compact_config().clone();
    let _runner = install_mock_subagent_runner(&registry, compact_config.clone()).await;
    let runtime = ThreadRuntime::with_feature_resolver(
        Arc::clone(&registry),
        registry.memory_repository(),
        compact_config,
        FeatureResolver::development_default(Features::from_iter([Feature::Subagent])),
    );

    let mut main_thread = Thread::new(
        ThreadContextLocator::new(
            None,
            "feishu",
            "ou_subagent_feature",
            "thread_subagent_main",
            "thread_subagent_main",
        ),
        Utc::now(),
    );
    runtime
        .initialize_thread(&mut main_thread, ThreadAgentKind::Main)
        .await
        .expect("main thread should initialize");

    let main_messages = main_thread.messages();
    let subagent_prompt = main_messages
        .iter()
        .find(|message| message.content.contains("当前可用 subagent 数量:"))
        .expect("main thread should receive subagent feature prompt");
    assert!(
        subagent_prompt
            .content
            .contains("当前可用 subagent 数量: 1")
    );
    assert!(subagent_prompt.content.contains("subagent_key: browser"));
    assert!(subagent_prompt.content.contains("when_to_use:"));
    assert!(
        subagent_prompt
            .content
            .contains("简单直接的工具调用不应默认升级成 subagent 调用")
    );
    assert!(main_thread.enabled_features().contains(Feature::Subagent));

    let main_tools = registry
        .list_for_context(&main_thread)
        .await
        .expect("main thread tools should list");
    assert!(main_tools.iter().any(|tool| tool.name == "spawn_subagent"));
    assert!(main_tools.iter().any(|tool| tool.name == "send_subagent"));

    let mut child_thread = Thread::new(
        ThreadContextLocator::new(
            None,
            "feishu",
            "ou_subagent_feature",
            "thread_subagent_main",
            derive_child_thread_id("thread_subagent_main", "browser").to_string(),
        )
        .with_child_thread(ChildThreadIdentity::new(
            "thread_subagent_main",
            "browser",
            SubagentSpawnMode::Persist,
        )),
        Utc::now(),
    );
    runtime
        .initialize_thread(&mut child_thread, ThreadAgentKind::Browser)
        .await
        .expect("child thread should initialize");

    assert!(
        !child_thread.enabled_features().contains(Feature::Subagent),
        "child thread must not inherit parent-only subagent feature"
    );
    assert!(child_thread.messages().iter().all(|message| {
        !message.content.contains("当前可用 subagent 数量:")
            && !message
                .content
                .contains("简单直接的工具调用不应默认升级成 subagent 调用")
    }));
    let child_tools = registry
        .list_for_context(&child_thread)
        .await
        .expect("child thread tools should list");
    assert!(!child_tools.iter().any(|tool| tool.name == "spawn_subagent"));
    assert!(!child_tools.iter().any(|tool| tool.name == "send_subagent"));
}

#[tokio::test]
async fn subagent_feature_prompt_lists_obswiki_when_toolset_is_registered() {
    // 测试场景: obswiki toolset 已注册时，主线程 subagent catalog 必须把 obswiki profile 暴露出来。
    let fixture = FeatureRuntimeFixture::new("openjarvis-feature-runtime-obswiki-catalog");
    let registry = fixture.registry();
    let compact_config = AppConfig::default().agent_config().compact_config().clone();
    let _runner = install_mock_subagent_runner(&registry, compact_config.clone()).await;
    let vault_root = fixture.root.join("vault");
    mark_obsidian_running(&fixture.root);
    let obsidian_bin = write_mock_obswiki_cli(&fixture.root);
    register_obswiki_toolset_with_config(
        &registry,
        build_obswiki_runtime_config(&vault_root, &obsidian_bin),
    )
    .await
    .expect("obswiki toolset should register");
    let runtime = ThreadRuntime::with_feature_resolver(
        Arc::clone(&registry),
        registry.memory_repository(),
        compact_config,
        FeatureResolver::development_default(Features::from_iter([Feature::Subagent])),
    );

    let mut main_thread = Thread::new(
        ThreadContextLocator::new(
            None,
            "feishu",
            "ou_obswiki_catalog",
            "thread_obswiki_catalog",
            "thread_obswiki_catalog",
        ),
        Utc::now(),
    );
    runtime
        .initialize_thread(&mut main_thread, ThreadAgentKind::Main)
        .await
        .expect("main thread should initialize");

    let main_messages = main_thread.messages();
    let subagent_prompt = main_messages
        .iter()
        .find(|message| message.content.contains("当前可用 subagent 数量:"))
        .expect("main thread should receive subagent feature prompt");
    assert!(
        subagent_prompt
            .content
            .contains("当前可用 subagent 数量: 2")
    );
    assert!(subagent_prompt.content.contains("subagent_key: browser"));
    assert!(subagent_prompt.content.contains("subagent_key: obswiki"));
    assert!(subagent_prompt.content.contains("persist 模式"));
}

#[tokio::test]
async fn obswiki_child_thread_initializes_with_vault_context_and_bound_tools() {
    // 测试场景: obswiki child thread 初始化时必须注入 vault 状态/AGENTS/index，
    // 只开启 skill feature，不继承 memory，并且内置工具只暴露 exec_command / load_skill。
    let fixture = FeatureRuntimeFixture::new("openjarvis-feature-runtime-obswiki-child");
    fixture.write_skill("obswiki_demo_skill", "obswiki-only fixture skill");
    let registry = fixture.registry();
    let vault_root = fixture.root.join("vault");
    mark_obsidian_running(&fixture.root);
    let obsidian_bin = write_mock_obswiki_cli(&fixture.root);
    register_obswiki_toolset_with_config(
        &registry,
        build_obswiki_runtime_config(&vault_root, &obsidian_bin),
    )
    .await
    .expect("obswiki toolset should register");
    let compact_config = AppConfig::default().agent_config().compact_config().clone();
    let runtime = ThreadRuntime::with_feature_resolver(
        Arc::clone(&registry),
        registry.memory_repository(),
        compact_config,
        FeatureResolver::development_default(Features::default()),
    );

    let mut seed_thread = Thread::new(
        ThreadContextLocator::new(
            None,
            "feishu",
            "ou_obswiki_seed",
            "thread_obswiki_seed",
            "thread_obswiki_seed",
        ),
        Utc::now(),
    );
    seed_thread.replace_loaded_toolsets(vec!["obswiki".to_string()]);
    registry
        .call_for_context(
            &mut seed_thread,
            ToolCallRequest {
                name: "obswiki_write".to_string(),
                arguments: json!({
                    "path": "wiki/guide.md",
                    "title": "Guide",
                    "content": "# Guide\n\nKnowledge entry",
                    "page_type": "guide",
                }),
            },
        )
        .await
        .expect("seed wiki page should be created");

    let mut child_thread = Thread::new(
        ThreadContextLocator::new(
            None,
            "feishu",
            "ou_obswiki_child",
            "thread_obswiki_child",
            derive_child_thread_id("thread_obswiki_child", "obswiki").to_string(),
        )
        .with_child_thread(ChildThreadIdentity::new(
            "thread_obswiki_child",
            "obswiki",
            SubagentSpawnMode::Persist,
        )),
        Utc::now(),
    );
    runtime
        .initialize_thread(&mut child_thread, ThreadAgentKind::Obswiki)
        .await
        .expect("obswiki child thread should initialize");

    assert_eq!(child_thread.thread_agent_kind(), ThreadAgentKind::Obswiki);
    assert_eq!(child_thread.enabled_features().names(), vec!["skill"]);
    assert_eq!(child_thread.load_toolsets(), vec!["obswiki".to_string()]);
    assert!(
        child_thread
            .messages()
            .iter()
            .any(|message| message.content.contains("Obswiki vault runtime status:"))
    );
    assert!(child_thread.messages().iter().any(|message| {
        message
            .content
            .contains("Obswiki vault `AGENTS.md` 正文如下:")
    }));
    assert!(
        child_thread
            .messages()
            .iter()
            .any(|message| message.content.contains("[wiki/guide.md|"))
    );
    assert!(child_thread.messages().iter().any(|message| {
        message.content.contains("Available local skills")
            && message.content.contains("obswiki_demo_skill")
    }));
    assert!(child_thread.messages().iter().all(|message| {
        !message.content.contains("memory feature")
            && !message.content.contains("Memory toolset")
            && !message.content.contains("当前可用 subagent 数量:")
    }));

    let visible_tools = registry
        .list_for_context(&child_thread)
        .await
        .expect("obswiki child tools should list");
    assert!(
        visible_tools
            .iter()
            .any(|tool| tool.name == "obswiki_search")
    );
    assert!(visible_tools.iter().any(|tool| tool.name == "obswiki_read"));
    assert!(
        visible_tools
            .iter()
            .any(|tool| tool.name == "obswiki_write")
    );
    assert!(
        visible_tools
            .iter()
            .any(|tool| tool.name == "obswiki_update")
    );
    assert!(visible_tools.iter().any(|tool| tool.name == "exec_command"));
    assert!(visible_tools.iter().any(|tool| tool.name == "load_skill"));
    assert!(
        !visible_tools
            .iter()
            .any(|tool| tool.name == "spawn_subagent")
    );
    assert!(!visible_tools.iter().any(|tool| tool.name == "memory_get"));
    assert!(!visible_tools.iter().any(|tool| tool.name == "write_stdin"));
    assert!(
        !visible_tools
            .iter()
            .any(|tool| tool.name == "list_unread_command_tasks")
    );

    let error = registry
        .call_for_context(
            &mut child_thread,
            ToolCallRequest {
                name: "write_stdin".to_string(),
                arguments: json!({
                    "session_id": "blocked",
                    "chars": "",
                }),
            },
        )
        .await
        .expect_err("obswiki child thread should reject disallowed builtin command session tools");
    assert!(format!("{error:#}").contains("tool `write_stdin` is not enabled"));
}

#[tokio::test]
async fn subagent_feature_disabled_hides_prompt_and_tools_on_main_thread() {
    // 测试场景: feature 关闭时，即使 runtime 注册了 subagent 工具，主线程也不能看到 subagent prompt 或管理工具。
    let fixture = FeatureRuntimeFixture::new("openjarvis-feature-runtime-subagent-off");
    let registry = fixture.registry();
    let compact_config = AppConfig::default().agent_config().compact_config().clone();
    let _runner = install_mock_subagent_runner(&registry, compact_config.clone()).await;
    let runtime = ThreadRuntime::with_feature_resolver(
        Arc::clone(&registry),
        registry.memory_repository(),
        compact_config,
        FeatureResolver::development_default(Features::default()),
    );
    let mut thread = Thread::new(
        ThreadContextLocator::new(
            None,
            "feishu",
            "ou_subagent_feature_off",
            "thread_subagent_off",
            "thread_subagent_off",
        ),
        Utc::now(),
    );
    runtime
        .initialize_thread(&mut thread, ThreadAgentKind::Main)
        .await
        .expect("main thread should initialize");

    assert!(
        thread
            .messages()
            .iter()
            .all(|message| !message.content.contains("当前可用 subagent 数量:")),
        "main thread should not receive subagent feature prompt when feature is disabled"
    );
    let visible_tools = registry
        .list_for_context(&thread)
        .await
        .expect("visible tools should list");
    assert!(
        !visible_tools
            .iter()
            .any(|tool| tool.name == "spawn_subagent")
    );
    assert!(
        !visible_tools
            .iter()
            .any(|tool| tool.name == "send_subagent")
    );
    assert!(
        !visible_tools
            .iter()
            .any(|tool| tool.name == "close_subagent")
    );
    assert!(
        !visible_tools
            .iter()
            .any(|tool| tool.name == "list_subagent")
    );
}

#[tokio::test]
async fn main_thread_agent_uses_bundled_markdown_prompt() {
    // 测试场景: Main thread 的稳定 system prompt 只来自随程序打包的 markdown 模板。
    let fixture = FeatureRuntimeFixture::new("openjarvis-feature-runtime-main-md");
    let registry = fixture.registry();
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");

    let runtime = ThreadRuntime::with_feature_resolver(
        Arc::clone(&registry),
        registry.memory_repository(),
        AppConfig::default().agent_config().compact_config().clone(),
        FeatureResolver::development_default(Features::default()),
    );
    let mut thread = Thread::new(
        ThreadContextLocator::new(None, "feishu", "ou_main", "thread_main", "thread_main"),
        Utc::now(),
    );
    runtime
        .initialize_thread(&mut thread, ThreadAgentKind::Main)
        .await
        .expect("main thread should initialize");

    let expected_main_prompt = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/prompts/thread_agent/main.md"),
    )
    .expect("main prompt markdown should load");

    assert_eq!(thread.thread_agent_kind(), ThreadAgentKind::Main);
    assert_eq!(thread.messages()[0].content, expected_main_prompt.trim());
}
