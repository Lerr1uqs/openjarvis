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

#[test]
fn default_agent_hook_config_is_empty() {
    let config = AppConfig::default();

    assert!(config.agent_config().hook_config().is_empty());
}

#[test]
fn malformed_hook_config_with_unknown_event_is_rejected() {
    let path = temp_dir().join(format!(
        "openjarvis-hook-unknown-event-{}.yaml",
        Uuid::new_v4()
    ));
    fs::write(
        &path,
        r#"
agent:
  hook:
    not_a_real_event: ["echo", "hello"]
llm:
  provider: "mock"
"#,
    )
    .expect("temp config should be written");

    let error = AppConfig::from_path(&path).expect_err("unknown hook event should be rejected");
    fs::remove_file(&path).expect("temp config should be removed");
    let error_chain = format!("{error:#}");

    assert!(error_chain.contains("unknown field `not_a_real_event`"));
}

#[test]
fn malformed_hook_config_with_empty_command_is_rejected() {
    let path = temp_dir().join(format!(
        "openjarvis-hook-empty-command-{}.yaml",
        Uuid::new_v4()
    ));
    fs::write(
        &path,
        r#"
agent:
  hook:
    notification: []
llm:
  provider: "mock"
"#,
    )
    .expect("temp config should be written");

    let error = AppConfig::from_path(&path).expect_err("empty hook command should be rejected");
    fs::remove_file(&path).expect("temp config should be removed");
    let error_chain = format!("{error:#}");

    assert!(error_chain.contains("notification hook command must not be empty"));
}

#[test]
fn malformed_hook_config_with_blank_command_part_is_rejected() {
    let path = temp_dir().join(format!(
        "openjarvis-hook-blank-part-{}.yaml",
        Uuid::new_v4()
    ));
    fs::write(
        &path,
        r#"
agent:
  hook:
    notification: ["powershell", "   "]
llm:
  provider: "mock"
"#,
    )
    .expect("temp config should be written");

    let error =
        AppConfig::from_path(&path).expect_err("blank hook command part should be rejected");
    fs::remove_file(&path).expect("temp config should be removed");
    let error_chain = format!("{error:#}");

    assert!(error_chain.contains("notification hook command part at index 1 must not be blank"));
}

#[test]
fn malformed_hook_config_with_wrong_type_is_rejected() {
    let path = temp_dir().join(format!(
        "openjarvis-hook-wrong-type-{}.yaml",
        Uuid::new_v4()
    ));
    fs::write(
        &path,
        r#"
agent:
  hook: "invalid"
llm:
  provider: "mock"
"#,
    )
    .expect("temp config should be written");

    let error = AppConfig::from_path(&path).expect_err("invalid hook section should be rejected");
    fs::remove_file(&path).expect("temp config should be removed");
    let error_chain = format!("{error:#}");

    assert!(error_chain.contains("invalid type"));
}
