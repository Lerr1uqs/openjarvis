//! Direct memory capability probe that exercises `memory_search` and `memory_get` without using
//! the agent loop.

use anyhow::{Context, Result, bail};
use chrono::Utc;
use openjarvis::{
    agent::{ToolCallRequest, ToolRegistry},
    config::AppConfig,
    thread::{Thread, ThreadContextLocator},
};
use serde_json::{Value, json};
use std::{
    env, fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

struct Args {
    config_path: Option<PathBuf>,
    mode: ProbeMode,
}

#[derive(Clone, Copy)]
enum ProbeMode {
    Lexical,
    Hybrid,
}

impl ProbeMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Lexical => "lexical",
            Self::Hybrid => "hybrid",
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = parse_args()?;
    let base_config = load_base_config(args.config_path.as_deref())?;
    let probe_config = build_probe_config(&base_config, args.mode)?;
    let workspace_root = prepare_probe_workspace()?;
    let original_cwd = env::current_dir().context("failed to capture current working directory")?;

    eprintln!(
        "memory_search_probe: workspace={}",
        workspace_root.display()
    );
    eprintln!("memory_search_probe: mode={}", args.mode.as_str());
    let hybrid = probe_config
        .agent_config()
        .tool_config()
        .memory_config()
        .search_config()
        .hybrid_config();
    eprintln!(
        "memory_search_probe: base_url={}, api_key_path={}, embedding_model={}, rerank_model={}",
        hybrid.base_url(),
        hybrid.api_key_path().display(),
        hybrid.embedding_model(),
        hybrid.rerank_model()
    );

    env::set_current_dir(&workspace_root)
        .with_context(|| format!("failed to switch cwd to {}", workspace_root.display()))?;
    let run_result = run_probe(&probe_config).await;
    env::set_current_dir(&original_cwd)
        .with_context(|| format!("failed to restore cwd to {}", original_cwd.display()))?;
    run_result?;

    Ok(())
}

fn parse_args() -> Result<Args> {
    let mut args = env::args().skip(1);
    let mut config_path = None;
    let mut mode = ProbeMode::Hybrid;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => {
                let Some(path) = args.next() else {
                    bail!("`--config` requires a file path");
                };
                config_path = Some(PathBuf::from(path));
            }
            "--mode" => {
                let Some(value) = args.next() else {
                    bail!("`--mode` requires `lexical` or `hybrid`");
                };
                mode = parse_mode(&value)?;
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            other => bail!("unexpected argument `{other}`"),
        }
    }

    Ok(Args { config_path, mode })
}

fn parse_mode(value: &str) -> Result<ProbeMode> {
    match value {
        "lexical" => Ok(ProbeMode::Lexical),
        "hybrid" => Ok(ProbeMode::Hybrid),
        other => bail!("unsupported mode `{other}`, expected `lexical` or `hybrid`"),
    }
}

fn print_help() {
    eprintln!(
        "Usage: cargo run --bin memory_search_probe -- [--config <path>] [--mode lexical|hybrid]"
    );
}

fn load_base_config(config_path: Option<&Path>) -> Result<AppConfig> {
    match config_path {
        Some(path) => AppConfig::from_path(path),
        None => AppConfig::load(),
    }
}

fn build_probe_config(base_config: &AppConfig, mode: ProbeMode) -> Result<AppConfig> {
    let hybrid = base_config
        .agent_config()
        .tool_config()
        .memory_config()
        .search_config()
        .hybrid_config();
    let yaml = serde_yaml::to_string(&json!({
        "agent": {
            "tool": {
                "memory": {
                    "search": {
                        "mode": mode.as_str(),
                        "hybrid": {
                            "base_url": hybrid.base_url(),
                            "api_key_path": hybrid.api_key_path().display().to_string(),
                            "embedding_model": hybrid.embedding_model(),
                            "rerank_model": hybrid.rerank_model(),
                            "bm25_top_n": hybrid.bm25_top_n(),
                            "dense_top_n": hybrid.dense_top_n(),
                            "rerank_top_n": hybrid.rerank_top_n(),
                            "rrf_k": hybrid.rrf_k(),
                            "mmr_lambda": hybrid.mmr_lambda(),
                            "freshness_half_life_days": hybrid.freshness_half_life_days(),
                        }
                    }
                }
            }
        },
        "llm": {
            "protocol": "mock",
            "provider": "mock",
        }
    }))
    .context("failed to render memory probe config yaml")?;
    AppConfig::from_yaml_str(&yaml).context("failed to parse memory probe config")
}

