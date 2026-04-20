use openjarvis::agent::tool::obswiki::{
    OBSWIKI_AGENTS_FILE_NAME, OBSWIKI_INDEX_FILE_NAME, OBSWIKI_RAW_DIR_NAME,
    OBSWIKI_SCHEMA_DIR_NAME, OBSWIKI_WIKI_DIR_NAME, ObswikiRuntime, ObswikiRuntimeConfig,
    ObswikiVaultLayout, is_mutable_obswiki_path, is_raw_obswiki_path,
    validate_obswiki_markdown_path,
};
use openjarvis::config::AppConfig;
use std::{
    env::temp_dir,
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

struct ObswikiFixture {
    root: PathBuf,
}

impl ObswikiFixture {
    fn new(prefix: &str) -> Self {
        let root = temp_dir().join(format!("{prefix}-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("obswiki fixture root should exist");
        Self { root }
    }

    fn path(&self) -> &Path {
        &self.root
    }

    fn vault_root(&self) -> PathBuf {
        self.root.join("vault")
    }

    fn qmd_fail_flag(&self) -> PathBuf {
        self.root.join("qmd-fail.flag")
    }

    fn obsidian_running_flag(&self) -> PathBuf {
        self.root.join("obsidian-running.flag")
    }

    fn write_script(&self, name: &str, content: &str) -> PathBuf {
        let path = self.root.join(name);
        fs::write(&path, content).expect("mock script should be written");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let metadata = fs::metadata(&path).expect("mock cli metadata should exist");
            let mut permissions = metadata.permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&path, permissions).expect("mock cli permissions should apply");
        }
        path
    }

    fn write_obsidian_cli(&self) -> PathBuf {
        let running_flag = self.obsidian_running_flag();
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
        self.write_script("mock-obsidian-cli.py", &script)
    }

    fn write_obsidian_cli_with_soft_agents_probe_error(&self) -> PathBuf {
        let running_flag = self.obsidian_running_flag();
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

if command == "read" and data.get("path") == "AGENTS.md":
    print('Error: File "AGENTS.md" not found.')
    sys.exit(0)

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
        self.write_script("mock-obsidian-cli-soft-error.py", &script)
    }

    fn mark_obsidian_running(&self) {
        fs::write(self.obsidian_running_flag(), "running")
            .expect("obsidian running flag should be written");
    }

    fn write_qmd_cli(&self) -> PathBuf {
        let fail_flag = self.qmd_fail_flag();
        self.write_script(
            "mock-qmd.py",
            &format!(
                r##"#!/usr/bin/env python3
import json
import sys
from pathlib import Path

fail_flag = Path(r"{}")

if len(sys.argv) >= 2 and sys.argv[1] == "--help":
    sys.exit(0)

if len(sys.argv) >= 2 and sys.argv[1] == "search":
    if fail_flag.exists():
        print("forced qmd failure", file=sys.stderr)
        sys.exit(3)
    query = ""
    limit = 10
    args = sys.argv[2:]
    index = 0
    while index < len(args):
        value = args[index]
        if value == "--json":
            index += 1
            continue
        if value == "-n":
            limit = int(args[index + 1])
            index += 2
            continue
        query = value
        index += 1

    query = query.lower()
    results = []
    for file in sorted(Path.cwd().rglob("*.md")):
        rel = file.relative_to(Path.cwd()).as_posix()
        text = file.read_text(encoding="utf-8")
        if query in rel.lower() or query in text.lower():
            title = file.stem
            for line in text.splitlines():
                if line.startswith("# "):
                    title = line[2:].strip()
                    break
            summary = next((line.strip() for line in text.splitlines() if line.strip()), "")
            results.append({{"path": rel, "title": title, "summary": summary}})
            if len(results) >= limit:
                break
    print(json.dumps(results))
    sys.exit(0)

print("unsupported qmd invocation", file=sys.stderr)
sys.exit(8)
"##,
                fail_flag.display()
            ),
        )
    }

    fn build_runtime(&self, with_qmd: bool) -> ObswikiRuntime {
        let layout = ObswikiVaultLayout::new(self.vault_root());
        layout
            .ensure_default_skeleton()
            .expect("vault skeleton should be created");
        self.mark_obsidian_running();
        let obsidian_bin = self.write_obsidian_cli();
        let qmd_yaml = if with_qmd {
            format!("      qmd_bin: \"{}\"\n", self.write_qmd_cli().display())
        } else {
            String::new()
        };
        let config = AppConfig::from_yaml_str(&format!(
            r#"
agent:
  tool:
    obswiki:
      enabled: true
      vault_path: "{}"
      obsidian_bin: "{}"
{}llm:
  protocol: "mock"
  provider: "mock"
"#,
            layout.root().display(),
            obsidian_bin.display(),
            qmd_yaml,
        ))
        .expect("obswiki config should parse");
        ObswikiRuntime::new(
            ObswikiRuntimeConfig::from_agent_config(
                config.agent_config().tool_config().obswiki_config(),
            )
            .expect("enabled config should produce runtime config"),
        )
    }
}

impl Drop for ObswikiFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn obswiki_default_skeleton_creates_required_layout_and_docs() {
    // 测试场景: 默认 workspace vault 骨架必须创建 raw/wiki/schema 目录以及 AGENTS.md/index.md/schema README。
    let fixture = ObswikiFixture::new("openjarvis-obswiki-skeleton");
    let layout = ObswikiVaultLayout::default_for_workspace(fixture.path());

    let created = layout
        .ensure_default_skeleton()
        .expect("default obswiki skeleton should be created");

    assert!(created);
    assert!(layout.raw_dir().is_dir());
    assert!(layout.wiki_dir().is_dir());
    assert!(layout.schema_dir().is_dir());
    assert!(layout.agents_file().is_file());
    assert!(layout.index_file().is_file());
    assert!(layout.schema_readme_file().is_file());

    let agents = fs::read_to_string(layout.agents_file()).expect("AGENTS.md should be readable");
    assert!(agents.contains("raw/"));
    assert!(agents.contains("wiki/"));
    assert!(agents.contains("schema/"));
    assert!(agents.contains("严禁改写 `raw/`"));

    let index = fs::read_to_string(layout.index_file()).expect("index.md should be readable");
    assert!(index.contains("当前暂无条目"));
}

#[test]
fn obswiki_runtime_preflight_accepts_complete_vault_and_optional_qmd() {
    // 测试场景: 完整 skeleton + 可执行 obsidian cli/qmd mock 且 app 已运行时，preflight 必须通过并记录 QMD 可用状态。
    let fixture = ObswikiFixture::new("openjarvis-obswiki-preflight-ok");
    let runtime = fixture.build_runtime(true);
    let status = runtime.preflight().expect("preflight should succeed");

    assert!(status.skeleton_complete());
    assert!(status.obsidian_cli_available);
    assert!(status.qmd_configured);
    assert!(status.qmd_cli_available);
}

#[test]
fn obswiki_runtime_preflight_rejects_when_obsidian_app_is_not_running() {
    // 测试场景: skeleton 完整但 Obsidian app 未运行时，preflight 必须快速失败并提示手动启动。
    let fixture = ObswikiFixture::new("openjarvis-obswiki-preflight-app-missing");
    let vault_root = fixture.path().join("vault");
    ObswikiVaultLayout::new(&vault_root)
        .ensure_default_skeleton()
        .expect("vault skeleton should exist");
    let obsidian_bin = fixture.write_obsidian_cli();
    let runtime = ObswikiRuntime::new(
        ObswikiRuntimeConfig::from_agent_config(
            AppConfig::from_yaml_str(&format!(
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
            .expect("config should parse")
            .agent_config()
            .tool_config()
            .obswiki_config(),
        )
        .expect("enabled config should produce runtime config"),
    );

    let error = runtime
        .preflight()
        .expect_err("missing running obsidian app should fail preflight");

    assert!(format!("{error:#}").contains("open that vault in Obsidian manually and retry"));
}

#[test]
fn obswiki_runtime_preflight_rejects_soft_cli_agents_probe_errors() {
    // 测试场景: 即使 CLI 返回 0，只要 AGENTS.md probe 返回错误文本，preflight 也必须拒绝错误 vault 绑定。
    let fixture = ObswikiFixture::new("openjarvis-obswiki-preflight-soft-error");
    let vault_root = fixture.path().join("vault");
    ObswikiVaultLayout::new(&vault_root)
        .ensure_default_skeleton()
        .expect("vault skeleton should exist");
    fixture.mark_obsidian_running();
    let obsidian_bin = fixture.write_obsidian_cli_with_soft_agents_probe_error();
    let runtime = ObswikiRuntime::new(
        ObswikiRuntimeConfig::from_agent_config(
            AppConfig::from_yaml_str(&format!(
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
            .expect("config should parse")
            .agent_config()
            .tool_config()
            .obswiki_config(),
        )
        .expect("enabled config should produce runtime config"),
    );

    let error = runtime
        .preflight()
        .expect_err("soft CLI AGENTS probe errors should fail preflight");

    assert!(format!("{error:#}").contains("open that vault in Obsidian manually and retry"));
}

#[test]
fn obswiki_runtime_preflight_rejects_missing_skeleton_entries() {
    // 测试场景: vault 缺少 AGENTS.md 等必需骨架时，preflight 必须直接失败而不是静默通过。
    let fixture = ObswikiFixture::new("openjarvis-obswiki-preflight-missing");
    let vault_root = fixture.path().join("vault");
    fs::create_dir_all(vault_root.join(OBSWIKI_RAW_DIR_NAME)).expect("raw dir should exist");
    fs::create_dir_all(vault_root.join(OBSWIKI_WIKI_DIR_NAME)).expect("wiki dir should exist");
    fs::create_dir_all(vault_root.join(OBSWIKI_SCHEMA_DIR_NAME)).expect("schema dir should exist");
    fs::write(vault_root.join(OBSWIKI_INDEX_FILE_NAME), "# index\n").expect("index should exist");
    let obsidian_bin = fixture.write_obsidian_cli();

    let runtime = ObswikiRuntime::new(
        ObswikiRuntimeConfig::from_agent_config(
            AppConfig::from_yaml_str(&format!(
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
            .expect("config should parse")
            .agent_config()
            .tool_config()
            .obswiki_config(),
        )
        .expect("enabled config should produce runtime config"),
    );

    let error = runtime
        .preflight()
        .expect_err("missing AGENTS.md should fail preflight");

    assert!(format!("{error:#}").contains(OBSWIKI_AGENTS_FILE_NAME));
}

#[test]
fn obswiki_path_validation_rejects_escape_and_non_markdown_targets() {
    // 测试场景: vault 相对路径必须防止路径逃逸，并且只接受 markdown 目标。
    let error = validate_obswiki_markdown_path("../escape.md")
        .expect_err("parent traversal should be rejected");
    assert!(error.to_string().contains("parent directory traversal"));

    let error = validate_obswiki_markdown_path("wiki/not-markdown.txt")
        .expect_err("non-markdown path should be rejected");
    assert!(error.to_string().contains("must end with `.md`"));
}

#[test]
fn obswiki_path_helpers_distinguish_mutable_and_raw_layers() {
    // 测试场景: 写回层判断必须只允许 wiki/schema 可变，并显式识别 raw 不可变层。
    let wiki = validate_obswiki_markdown_path("wiki/topic.md").expect("wiki path should parse");
    let schema =
        validate_obswiki_markdown_path("schema/page-rule.md").expect("schema path should parse");
    let raw = validate_obswiki_markdown_path("raw/source.md").expect("raw path should parse");

    assert!(is_mutable_obswiki_path(&wiki));
    assert!(is_mutable_obswiki_path(&schema));
    assert!(!is_mutable_obswiki_path(&raw));
    assert!(is_raw_obswiki_path(&raw));
}

#[tokio::test]
async fn obswiki_runtime_write_update_and_import_refresh_index() {
    // 测试场景: Raw 导入、wiki 写入和定向更新后都必须自动刷新 index.md。
    let fixture = ObswikiFixture::new("openjarvis-obswiki-runtime-index");
    let runtime = fixture.build_runtime(false);

    let source = fixture.path().join("source.md");
    fs::write(&source, "# Source\n\nraw fact").expect("source markdown should be written");

    let imported = runtime
        .import_raw_markdown(
            &source,
            Some("Source Note"),
            Some("https://example.com/source"),
        )
        .await
        .expect("raw import should succeed");
    let written = runtime
        .write_document(
            "wiki/topic.md",
            "Topic",
            "# Topic\n\nInitial fact",
            Some("concept"),
            None,
            None,
        )
        .await
        .expect("wiki write should succeed");
    let updated = runtime
        .update_document(
            "wiki/topic.md",
            openjarvis::agent::tool::obswiki::ObswikiUpdateInstruction::Append {
                content: "\nMore fact".to_string(),
            },
            None,
            None,
        )
        .await
        .expect("wiki update should succeed");
    let index = runtime
        .read_document(OBSWIKI_INDEX_FILE_NAME)
        .await
        .expect("index should be readable");

    assert!(imported.path.starts_with("raw/"));
    assert_eq!(written.path, "wiki/topic.md");
    assert!(updated.content.contains("More fact"));
    assert!(index.content.contains(&format!("[{}|", imported.path)));
    assert!(index.content.contains("[wiki/topic.md|"));
}

#[tokio::test]
async fn obswiki_runtime_search_prefers_qmd_and_falls_back_to_obsidian() {
    // 测试场景: QMD 可用时搜索应优先走 qmd；当 qmd search 失败时必须自动回退到 obsidian backend。
    let fixture = ObswikiFixture::new("openjarvis-obswiki-runtime-search");
    let runtime = fixture.build_runtime(true);

    runtime
        .write_document(
            "wiki/rust.md",
            "Rust",
            "# Rust\n\nOwnership and borrowing",
            Some("topic"),
            None,
            None,
        )
        .await
        .expect("wiki write should succeed");

    let qmd = runtime
        .search("ownership", Some("wiki"), 5)
        .await
        .expect("qmd search should succeed");
    fs::write(fixture.qmd_fail_flag(), "fail").expect("qmd fail flag should be written");
    let fallback = runtime
        .search("ownership", Some("wiki"), 5)
        .await
        .expect("obsidian fallback search should succeed");

    assert_eq!(qmd.backend, "qmd");
    assert_eq!(qmd.items[0].backend, "qmd");
    assert_eq!(fallback.backend, "obsidian");
    assert_eq!(fallback.items[0].backend, "obsidian");
}

#[tokio::test]
async fn obswiki_runtime_loads_vault_context_from_agents_and_index() {
    // 测试场景: child thread 初始化前必须能读取 preflight 状态以及 AGENTS.md/index.md 正文。
    let fixture = ObswikiFixture::new("openjarvis-obswiki-runtime-context");
    let runtime = fixture.build_runtime(false);
    runtime
        .write_document(
            "wiki/context.md",
            "Context",
            "# Context\n\nContext fact",
            Some("topic"),
            None,
            None,
        )
        .await
        .expect("wiki write should succeed");

    let context = runtime
        .load_vault_context()
        .await
        .expect("vault context should load");

    assert!(context.preflight.skeleton_complete());
    assert!(context.agents_body.contains("Obswiki Vault Instructions"));
    assert!(context.index_body.contains("[wiki/context.md|"));
}
