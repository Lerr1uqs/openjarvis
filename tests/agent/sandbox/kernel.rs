use super::{SandboxFixture, bubblewrap_yaml_with_compatibility};
use openjarvis::agent::SandboxCapabilityConfig;
use std::path::Path;

#[test]
fn sandbox_capability_config_parses_kernel_enforcement_profiles_and_compatibility() {
    // 测试场景: bubblewrap capability policy 应能解析 namespace、profile 映射和兼容性字段。
    let fixture = SandboxFixture::new("openjarvis-sandbox-kernel-config-parse");
    let config = SandboxCapabilityConfig::from_yaml_str(
        &bubblewrap_yaml_with_compatibility(Path::new("bwrap"), "readonly", true, true, true, 1),
        fixture.root(),
    )
    .expect("kernel enforcement capability config should parse");

    let bubblewrap = config.sandbox().bubblewrap();
    assert!(bubblewrap.namespaces().user());
    assert!(bubblewrap.namespaces().net());
    assert_eq!(bubblewrap.baseline_seccomp_profile(), "proxy-baseline-v1");
    assert_eq!(bubblewrap.proxy_landlock_profile(), "workspace-rpc");
    assert_eq!(bubblewrap.command_profiles().selected_profile(), "readonly");
    assert_eq!(
        bubblewrap
            .command_profiles()
            .profiles()
            .get("readonly")
            .expect("readonly profile should exist")
            .landlock_profile(),
        "command-readonly"
    );
    assert!(bubblewrap.compatibility().require_landlock());
    assert!(bubblewrap.compatibility().require_seccomp());
    assert!(bubblewrap.compatibility().strict());
    assert_eq!(bubblewrap.compatibility().min_landlock_abi(), 1);
}

#[test]
fn sandbox_capability_config_rejects_unknown_kernel_profiles() {
    // 测试场景: capability policy 引用了未知的 builtin profile 时应在解析阶段 fail closed。
    let fixture = SandboxFixture::new("openjarvis-sandbox-kernel-config-unknown-profile");
    let error = SandboxCapabilityConfig::from_yaml_str(
        r#"
sandbox:
  backend: "bubblewrap"
  bubblewrap:
    baseline_seccomp_profile: "proxy-baseline-v1"
    proxy_landlock_profile: "workspace-rpc"
    command_profiles:
      selected_profile: "default"
      profiles:
        default:
          landlock_profile: "command-default"
          seccomp_profile: "missing-profile"
"#,
        fixture.root(),
    )
    .expect_err("unknown seccomp profile should fail validation");

    let rendered = format!("{error:#}");
    assert!(rendered.contains("unknown sandbox seccomp profile"));
}

#[test]
fn sandbox_capability_config_rejects_invalid_min_landlock_abi() {
    // 测试场景: capability policy 指定非法的 Landlock ABI 时应明确报错，而不是静默降级。
    let fixture = SandboxFixture::new("openjarvis-sandbox-kernel-config-invalid-abi");
    let error = SandboxCapabilityConfig::from_yaml_str(
        &bubblewrap_yaml_with_compatibility(Path::new("bwrap"), "default", true, true, true, 7),
        fixture.root(),
    )
    .expect_err("invalid Landlock ABI should fail validation");

    let rendered = format!("{error:#}");
    assert!(rendered.contains("min_landlock_abi"));
}
