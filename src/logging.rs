//! Tracing bootstrap and local persistent log initialization.

use crate::config::{FileLoggingConfig, LogRotation, LoggingConfig};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::{env, fs, path::Path};
use tracing::info;
use tracing_appender::{
    non_blocking::WorkerGuard,
    rolling::{RollingFileAppender, Rotation},
};
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

/// Hold tracing writer guards so non-blocking file logging can flush on shutdown.
#[derive(Debug, Default)]
pub struct LoggingGuards {
    _file_guard: Option<WorkerGuard>,
}

impl LoggingGuards {
    fn new(file_guard: Option<WorkerGuard>) -> Self {
        Self {
            _file_guard: file_guard,
        }
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct LoggingBootstrapDocument {
    logging: LoggingConfig,
}

/// Bootstrap tracing from `OPENJARVIS_CONFIG` or `config.yaml`.
///
/// # 示例
/// ```no_run
/// use openjarvis::logging::init_tracing_from_default_config;
///
/// let _guards = init_tracing_from_default_config().expect("logging should initialize");
/// ```
pub fn init_tracing_from_default_config() -> Result<LoggingGuards> {
    let config_path = env::var("OPENJARVIS_CONFIG").unwrap_or_else(|_| "config.yaml".to_string());
    let logging_config = load_logging_config_from_path(&config_path)?;
    init_tracing(&logging_config)
}

/// Load only the `logging` section from one YAML file.
///
/// Relative file-log directories are resolved against the YAML file location.
///
/// # 示例
/// ```rust
/// use openjarvis::logging::load_logging_config_from_path;
///
/// let logging = load_logging_config_from_path("missing-config.yaml")
///     .expect("missing config should fall back to defaults");
/// assert!(logging.file_config().enabled());
/// ```
pub fn load_logging_config_from_path(path: impl AsRef<Path>) -> Result<LoggingConfig> {
    let path = path.as_ref();
    if !path.exists() {
        return Ok(LoggingConfig::default());
    }

    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read config file {}", path.display()))?;
    let mut document = serde_yaml::from_str::<LoggingBootstrapDocument>(&raw)
        .with_context(|| format!("failed to parse config file {}", path.display()))?;
    document.logging.resolve_paths(path);
    document
        .logging
        .validate()
        .with_context(|| format!("failed to validate config file {}", path.display()))?;
    Ok(document.logging)
}

/// Install tracing with stderr output and an optional rolling file sink.
///
/// # 示例
/// ```no_run
/// use openjarvis::{config::LoggingConfig, logging::init_tracing};
///
/// let _guards = init_tracing(&LoggingConfig::default()).expect("logging should initialize");
/// ```
pub fn init_tracing(logging_config: &LoggingConfig) -> Result<LoggingGuards> {
    logging_config.validate()?;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(logging_config.level_filter().trim()));
    let stderr_layer = logging_config.stderr_enabled().then(|| {
        fmt::layer()
            .with_target(false)
            .with_file(true)
            .with_line_number(true)
            .with_thread_ids(true)
            .with_thread_names(true)
            .with_ansi(logging_config.stderr_ansi())
            .with_writer(std::io::stderr)
    });
    let (file_layer, file_guard) = if logging_config.file_config().enabled() {
        let file_appender = build_file_appender(logging_config.file_config())?;
        let (writer, guard) = tracing_appender::non_blocking(file_appender);
        let layer = fmt::layer()
            .with_target(false)
            .with_file(true)
            .with_line_number(true)
            .with_thread_ids(true)
            .with_thread_names(true)
            .with_ansi(false)
            .with_writer(writer);
        (Some(layer), Some(guard))
    } else {
        (None, None)
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(stderr_layer)
        .with(file_layer)
        .try_init()
        .context("failed to install tracing subscriber")?;

    info!(
        level = logging_config.level_filter(),
        stderr = logging_config.stderr_enabled(),
        file_enabled = logging_config.file_config().enabled(),
        file_directory = %logging_config.file_config().directory().display(),
        rotation = %logging_config.file_config().rotation(),
        max_files = logging_config.file_config().max_files(),
        "tracing initialized"
    );

    Ok(LoggingGuards::new(file_guard))
}

fn build_file_appender(file_config: &FileLoggingConfig) -> Result<RollingFileAppender> {
    fs::create_dir_all(file_config.directory()).with_context(|| {
        format!(
            "failed to create log directory {}",
            file_config.directory().display()
        )
    })?;

    RollingFileAppender::builder()
        .rotation(to_rotation(file_config.rotation()))
        .filename_prefix(file_config.filename_prefix())
        .filename_suffix(file_config.filename_suffix())
        .max_log_files(file_config.max_files())
        .build(file_config.directory())
        .with_context(|| {
            format!(
                "failed to initialize file logging under {}",
                file_config.directory().display()
            )
        })
}

fn to_rotation(rotation: LogRotation) -> Rotation {
    match rotation {
        LogRotation::Minutely => Rotation::MINUTELY,
        LogRotation::Hourly => Rotation::HOURLY,
        LogRotation::Daily => Rotation::DAILY,
        LogRotation::Never => Rotation::NEVER,
    }
}
