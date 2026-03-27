mod protocol;
mod service;
mod session;
mod tool;

use openjarvis::agent::tool::browser::{
    BrowserProcessCommandSpec, BrowserRuntimeOptions, BrowserSessionManagerConfig,
    BrowserSidecarServiceConfig,
};
use std::{env::temp_dir, fs, path::PathBuf};
use uuid::Uuid;

pub(super) struct BrowserFixture {
    root: PathBuf,
}

impl BrowserFixture {
    pub(super) fn new(prefix: &str) -> Self {
        let root = temp_dir().join(format!("{prefix}-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("browser fixture root should exist");
        Self { root }
    }

    pub(super) fn root(&self) -> &PathBuf {
        &self.root
    }

    pub(super) fn service_config(&self, keep_artifacts: bool) -> BrowserSidecarServiceConfig {
        let session_root = self.root.join("service-session");
        let user_data_dir = session_root.join("user-data");
        fs::create_dir_all(&user_data_dir).expect("service user data dir should exist");
        BrowserSidecarServiceConfig::new(
            mock_process_spec(),
            BrowserRuntimeOptions {
                headless: true,
                keep_artifacts,
                ..Default::default()
            },
            session_root,
            user_data_dir,
        )
    }

    pub(super) fn manager_config(&self, keep_artifacts: bool) -> BrowserSessionManagerConfig {
        BrowserSessionManagerConfig {
            process: mock_process_spec(),
            runtime: BrowserRuntimeOptions {
                headless: true,
                keep_artifacts,
                ..Default::default()
            },
            artifact_root: self.root.join("manager-artifacts"),
        }
    }
}

impl Drop for BrowserFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

pub(super) fn mock_process_spec() -> BrowserProcessCommandSpec {
    BrowserProcessCommandSpec {
        executable: env!("CARGO_BIN_EXE_openjarvis").to_string(),
        args: vec!["internal-browser".to_string(), "mock-sidecar".to_string()],
        env: Default::default(),
    }
}
