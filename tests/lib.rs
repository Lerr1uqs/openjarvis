use openjarvis::config::AppConfig;

#[test]
fn library_exports_core_modules() {
    let config = AppConfig::default();

    assert_eq!(config.llm_config().provider, "mock");
    assert!(config.agent_config().hook_config().is_empty());
}