fn prepare_probe_workspace() -> Result<PathBuf> {
    let workspace_root =
        env::temp_dir().join(format!("openjarvis-memory-search-probe-{}", Uuid::new_v4()));
    fs::create_dir_all(&workspace_root).with_context(|| {
        format!(
            "failed to create probe workspace {}",
            workspace_root.display()
        )
    })?;

    write_memory_document(
        &workspace_root,
        "passive/preferences/semantic-style.md",
        r#"---
title: "回答风格约定"
created_at: 2026-04-01T10:00:00Z
updated_at: 2026-04-01T10:00:00Z
---
输出尽量简洁，默认中文，并把重点放在结论前面。
"#,
    )?;
    write_memory_document(
        &workspace_root,
        "passive/preferences/semantic-style-fresh.md",
        r#"---
title: "最新回答风格"
created_at: 2026-04-10T10:00:00Z
updated_at: 2026-04-18T10:00:00Z
---
最近更新：默认使用中文，回答保持简洁，先给结论再展开细节。
"#,
    )?;
    write_memory_document(
        &workspace_root,
        "passive/preferences/noise.md",
        r#"---
title: "周末随笔"
created_at: 2026-04-03T10:00:00Z
updated_at: 2026-04-03T10:00:00Z
---
这是一篇纯噪声文档，只记录天气、早餐和周末散步。
"#,
    )?;
    write_memory_document(
        &workspace_root,
        "active/workflow/notion.md",
        r#"---
title: "Notion 上传工作流"
created_at: 2026-04-05T10:00:00Z
updated_at: 2026-04-05T10:00:00Z
keywords:
  - notion
  - 上传
---
上传到 notion 时走用户自定义模板，保留原始链接。
"#,
    )?;

    Ok(workspace_root)
}

fn write_memory_document(workspace_root: &Path, relative_path: &str, content: &str) -> Result<()> {
    let path = workspace_root
        .join(".openjarvis/memory")
        .join(relative_path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create memory parent {}", parent.display()))?;
    }
    fs::write(&path, content)
        .with_context(|| format!("failed to write memory document {}", path.display()))?;
    Ok(())
}

async fn run_probe(config: &AppConfig) -> Result<()> {
    let registry =
        ToolRegistry::from_config_with_skill_roots(config.agent_config().tool_config(), Vec::new())
            .await
            .context("failed to build probe tool registry")?;
    registry
        .register_builtin_tools()
        .await
        .context("failed to register builtin tools")?;
    let mut thread = build_probe_thread();

    call_tool(
        &registry,
        &mut thread,
        "load_toolset",
        json!({ "name": "memory" }),
    )
    .await?;

    let passive_search = call_tool(
        &registry,
        &mut thread,
        "memory_search",
        json!({
            "query": "以后回答时记住我的表达方式",
            "type": "passive",
            "limit": 3,
        }),
    )
    .await?;
    println!("=== passive memory_search ===");
    println!("{passive_search}");

    let passive_top_path = first_item_path(&passive_search)
        .context("passive memory_search did not return any items")?;
    let passive_get = call_tool(
        &registry,
        &mut thread,
        "memory_get",
        json!({
            "path": passive_top_path,
            "type": "passive",
        }),
    )
    .await?;
    println!("=== passive memory_get(top1) ===");
    println!("{passive_get}");

    let active_search = call_tool(
        &registry,
        &mut thread,
        "memory_search",
        json!({
            "query": "notion 上传模板",
            "type": "active",
            "limit": 3,
        }),
    )
    .await?;
    println!("=== active memory_search ===");
    println!("{active_search}");

    Ok(())
}

fn build_probe_thread() -> Thread {
    Thread::new(
        ThreadContextLocator::new(
            None,
            "probe",
            "memory-probe-user",
            "memory-probe-thread",
            "memory-probe-thread",
        ),
        Utc::now(),
    )
}

async fn call_tool(
    registry: &ToolRegistry,
    thread: &mut Thread,
    name: &str,
    arguments: Value,
) -> Result<String> {
    let result = registry
        .call_for_context(
            thread,
            ToolCallRequest {
                name: name.to_string(),
                arguments,
            },
        )
        .await
        .with_context(|| format!("tool `{name}` invocation failed"))?;
    Ok(result.content)
}

fn first_item_path(payload: &str) -> Result<String> {
    let parsed =
        serde_json::from_str::<Value>(payload).context("tool payload is not valid json")?;
    parsed["items"]
        .as_array()
        .and_then(|items| items.first())
        .and_then(|item| item["path"].as_str())
        .map(str::to_string)
        .context("tool payload does not contain `items[0].path`")
}
