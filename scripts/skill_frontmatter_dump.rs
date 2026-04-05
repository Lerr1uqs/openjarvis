//! Print all discovered local skill frontmatter `name` and `description` fields from one workspace.

use anyhow::{Result, bail};
use comfy_table::{
    Cell, ContentArrangement, Table, modifiers::UTF8_ROUND_CORNERS, presets::UTF8_FULL,
};
use openjarvis::{agent::SkillManifest, skill::list_local_skill_manifests};
use std::{
    env,
    path::{Path, PathBuf},
};

struct Args {
    workspace_root: PathBuf,
    output_mode: OutputMode,
    table_width: Option<u16>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OutputMode {
    Table,
    Plain,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = parse_args()?;
    let manifests = list_local_skill_manifests(&args.workspace_root).await?;

    match args.output_mode {
        OutputMode::Plain => print_plain(&manifests),
        OutputMode::Table => print_table(&manifests, args.table_width, &args.workspace_root),
    }

    Ok(())
}

fn parse_args() -> Result<Args> {
    let mut args = env::args().skip(1);
    let mut workspace_root = None;
    let mut output_mode = OutputMode::Table;
    let mut table_width = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--workspace" => {
                let Some(path) = args.next() else {
                    bail!("`--workspace` requires a path");
                };
                workspace_root = Some(PathBuf::from(path));
            }
            "--plain" => output_mode = OutputMode::Plain,
            "--width" => {
                let Some(raw_width) = args.next() else {
                    bail!("`--width` requires a positive integer");
                };
                table_width = Some(
                    raw_width
                        .parse::<u16>()
                        .ok()
                        .filter(|width| *width >= 40)
                        .ok_or_else(|| anyhow::anyhow!("`--width` requires an integer >= 40"))?,
                );
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
        output_mode,
        table_width,
    })
}

fn print_help() {
    eprintln!(
        "Usage: cargo run --bin skill_frontmatter_dump -- [--workspace <path>] [--plain] [--width <n>]"
    );
}

fn print_plain(manifests: &[SkillManifest]) {
    for manifest in manifests {
        println!("{}\t{}", manifest.name, manifest.description);
    }
}

fn print_table(manifests: &[SkillManifest], table_width: Option<u16>, workspace_root: &Path) {
    if manifests.is_empty() {
        println!(
            "No local skills found under {}.",
            workspace_root.join(".openjarvis/skills").display()
        );
        return;
    }

    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec!["#", "name", "description"]);
    if let Some(table_width) = table_width {
        table.set_width(table_width);
    }
    for (index, manifest) in manifests.iter().enumerate() {
        table.add_row(vec![
            Cell::new(index + 1),
            Cell::new(&manifest.name),
            Cell::new(&manifest.description),
        ]);
    }

    println!("{table}");
    println!(
        "{} skill(s) from {}",
        manifests.len(),
        workspace_root.join(".openjarvis/skills").display()
    );
}
