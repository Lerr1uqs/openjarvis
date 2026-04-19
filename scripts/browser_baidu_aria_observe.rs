//! Standalone Rust observation binary that captures both browser snapshot and ARIA snapshot for
//! the Baidu homepage through the Rust browser session manager.

use anyhow::{Context, Result};
use chrono::Utc;
use clap::Parser;
use openjarvis::agent::tool::browser::{
    BrowserOpenRequest, BrowserProcessCommandSpec, BrowserRuntimeOptions, BrowserSessionManager,
    BrowserSessionManagerConfig, default_sidecar_script_path,
};
use serde_json::json;
use std::{fs, path::PathBuf};

/// Command-line options for the Baidu ARIA observation binary.
#[derive(Debug, Clone, Parser)]
#[command(name = "browser_baidu_aria_observe")]
struct BrowserBaiduAriaObserveCli {
    /// Target URL captured by this observation run.
    #[arg(long, default_value = "https://www.baidu.com/")]
    url: String,
    /// Run the browser in headless mode.
    #[arg(long, default_value_t = true)]
    headless: bool,
    /// Override the Node.js executable used to launch the browser sidecar.
    #[arg(long, default_value = "node")]
    node_bin: String,
    /// Override the browser sidecar script path.
    #[arg(long)]
    script_path: Option<PathBuf>,
    /// Optional explicit Chrome executable path.
    #[arg(long)]
    chrome_path: Option<PathBuf>,
}

/// Capture one Baidu observation run and persist both snapshot forms into `observation/`.
///
/// # 示例
/// ```rust,no_run
/// # async fn demo() -> anyhow::Result<()> {
/// let cli = clap::Parser::parse_from(["browser_baidu_aria_observe"]);
/// # let _ = cli;
/// # Ok(())
/// # }
/// ```
async fn run_baidu_observation(cli: &BrowserBaiduAriaObserveCli) -> Result<()> {
    let captured_at = Utc::now();
    let run_dir = observation_root().join("runs").join(format!(
        "capture-{}",
        captured_at.format("%Y%m%dT%H%M%S%.3fZ")
    ));
    fs::create_dir_all(&run_dir).with_context(|| {
        format!(
            "failed to create Baidu observation run dir {}",
            run_dir.display()
        )
    })?;

    let manager = BrowserSessionManager::new(build_manager_config(cli, &run_dir));
    let thread_id = "browser-baidu-aria-observe";
    let open = manager
        .open(thread_id, BrowserOpenRequest::launch())
        .await?;
    let navigate = manager.navigate(thread_id, &cli.url).await?;
    let snapshot = manager.snapshot(thread_id, Some(160)).await?;
    let aria_snapshot = manager.aria_snapshot(thread_id).await?;
    let screenshot_path = run_dir.join("baidu-homepage.png");
    let screenshot = manager
        .screenshot(thread_id, Some(&screenshot_path))
        .await?;
    let close = manager.close(thread_id).await?;

    let browser_snapshot_path = run_dir.join("browser-snapshot.txt");
    let aria_snapshot_path = run_dir.join("aria-snapshot.yaml");
    let metadata_path = run_dir.join("page-metadata.json");
    fs::write(&browser_snapshot_path, &snapshot.snapshot_text).with_context(|| {
        format!(
            "failed to write browser snapshot artifact {}",
            browser_snapshot_path.display()
        )
    })?;
    fs::write(&aria_snapshot_path, &aria_snapshot.aria_snapshot).with_context(|| {
        format!(
            "failed to write aria snapshot artifact {}",
            aria_snapshot_path.display()
        )
    })?;
    fs::write(
        &metadata_path,
        serde_json::to_vec_pretty(&json!({
            "target_url": cli.url,
            "final_url": navigate.url,
            "title": navigate.title,
            "captured_at": captured_at.to_rfc3339(),
            "cookies_loaded": open.cookies_loaded,
            "browser_snapshot": browser_snapshot_path.file_name().and_then(|name| name.to_str()),
            "aria_snapshot": aria_snapshot_path.file_name().and_then(|name| name.to_str()),
            "screenshot": PathBuf::from(&screenshot.path).file_name().and_then(|name| name.to_str()),
            "session_artifacts": close
                .artifacts
                .as_ref()
                .map(|artifacts| artifacts.session_dir.display().to_string()),
        }))?,
    )
    .with_context(|| format!("failed to write metadata file {}", metadata_path.display()))?;

    println!("run_dir: {}", run_dir.display());
    println!("final_url: {}", navigate.url);
    println!("title: {}", navigate.title);
    println!("cookies_loaded: {}", open.cookies_loaded);
    println!("browser_snapshot: {}", browser_snapshot_path.display());
    println!("aria_snapshot: {}", aria_snapshot_path.display());
    println!("screenshot: {}", screenshot.path);

    Ok(())
}

/// Build the browser session manager config used by the Baidu observation binary.
fn build_manager_config(
    cli: &BrowserBaiduAriaObserveCli,
    run_dir: &PathBuf,
) -> BrowserSessionManagerConfig {
    let script_path = cli
        .script_path
        .clone()
        .unwrap_or_else(default_sidecar_script_path);
    BrowserSessionManagerConfig {
        process: BrowserProcessCommandSpec {
            executable: cli.node_bin.clone(),
            args: vec![script_path.display().to_string()],
            env: Default::default(),
        },
        runtime: BrowserRuntimeOptions {
            headless: cli.headless,
            keep_artifacts: true,
            chrome_executable: cli.chrome_path.clone(),
            ..Default::default()
        },
        artifact_root: run_dir.join("session-artifacts"),
    }
}

/// Return the workspace-local output directory for Baidu observations.
fn observation_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("observation")
        .join("aria-snapshot")
        .join("baidu")
}

/// Main entrypoint for the Baidu ARIA observation binary.
#[tokio::main]
async fn main() -> Result<()> {
    let cli = BrowserBaiduAriaObserveCli::parse();
    run_baidu_observation(&cli).await
}
