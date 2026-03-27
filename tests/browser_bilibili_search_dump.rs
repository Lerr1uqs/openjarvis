use std::{path::Path, process::Command};

#[test]
fn browser_bilibili_search_dump_bin_reports_sidecar_spawn_errors() {
    // 验证场景: 独立验证 bin 应该能启动，并在 sidecar 无法拉起时返回明确错误。
    let output = Command::new(env!("CARGO_BIN_EXE_browser_bilibili_search_dump"))
        .arg("--headless")
        .arg("--node-bin")
        .arg("missing-browser-node")
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")))
        .output()
        .expect("browser_bilibili_search_dump binary should run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("failed to spawn browser sidecar executable"));
}
