use crate::agent::tool::{build_thread, call_tool, list_tools};
use openjarvis::{
    agent::{
        ToolCallRequest, ToolRegistry,
        tool::obswiki::{
            ObswikiRuntimeConfig, ObswikiVaultLayout, register_obswiki_toolset_with_config,
        },
    },
    config::AppConfig,
};
use serde_json::json;
use std::{env::temp_dir, fs, path::PathBuf};
use uuid::Uuid;

struct ObswikiToolFixture {
    root: PathBuf,
}

impl ObswikiToolFixture {
    fn new(prefix: &str) -> Self {
        let root = temp_dir().join(format!("{prefix}-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("obswiki tool fixture root should exist");
        Self { root }
    }

    fn vault_root(&self) -> PathBuf {
        self.root.join("vault")
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

    fn obsidian_running_flag(&self) -> PathBuf {
        self.root.join("obsidian-running.flag")
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

    fn mark_obsidian_running(&self) {
        fs::write(self.obsidian_running_flag(), "running")
            .expect("obsidian running flag should be written");
    }

    fn runtime_config(&self) -> ObswikiRuntimeConfig {
        ObswikiVaultLayout::new(self.vault_root())
            .ensure_default_skeleton()
            .expect("obswiki test vault skeleton should exist");
        self.mark_obsidian_running();
        let obsidian_bin = self.write_obsidian_cli();
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
            self.vault_root().display(),
            obsidian_bin.display(),
        ))
        .expect("obswiki tool config should parse");
        ObswikiRuntimeConfig::from_agent_config(
            config.agent_config().tool_config().obswiki_config(),
        )
        .expect("enabled obswiki config should produce runtime config")
    }

    fn source_markdown(&self, name: &str, content: &str) -> PathBuf {
        let path = self.root.join(name);
        fs::write(&path, content).expect("source markdown should be written");
        path
    }
}

impl Drop for ObswikiToolFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn build_obswiki_thread(thread_id: &str) -> openjarvis::thread::Thread {
    let mut thread = build_thread(thread_id);
    thread.replace_loaded_toolsets(vec!["obswiki".to_string()]);
    thread
}

#[tokio::test]
async fn obswiki_toolset_exposes_only_core_tools_for_loaded_context() {
    // 测试场景: obswiki toolset 加载后，线程上下文里必须只看到约定的五个核心工具。
    let fixture = ObswikiToolFixture::new("openjarvis-obswiki-toolset-core");
    let registry = ToolRegistry::with_workspace_root(fixture.root.clone());
    register_obswiki_toolset_with_config(&registry, fixture.runtime_config())
        .await
        .expect("obswiki toolset should register");
    let thread = build_obswiki_thread("thread-obswiki-core");

    let tool_names = list_tools(&registry, &thread)
        .await
        .expect("tool list should succeed")
        .into_iter()
        .map(|definition| definition.name)
        .collect::<Vec<_>>();

    assert!(tool_names.contains(&"obswiki_import_raw".to_string()));
    assert!(tool_names.contains(&"obswiki_search".to_string()));
    assert!(tool_names.contains(&"obswiki_read".to_string()));
    assert!(tool_names.contains(&"obswiki_write".to_string()));
    assert!(tool_names.contains(&"obswiki_update".to_string()));
}

#[tokio::test]
async fn obswiki_tool_calls_round_trip_and_block_raw_mutation() {
    // 测试场景: 工具层要能完成 import/search/read/write/update round trip，并拒绝对 raw 层写回。
    let fixture = ObswikiToolFixture::new("openjarvis-obswiki-toolset-roundtrip");
    let registry = ToolRegistry::with_workspace_root(fixture.root.clone());
    register_obswiki_toolset_with_config(&registry, fixture.runtime_config())
        .await
        .expect("obswiki toolset should register");
    let mut thread = build_obswiki_thread("thread-obswiki-roundtrip");
    let source = fixture.source_markdown("source.md", "# Source\n\nalpha fact");

    let imported = call_tool(
        &registry,
        &mut thread,
        ToolCallRequest {
            name: "obswiki_import_raw".to_string(),
            arguments: json!({
                "source_path": source,
                "title": "Source",
                "source_uri": "https://example.com/source",
            }),
        },
    )
    .await
    .expect("raw import should succeed");
    let written = call_tool(
        &registry,
        &mut thread,
        ToolCallRequest {
            name: "obswiki_write".to_string(),
            arguments: json!({
                "path": "wiki/topic.md",
                "title": "Topic",
                "content": "# Topic\n\nalpha fact",
                "page_type": "topic",
                "links": ["raw/source.md"],
            }),
        },
    )
    .await
    .expect("wiki write should succeed");
    let search = call_tool(
        &registry,
        &mut thread,
        ToolCallRequest {
            name: "obswiki_search".to_string(),
            arguments: json!({
                "query": "alpha",
                "scope": "wiki",
                "limit": 5,
            }),
        },
    )
    .await
    .expect("search should succeed");
    let read = call_tool(
        &registry,
        &mut thread,
        ToolCallRequest {
            name: "obswiki_read".to_string(),
            arguments: json!({
                "path": "wiki/topic.md",
            }),
        },
    )
    .await
    .expect("read should succeed");
    let updated = call_tool(
        &registry,
        &mut thread,
        ToolCallRequest {
            name: "obswiki_update".to_string(),
            arguments: json!({
                "path": "wiki/topic.md",
                "instructions": "operation: append\ncontent: |\n  \n  beta fact\n",
            }),
        },
    )
    .await
    .expect("update should succeed");
    let error = call_tool(
        &registry,
        &mut thread,
        ToolCallRequest {
            name: "obswiki_update".to_string(),
            arguments: json!({
                "path": "raw/source.md",
                "instructions": "operation: append\ncontent: |\n  forbidden\n",
            }),
        },
    )
    .await
    .expect_err("raw mutation should be rejected");

    assert_eq!(imported.metadata["event_kind"], "obswiki_import_raw");
    assert_eq!(written.metadata["event_kind"], "obswiki_write");
    assert_eq!(search.metadata["payload"]["backend"], "obsidian");
    assert!(
        read.metadata["payload"]["content"]
            .as_str()
            .expect("read payload content should be a string")
            .contains("alpha fact")
    );
    assert!(
        updated.metadata["payload"]["content"]
            .as_str()
            .expect("update payload content should be a string")
            .contains("beta fact")
    );
    assert!(error.to_string().contains("immutable `raw/` layer"));
}
