//! Standalone verification binary for the Bilibili search-to-video browser flow.

use anyhow::{Context, Result, bail};
use clap::Parser;
use openjarvis::agent::tool::browser::{
    BrowserProcessCommandSpec, BrowserRuntimeOptions, BrowserSessionManager,
    BrowserSessionManagerConfig, BrowserSnapshotElement, BrowserSnapshotResult,
    default_sidecar_script_path,
};
use std::{fs, path::PathBuf};

/// Command-line options for the Bilibili browser verification binary.
#[derive(Debug, Clone, Parser)]
#[command(name = "browser_bilibili_search_dump")]
struct BrowserBilibiliSearchDumpCli {
    /// Homepage used as the search entrypoint.
    #[arg(long, default_value = "https://www.bilibili.com")]
    home_url: String,
    /// Search keyword typed into the homepage search box.
    #[arg(long, default_value = "咒术回战")]
    keyword: String,
    /// Run the browser in headless mode.
    #[arg(long, default_value_t = false)]
    headless: bool,
    /// Optional root directory used to retain browser artifacts.
    #[arg(long)]
    output_dir: Option<PathBuf>,
    /// Override the Node.js executable used to launch the sidecar.
    #[arg(long, default_value = "node")]
    node_bin: String,
    /// Override the browser sidecar script path.
    #[arg(long)]
    script_path: Option<PathBuf>,
    /// Optional explicit Chrome executable path.
    #[arg(long)]
    chrome_path: Option<PathBuf>,
    /// Snapshot size used for the homepage search-box lookup.
    #[arg(long, default_value_t = 200)]
    home_max_elements: usize,
    /// Snapshot size used for the search-results video lookup.
    #[arg(long, default_value_t = 320)]
    results_max_elements: usize,
    /// Snapshot size used for the final video-page dump.
    #[arg(long, default_value_t = 180)]
    video_max_elements: usize,
}

/// Run the scenario-specific Bilibili browser verification flow.
///
/// # 示例
/// ```rust,no_run
/// # async fn demo() -> anyhow::Result<()> {
/// let cli = clap::Parser::parse_from([
///     "browser_bilibili_search_dump",
///     "--headless",
/// ]);
/// # let _ = cli;
/// # Ok(())
/// # }
/// ```
async fn run_bilibili_search_dump(cli: &BrowserBilibiliSearchDumpCli) -> Result<()> {
    let manager = BrowserSessionManager::new(build_manager_config(cli));
    let thread_id = "browser-bilibili-search-dump";

    let navigate = manager.navigate(thread_id, &cli.home_url).await?;
    println!("navigate: {}", navigate.url);
    println!("title: {}", navigate.title);

    let home_snapshot = manager
        .snapshot(thread_id, Some(clamp_snapshot_limit(cli.home_max_elements)))
        .await?;
    let search_box = find_search_box(&home_snapshot)?;
    println!(
        "search_box_ref: {} ({}/{})",
        search_box.reference, search_box.role, search_box.tag_name
    );

    let typed = manager
        .type_ref(thread_id, &search_box.reference, &cli.keyword, true)
        .await?;
    println!("search_url: {}", typed.url);
    println!("search_title: {}", typed.title);
    println!("opened_new_page: {}", typed.opened_new_page);

    let results_snapshot = manager
        .snapshot(
            thread_id,
            Some(clamp_snapshot_limit(cli.results_max_elements)),
        )
        .await?;
    let first_video = find_first_video_link(&results_snapshot)?;
    println!(
        "first_video_ref: {} ({}/{})",
        first_video.reference, first_video.role, first_video.tag_name
    );
    if let Some(href) = first_video.href.as_deref() {
        println!("first_video_href: {href}");
    }

    let clicked = manager.click_ref(thread_id, &first_video.reference).await?;
    println!("video_url: {}", clicked.url);
    println!("video_title: {}", clicked.title);
    println!("video_opened_new_page: {}", clicked.opened_new_page);

    let video_snapshot = manager
        .snapshot(
            thread_id,
            Some(clamp_snapshot_limit(cli.video_max_elements)),
        )
        .await?;
    println!("video snapshot:");
    println!("{}", video_snapshot.snapshot_text);

    let screenshot = manager.screenshot(thread_id, None).await?;
    let close = manager.close(thread_id).await?;

    let artifacts = close
        .artifacts
        .context("browser session did not preserve artifacts")?;
    let results_snapshot_path = artifacts.session_dir.join("search-results-snapshot.txt");
    let video_snapshot_path = artifacts.session_dir.join("video-page-snapshot.txt");
    write_snapshot_artifact(&results_snapshot_path, &results_snapshot)?;
    write_snapshot_artifact(&video_snapshot_path, &video_snapshot)?;

    println!("screenshot: {}", screenshot.path);
    println!("artifacts: {}", artifacts.session_dir.display());
    println!("results_snapshot_file: {}", results_snapshot_path.display());
    println!("video_snapshot_file: {}", video_snapshot_path.display());

    Ok(())
}

/// Build the browser session manager config used by the verification binary.
fn build_manager_config(cli: &BrowserBilibiliSearchDumpCli) -> BrowserSessionManagerConfig {
    let artifact_root = cli
        .output_dir
        .clone()
        .unwrap_or_else(|| std::env::temp_dir().join("openjarvis-browser-bilibili-search-dump"));
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
        artifact_root,
    }
}

/// Find the first usable homepage search box from a snapshot.
fn find_search_box(snapshot: &BrowserSnapshotResult) -> Result<BrowserSnapshotElement> {
    find_element(snapshot, "the homepage search box", |element| {
        element.role == "textbox" && element.tag_name == "input" && !element.disabled
    })
}

/// Find the first search-result link whose href points at a video page.
fn find_first_video_link(snapshot: &BrowserSnapshotResult) -> Result<BrowserSnapshotElement> {
    find_element(snapshot, "the first `/video/` result link", |element| {
        element.role == "link"
            && !element.disabled
            && element
                .href
                .as_deref()
                .map(|href| href.contains("/video/"))
                .unwrap_or(false)
    })
}

/// Find one snapshot element by predicate and return a cloned copy.
fn find_element<F>(
    snapshot: &BrowserSnapshotResult,
    description: &str,
    predicate: F,
) -> Result<BrowserSnapshotElement>
where
    F: Fn(&BrowserSnapshotElement) -> bool,
{
    snapshot
        .elements
        .iter()
        .find(|element| predicate(element))
        .cloned()
        .with_context(|| {
            format!(
                "failed to locate {description} in snapshot with {} elements",
                snapshot.elements.len()
            )
        })
}

/// Clamp one requested snapshot size into the sidecar-supported range.
fn clamp_snapshot_limit(limit: usize) -> usize {
    limit.clamp(1, 500)
}

/// Write one textual snapshot artifact into the preserved browser session directory.
fn write_snapshot_artifact(path: &PathBuf, snapshot: &BrowserSnapshotResult) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create snapshot artifact directory {}",
                parent.display()
            )
        })?;
    }
    fs::write(path, &snapshot.snapshot_text)
        .with_context(|| format!("failed to write snapshot artifact {}", path.display()))
}

/// Main entrypoint for the Bilibili browser verification binary.
#[tokio::main]
async fn main() -> Result<()> {
    let cli = BrowserBilibiliSearchDumpCli::parse();
    if cli.keyword.trim().is_empty() {
        bail!("`--keyword` must not be blank");
    }
    run_bilibili_search_dump(&cli).await
}
