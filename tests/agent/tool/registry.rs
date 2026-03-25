use openjarvis::agent::ToolRegistry;

#[tokio::test]
async fn builtin_tools_can_be_registered_together() {
    let registry = ToolRegistry::with_skill_roots(Vec::new());
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");

    let definitions = registry.list().await;
    let mut names = definitions
        .into_iter()
        .map(|definition| definition.name)
        .collect::<Vec<_>>();
    names.sort();

    assert_eq!(names, vec!["bash", "edit", "read", "write"]);
}
