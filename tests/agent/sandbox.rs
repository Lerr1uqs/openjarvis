use openjarvis::agent::DummySandboxContainer;

#[test]
fn dummy_sandbox_reports_placeholder_status() {
    let sandbox = DummySandboxContainer::new();

    assert_eq!(sandbox.kind(), "dummy");
    assert!(sandbox.is_placeholder());
}

#[test]
fn dummy_sandbox_default_matches_new() {
    let from_new = DummySandboxContainer::new();
    let from_default = DummySandboxContainer::default();

    assert_eq!(from_new, from_default);
}
