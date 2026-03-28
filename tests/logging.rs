use openjarvis::logging::{init_tracing, load_logging_config_from_path};
use std::{
    env::temp_dir,
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

struct LoggingFixture {
    root: PathBuf,
    config_path: PathBuf,
}

impl LoggingFixture {
    fn new(prefix: &str) -> Self {
        let root = temp_dir().join(format!("{prefix}-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("fixture root should be created");
        let config_path = root.join("config.yaml");
        Self { root, config_path }
    }

    fn config_path(&self) -> &Path {
        &self.config_path
    }

    fn write_yaml(&self, yaml: &str) {
        fs::write(&self.config_path, yaml).expect("fixture yaml should be written");
    }
}

impl Drop for LoggingFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn logging_module_bootstrap_initializes_local_file_sink() {
    let fixture = LoggingFixture::new("openjarvis-logging-module");
    fixture.write_yaml(
        r#"
logging:
  level: "info"
  stderr: false
  file:
    enabled: true
    directory: "runtime-logs"
    rotation: "never"
    filename_prefix: "openjarvis-module"
    filename_suffix: "log"
    max_files: 1
"#,
    );

    let logging_config = load_logging_config_from_path(fixture.config_path())
        .expect("logging config should bootstrap");
    let expected_directory = fixture.root.join("runtime-logs");
    assert_eq!(
        logging_config.file_config().directory(),
        expected_directory.as_path()
    );

    let logging_guards = init_tracing(&logging_config).expect("tracing should initialize");
    // 验证场景: 日志模块初始化后，业务日志应写入本地文件而不是只停留在 stderr。
    tracing::info!("logging module integration smoke");
    drop(logging_guards);

    let log_entries = fs::read_dir(&expected_directory)
        .expect("log directory should exist")
        .collect::<Result<Vec<_>, _>>()
        .expect("log directory entries should be readable");
    assert_eq!(log_entries.len(), 1);

    let log_output =
        fs::read_to_string(log_entries[0].path()).expect("local log file should be readable");
    assert!(log_output.contains("tracing initialized"));
    assert!(log_output.contains("logging module integration smoke"));
}
