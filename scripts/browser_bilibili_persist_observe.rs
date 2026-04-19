//! Standalone Rust observation binary that validates cookie persistence on the Bilibili homepage
//! through the Rust browser session manager.

use anyhow::{Context, Result, bail};
use chrono::Utc;
use clap::{Parser, ValueEnum};
use openjarvis::agent::tool::browser::{
    BrowserOpenRequest, BrowserProcessCommandSpec, BrowserRuntimeOptions, BrowserSessionManager,
    BrowserSessionManagerConfig, default_sidecar_script_path,
};
use serde_json::json;
use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
};

/// Observation mode used by the Bilibili persistence binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum BilibiliPersistMode {
    Login,
    Capture,
}

/// Command-line options for the Bilibili persistence observation binary.
#[derive(Debug, Clone, Parser)]
#[command(name = "browser_bilibili_persist_observe")]
struct BrowserBilibiliPersistObserveCli {
    /// Observation mode. `login` keeps a non-headless browser open for manual login; `capture`
    /// reuses the persisted cookies state file on a fresh session.
    #[arg(value_enum, default_value_t = BilibiliPersistMode::Capture)]
    mode: BilibiliPersistMode,
    /// Target URL captured by this observation run.
    #[arg(long, default_value = "https://www.bilibili.com/")]
    url: String,
    /// Run the browser in headless mode.
    #[arg(long, default_value_t = false)]
    headless: bool,
    /// Reuse the persisted cookies state file during `login` mode too.
    #[arg(long, default_value_t = false)]
    reuse_state: bool,
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

/// Run one Bilibili persistence observation and persist both snapshot forms.
///
/// # 示例
/// ```rust,no_run
/// # async fn demo() -> anyhow::Result<()> {
/// let cli = clap::Parser::parse_from(["browser_bilibili_persist_observe", "capture"]);
/// # let _ = cli;
/// # Ok(())
/// # }
/// ```
async fn run_bilibili_persistence(cli: &BrowserBilibiliPersistObserveCli) -> Result<()> {
    let captured_at = Utc::now();
    let run_dir = observation_root().join("runs").join(format!(
        "capture-{}-{}",
        captured_at.format("%Y%m%dT%H%M%S%.3fZ"),
        match cli.mode {
            BilibiliPersistMode::Login => "login",
            BilibiliPersistMode::Capture => "capture",
        }
    ));
    fs::create_dir_all(&run_dir).with_context(|| {
        format!(
            "failed to create Bilibili observation run dir {}",
            run_dir.display()
        )
    })?;
    fs::create_dir_all(cookies_state_root()).with_context(|| {
        format!(
            "failed to create Bilibili cookies state dir {}",
            cookies_state_root().display()
        )
    })?;

    let manager = BrowserSessionManager::new(build_manager_config(cli, &run_dir));
    let thread_id = "browser-bilibili-persist-observe";
    let open = manager
        .open(thread_id, BrowserOpenRequest::launch())
        .await?;
    let navigate = manager.navigate(thread_id, &cli.url).await?;

    if matches!(cli.mode, BilibiliPersistMode::Login) {
        wait_for_manual_confirmation()?;
    }

    let snapshot = manager.snapshot(thread_id, Some(200)).await?;
    let aria_snapshot = manager.aria_snapshot(thread_id).await?;
    let screenshot_path = run_dir.join("bilibili-homepage.png");
    let screenshot = manager
        .screenshot(thread_id, Some(&screenshot_path))
        .await?;
    let close = manager.close(thread_id).await?;
    let cookies_saved = close.exported_cookie_count.unwrap_or(0);
    let exported_path = close
        .auto_exported_path
        .clone()
        .unwrap_or_else(|| cookies_state_file().display().to_string());

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
            "mode": match cli.mode {
                BilibiliPersistMode::Login => "login",
                BilibiliPersistMode::Capture => "capture",
            },
            "headless": cli.headless,
            "target_url": cli.url,
            "final_url": navigate.url,
            "title": navigate.title,
            "captured_at": captured_at.to_rfc3339(),
            "cookies_loaded": open.cookies_loaded,
            "cookies_saved": cookies_saved,
            "cookies_state_file": cookies_state_file().display().to_string(),
            "exported_cookies_state_file": exported_path,
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
    println!("cookies_saved: {}", cookies_saved);
    println!("cookies_state_file: {}", cookies_state_file().display());
    println!("browser_snapshot: {}", browser_snapshot_path.display());
    println!("aria_snapshot: {}", aria_snapshot_path.display());
    println!("screenshot: {}", screenshot.path);

    Ok(())
}

/// Build the browser session manager config used by the Bilibili persistence observation binary.
fn build_manager_config(
    cli: &BrowserBilibiliPersistObserveCli,
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
            cookies_state_file: Some(cookies_state_file()),
            load_cookies_on_open: matches!(cli.mode, BilibiliPersistMode::Capture)
                || cli.reuse_state,
            save_cookies_on_close: true,
            ..Default::default()
        },
        artifact_root: run_dir.join("session-artifacts"),
    }
}

/// Return the workspace-local output directory for Bilibili observations.
fn observation_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("observation")
        .join("aria-snapshot")
        .join("bilibili-persist")
}

/// Return the workspace-local persistent cookies state directory.
fn cookies_state_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(".openjarvis")
        .join("browser")
        .join("bilibili-persist")
}

/// Return the cookies state file path used by the Bilibili persistence observation.
fn cookies_state_file() -> PathBuf {
    cookies_state_root().join("browser-cookies.json")
}

/// Wait for the operator to finish manual login inside the non-headless browser window.
fn wait_for_manual_confirmation() -> Result<()> {
    print!("登录完成后按回车保存 cookies 并采集快照: ");
    io::stdout()
        .flush()
        .context("failed to flush login prompt to stdout")?;
    let mut buffer = String::new();
    io::stdin()
        .read_line(&mut buffer)
        .context("failed to read manual confirmation from stdin")?;
    if buffer.trim() == ":abort" {
        bail!("manual login observation aborted by operator");
    }
    Ok(())
}

/// Main entrypoint for the Bilibili persistence observation binary.
#[tokio::main]
async fn main() -> Result<()> {
    let cli = BrowserBilibiliPersistObserveCli::parse();
    run_bilibili_persistence(&cli).await
}
