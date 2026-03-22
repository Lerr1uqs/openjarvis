use openjarvis::config::{AppConfig, DEFAULT_ASSISTANT_SYSTEM_PROMPT};
use std::{env::temp_dir, fs};
use uuid::Uuid;

#[test]
fn missing_config_path_returns_default_config() {
    let path = temp_dir().join(format!("openjarvis-missing-{}.yaml", Uuid::new_v4()));
    let config = AppConfig::from_path(&path).expect("missing path should fall back to defaults");

    assert_eq!(config.llm_config().provider, "mock");
    assert_eq!(
        config.channel_config().feishu_config().mode,
        "long_connection"
    );
}

#[test]
fn yaml_config_can_be_loaded_from_path() {
    let path = temp_dir().join(format!("openjarvis-config-{}.yaml", Uuid::new_v4()));
    fs::write(
        &path,
        r#"
feishu:
  mode: ""
llm:
  provider: "mock_llm"
  mock_response: "pong"
"#,
    )
    .expect("temp config should be written");

    let config = AppConfig::from_path(&path).expect("yaml config should parse");
    fs::remove_file(&path).expect("temp config should be removed");

    assert_eq!(config.channel_config().feishu_config().mode, "");
    assert_eq!(config.llm_config().provider, "mock_llm");
    assert_eq!(config.llm_config().mock_response, "pong");
}

#[test]
fn default_assistant_system_prompt_is_not_empty() {
    assert!(!DEFAULT_ASSISTANT_SYSTEM_PROMPT.trim().is_empty());
}
