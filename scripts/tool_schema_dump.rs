//! Dump all program-defined tool schemas after loading every local toolset into one thread.

use anyhow::{Result, bail};
use chrono::Utc;
use openjarvis::{
    agent::{ToolRegistry, ToolSchemaProtocol},
    thread::{Thread, ThreadContextLocator},
};
use serde::Serialize;
use std::{env, path::PathBuf};

struct Args {
    workspace_root: PathBuf,
}

#[derive(Debug, Serialize)]
struct ToolSchemaDump {
    workspace_root: String,
    loaded_toolsets: Vec<String>,
    tool_count: usize,
    tools: Vec<ToolSchemaEntry>,
}

#[derive(Debug, Serialize)]
struct ToolSchemaEntry {
    name: String,
    description: String,
    source: serde_json::Value,
    input_schema: serde_json::Value,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = parse_args()?;
    let registry = ToolRegistry::with_workspace_root(args.workspace_root.clone());
    registry.register_builtin_tools().await?;

    let mut thread_context = build_dump_thread();
    let toolsets = registry.list_toolsets().await;
    for toolset in &toolsets {
        registry
            .open_tool(&mut thread_context, &toolset.name)
            .await?;
    }

    let definitions = registry.list_for_context_static(&thread_context).await?;
    let dump = ToolSchemaDump {
        workspace_root: args.workspace_root.display().to_string(),
        loaded_toolsets: thread_context.load_toolsets(),
        tool_count: definitions.len(),
        tools: definitions
            .into_iter()
            .map(|definition| ToolSchemaEntry {
                name: definition.name,
                description: definition.description,
                source: serde_json::to_value(definition.source)
                    .expect("tool source should serialize"),
                input_schema: definition
                    .input_schema
                    .for_protocol(ToolSchemaProtocol::OpenAi),
            })
            .collect(),
    };

    println!("{}", serde_json::to_string_pretty(&dump)?);
    Ok(())
}

fn parse_args() -> Result<Args> {
    let mut args = env::args().skip(1);
    let mut workspace_root = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--workspace" => {
                let Some(path) = args.next() else {
                    bail!("`--workspace` requires a path");
                };
                workspace_root = Some(PathBuf::from(path));
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            other => bail!("unexpected argument `{other}`"),
        }
    }

    Ok(Args {
        workspace_root: workspace_root.unwrap_or(env::current_dir()?),
    })
}

fn build_dump_thread() -> Thread {
    Thread::new(
        ThreadContextLocator::new(
            None,
            "debug",
            "tool_schema_dump",
            "tool_schema_dump",
            "tool_schema_dump",
        ),
        Utc::now(),
    )
}

fn print_help() {
    eprintln!("Usage: cargo run --bin tool_schema_dump -- [--workspace <path>]");
}
