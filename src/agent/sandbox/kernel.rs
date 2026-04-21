//! Kernel enforcement plan compilation and Linux Landlock/Seccomp installers for bubblewrap.

use super::{
    BubblewrapCapabilityConfig, BubblewrapCommandProfileConfig, BubblewrapCompatibilityConfig,
};
use anyhow::{Context, Result, bail};
use landlock::{
    ABI, Access, AccessFs, CompatLevel, Compatible, PathBeneath, PathFd, RestrictionStatus,
    Ruleset, RulesetAttr, RulesetCreatedAttr, RulesetStatus,
};
use seccompiler::{SeccompAction, SeccompFilter, TargetArch};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    convert::TryInto,
    path::{Path, PathBuf},
};
use tracing::{debug, info, warn};

pub(crate) const DEFAULT_BASELINE_SECCOMP_PROFILE: &str = "proxy-baseline-v1";
pub(crate) const DEFAULT_PROXY_LANDLOCK_PROFILE: &str = "workspace-rpc";
pub(crate) const DEFAULT_COMMAND_PROFILE_NAME: &str = "default";
pub(crate) const DEFAULT_COMMAND_LANDLOCK_PROFILE: &str = "command-default";
pub(crate) const DEFAULT_COMMAND_SECCOMP_PROFILE: &str = "command-default-v1";
pub(crate) const COMMAND_READONLY_LANDLOCK_PROFILE: &str = "command-readonly";
pub(crate) const COMMAND_READONLY_SECCOMP_PROFILE: &str = "command-readonly-v1";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct KernelEnforcementCompatibilityPlan {
    require_landlock: bool,
    min_landlock_abi: u8,
    require_seccomp: bool,
    strict: bool,
}

impl KernelEnforcementCompatibilityPlan {
    pub(crate) fn min_landlock_abi(&self) -> u8 {
        self.min_landlock_abi
    }

    pub(crate) fn require_landlock(&self) -> bool {
        self.require_landlock
    }

    pub(crate) fn require_seccomp(&self) -> bool {
        self.require_seccomp
    }

    pub(crate) fn strict(&self) -> bool {
        self.strict
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct BubblewrapNamespacePlan {
    user: bool,
    ipc: bool,
    pid: bool,
    uts: bool,
    net: bool,
}

impl BubblewrapNamespacePlan {
    pub(crate) fn user(&self) -> bool {
        self.user
    }

    pub(crate) fn ipc(&self) -> bool {
        self.ipc
    }

    pub(crate) fn pid(&self) -> bool {
        self.pid
    }

    pub(crate) fn uts(&self) -> bool {
        self.uts
    }

    pub(crate) fn net(&self) -> bool {
        self.net
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct SandboxProxyEnforcementPlan {
    baseline_seccomp_profile: String,
    landlock_profile: String,
}

impl SandboxProxyEnforcementPlan {
    pub(crate) fn baseline_seccomp_profile(&self) -> &str {
        &self.baseline_seccomp_profile
    }

    pub(crate) fn landlock_profile(&self) -> &str {
        &self.landlock_profile
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct SandboxCommandChildProfilePlan {
    name: String,
    landlock_profile: String,
    seccomp_profile: String,
    compatibility: KernelEnforcementCompatibilityPlan,
}

impl SandboxCommandChildProfilePlan {
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn landlock_profile(&self) -> &str {
        &self.landlock_profile
    }

    pub(crate) fn seccomp_profile(&self) -> &str {
        &self.seccomp_profile
    }

    pub(crate) fn compatibility(&self) -> &KernelEnforcementCompatibilityPlan {
        &self.compatibility
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct SandboxKernelEnforcementPlan {
    namespace: BubblewrapNamespacePlan,
    compatibility: KernelEnforcementCompatibilityPlan,
    proxy: SandboxProxyEnforcementPlan,
    default_command_profile: String,
    command_profiles: BTreeMap<String, SandboxCommandChildProfilePlan>,
}

impl SandboxKernelEnforcementPlan {
    pub(crate) fn namespace(&self) -> &BubblewrapNamespacePlan {
        &self.namespace
    }

    pub(crate) fn compatibility(&self) -> &KernelEnforcementCompatibilityPlan {
        &self.compatibility
    }

    pub(crate) fn default_command_profile(&self) -> &str {
        &self.default_command_profile
    }

    pub(crate) fn command_profile(
        &self,
        name: Option<&str>,
    ) -> Result<&SandboxCommandChildProfilePlan> {
        let selected = name.unwrap_or(self.default_command_profile.as_str());
        self.command_profiles
            .get(selected)
            .ok_or_else(|| anyhow::anyhow!("unknown sandbox command profile `{selected}`"))
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
enum LandlockPathMode {
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Clone, Copy)]
enum LandlockPathTarget {
    Root,
    Workspace,
    Tmp,
    DevPts,
    DevPtmx,
}

#[derive(Debug, Clone, Copy)]
struct LandlockBuiltinRule {
    target: LandlockPathTarget,
    mode: LandlockPathMode,
}

#[derive(Debug, Clone, Copy)]
struct LandlockBuiltinProfile {
    rules: &'static [LandlockBuiltinRule],
}

/// Validate bubblewrap kernel-enforcement configuration semantics before runtime compilation.
///
/// # 示例
/// ```rust
/// use openjarvis::agent::SandboxCapabilityConfig;
///
/// let config = SandboxCapabilityConfig::from_yaml_str(
///     "sandbox:\n  backend: bubblewrap\n",
///     "/tmp/openjarvis-kernel-config",
/// )
/// .expect("bubblewrap capability config should parse");
///
/// assert_eq!(
///     config.sandbox().bubblewrap().command_profiles().selected_profile(),
///     "default"
/// );
/// ```
pub(crate) fn validate_kernel_enforcement_config(
    config: &BubblewrapCapabilityConfig,
) -> Result<()> {
    validate_profile_field(
        "sandbox.bubblewrap.baseline_seccomp_profile",
        config.baseline_seccomp_profile(),
    )?;
    validate_profile_field(
        "sandbox.bubblewrap.proxy_landlock_profile",
        config.proxy_landlock_profile(),
    )?;
    validate_builtin_seccomp_profile(config.baseline_seccomp_profile())?;
    validate_builtin_landlock_profile(config.proxy_landlock_profile())?;
    validate_compatibility(config.compatibility())?;

    let command_profiles = config.command_profiles();
    if command_profiles.profiles().is_empty() {
        bail!("sandbox.bubblewrap.command_profiles.profiles must not be empty");
    }
    validate_profile_field(
        "sandbox.bubblewrap.command_profiles.selected_profile",
        command_profiles.selected_profile(),
    )?;
    if !command_profiles
        .profiles()
        .contains_key(command_profiles.selected_profile())
    {
        bail!(
            "sandbox.bubblewrap.command_profiles.selected_profile `{}` is not declared",
            command_profiles.selected_profile()
        );
    }

    let mut logical_names = BTreeSet::new();
    for (logical_name, profile) in command_profiles.profiles() {
        if !logical_names.insert(logical_name.clone()) {
            bail!("duplicate sandbox command profile `{logical_name}`");
        }
        validate_command_profile(logical_name, profile)?;
    }
    Ok(())
}

/// Compile one bubblewrap kernel-enforcement plan from the resolved capability policy.
///
/// # 示例
/// ```rust
/// use openjarvis::agent::SandboxCapabilityConfig;
///
/// let config = SandboxCapabilityConfig::from_yaml_str(
///     "sandbox:\n  backend: bubblewrap\n",
///     "/tmp/openjarvis-kernel-plan",
/// )
/// .expect("bubblewrap capability config should parse");
///
/// assert_eq!(
///     config.sandbox().bubblewrap().baseline_seccomp_profile(),
///     "proxy-baseline-v1"
/// );
/// ```
pub(crate) fn compile_kernel_enforcement_plan(
    config: &BubblewrapCapabilityConfig,
) -> Result<SandboxKernelEnforcementPlan> {
    validate_kernel_enforcement_config(config)?;
    let compatibility = KernelEnforcementCompatibilityPlan {
        require_landlock: config.compatibility().require_landlock(),
        min_landlock_abi: config.compatibility().min_landlock_abi(),
        require_seccomp: config.compatibility().require_seccomp(),
        strict: config.compatibility().strict(),
    };

    if compatibility.require_landlock() || compatibility.strict() {
        probe_landlock_support(compatibility.min_landlock_abi()).with_context(|| {
            format!(
                "bubblewrap kernel enforcement requires Landlock ABI >= {}",
                compatibility.min_landlock_abi()
            )
        })?;
    }
    if compatibility.require_seccomp() || compatibility.strict() {
        seccomp_target_arch()
            .context("bubblewrap kernel enforcement requires a supported seccomp target arch")?;
        compile_seccomp_program(config.baseline_seccomp_profile()).with_context(|| {
            format!(
                "failed to compile baseline seccomp profile `{}`",
                config.baseline_seccomp_profile()
            )
        })?;
        for (logical_name, profile) in config.command_profiles().profiles() {
            compile_seccomp_program(profile.seccomp_profile()).with_context(|| {
                format!(
                    "failed to compile command seccomp profile `{}` for `{logical_name}`",
                    profile.seccomp_profile()
                )
            })?;
        }
    }

    let command_profiles = config
        .command_profiles()
        .profiles()
        .iter()
        .map(|(logical_name, profile)| {
            Ok((
                logical_name.clone(),
                SandboxCommandChildProfilePlan {
                    name: logical_name.clone(),
                    landlock_profile: profile.landlock_profile().to_string(),
                    seccomp_profile: profile.seccomp_profile().to_string(),
                    compatibility: compatibility.clone(),
                },
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;

    let plan = SandboxKernelEnforcementPlan {
        namespace: BubblewrapNamespacePlan {
            user: config.namespaces().user(),
            ipc: config.namespaces().ipc(),
            pid: config.namespaces().pid(),
            uts: config.namespaces().uts(),
            net: config.namespaces().net(),
        },
        compatibility,
        proxy: SandboxProxyEnforcementPlan {
            baseline_seccomp_profile: config.baseline_seccomp_profile().to_string(),
            landlock_profile: config.proxy_landlock_profile().to_string(),
        },
        default_command_profile: config.command_profiles().selected_profile().to_string(),
        command_profiles,
    };

    info!(
        proxy_landlock_profile = plan.proxy.landlock_profile(),
        baseline_seccomp_profile = plan.proxy.baseline_seccomp_profile(),
        default_command_profile = plan.default_command_profile(),
        "compiled sandbox kernel enforcement plan"
    );
    Ok(plan)
}

pub(crate) fn install_proxy_enforcement(
    plan: &SandboxKernelEnforcementPlan,
    workspace_root: &Path,
) -> Result<()> {
    ensure_no_new_privs().context("failed to enable no_new_privs for sandbox proxy")?;
    info!(
        workspace_root = %workspace_root.display(),
        landlock_profile = plan.proxy.landlock_profile(),
        seccomp_profile = plan.proxy.baseline_seccomp_profile(),
        "installing sandbox proxy kernel enforcement"
    );
    maybe_install_landlock(
        plan.proxy.landlock_profile(),
        workspace_root,
        plan.compatibility(),
        "proxy",
    )?;
    maybe_install_seccomp(
        plan.proxy.baseline_seccomp_profile(),
        plan.compatibility(),
        "proxy",
    )?;
    Ok(())
}

pub(crate) fn install_command_profile(
    profile: &SandboxCommandChildProfilePlan,
    workspace_root: &Path,
) -> Result<()> {
    ensure_no_new_privs().context("failed to enable no_new_privs for sandbox command child")?;
    info!(
        workspace_root = %workspace_root.display(),
        profile_name = profile.name(),
        landlock_profile = profile.landlock_profile(),
        seccomp_profile = profile.seccomp_profile(),
        "installing sandbox command-child kernel enforcement"
    );
    maybe_install_landlock(
        profile.landlock_profile(),
        workspace_root,
        profile.compatibility(),
        "command child",
    )?;
    maybe_install_seccomp(
        profile.seccomp_profile(),
        profile.compatibility(),
        "command child",
    )?;
    Ok(())
}

fn validate_profile_field(field_name: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("{field_name} must not be blank");
    }
    Ok(())
}

fn validate_command_profile(
    logical_name: &str,
    profile: &BubblewrapCommandProfileConfig,
) -> Result<()> {
    if logical_name.trim().is_empty() {
        bail!("sandbox.bubblewrap.command_profiles.profiles keys must not be blank");
    }
    validate_profile_field(
        &format!("sandbox.bubblewrap.command_profiles.profiles.{logical_name}.landlock_profile"),
        profile.landlock_profile(),
    )?;
    validate_profile_field(
        &format!("sandbox.bubblewrap.command_profiles.profiles.{logical_name}.seccomp_profile"),
        profile.seccomp_profile(),
    )?;
    validate_builtin_landlock_profile(profile.landlock_profile())?;
    validate_builtin_seccomp_profile(profile.seccomp_profile())?;
    Ok(())
}

fn validate_compatibility(config: &BubblewrapCompatibilityConfig) -> Result<()> {
    if config.min_landlock_abi() == 0 || config.min_landlock_abi() > 6 {
        bail!("sandbox.bubblewrap.compatibility.min_landlock_abi must be between 1 and 6");
    }
    Ok(())
}

fn validate_builtin_landlock_profile(profile_name: &str) -> Result<()> {
    if builtin_landlock_profile(profile_name).is_none() {
        bail!("unknown sandbox landlock profile `{profile_name}`");
    }
    Ok(())
}

fn validate_builtin_seccomp_profile(profile_name: &str) -> Result<()> {
    if builtin_seccomp_syscalls(profile_name).is_none() {
        bail!("unknown sandbox seccomp profile `{profile_name}`");
    }
    Ok(())
}

fn probe_landlock_support(min_landlock_abi: u8) -> Result<()> {
    let abi = abi_from_u8(min_landlock_abi)?;
    Ruleset::default()
        .set_compatibility(CompatLevel::HardRequirement)
        .handle_access(AccessFs::from_all(abi))
        .context("failed to prepare Landlock ruleset")?
        .create()
        .context("failed to create Landlock ruleset")?;
    debug!(min_landlock_abi, "validated host Landlock compatibility");
    Ok(())
}

fn abi_from_u8(value: u8) -> Result<ABI> {
    match value {
        1 => Ok(ABI::V1),
        2 => Ok(ABI::V2),
        3 => Ok(ABI::V3),
        4 => Ok(ABI::V4),
        5 => Ok(ABI::V5),
        6 => Ok(ABI::V6),
        _ => bail!("unsupported Landlock ABI `{value}`"),
    }
}

fn maybe_install_landlock(
    profile_name: &str,
    workspace_root: &Path,
    compatibility: &KernelEnforcementCompatibilityPlan,
    subject: &str,
) -> Result<()> {
    if let Err(error) = install_landlock_profile(
        profile_name,
        workspace_root,
        compatibility.min_landlock_abi(),
    ) {
        if compatibility.require_landlock() || compatibility.strict() {
            return Err(error).with_context(|| {
                format!(
                    "{subject} Landlock enforcement `{profile_name}` is required but could not be installed"
                )
            });
        }
        warn!(
            subject,
            profile_name,
            error = %error,
            "landlock enforcement was not installed; continuing because the profile is optional"
        );
    }
    Ok(())
}

fn maybe_install_seccomp(
    profile_name: &str,
    compatibility: &KernelEnforcementCompatibilityPlan,
    subject: &str,
) -> Result<()> {
    if let Err(error) = install_seccomp_profile(profile_name) {
        if compatibility.require_seccomp() || compatibility.strict() {
            return Err(error).with_context(|| {
                format!(
                    "{subject} seccomp enforcement `{profile_name}` is required but could not be installed"
                )
            });
        }
        warn!(
            subject,
            profile_name,
            error = %error,
            "seccomp enforcement was not installed; continuing because the profile is optional"
        );
    }
    Ok(())
}

fn install_landlock_profile(
    profile_name: &str,
    workspace_root: &Path,
    min_landlock_abi: u8,
) -> Result<RestrictionStatus> {
    let abi = abi_from_u8(min_landlock_abi)?;
    let profile = builtin_landlock_profile(profile_name)
        .ok_or_else(|| anyhow::anyhow!("unknown sandbox landlock profile `{profile_name}`"))?;
    let ruleset = Ruleset::default()
        .set_compatibility(CompatLevel::HardRequirement)
        .handle_access(AccessFs::from_all(abi))
        .context("failed to create handled Landlock access set")?;
    let mut created = ruleset
        .create()
        .context("failed to create Landlock ruleset")?;

    for rule in profile.rules {
        let target = resolve_landlock_target(rule.target, workspace_root);
        if !target.exists() {
            bail!(
                "landlock profile `{profile_name}` requires missing path `{}`",
                target.display()
            );
        }
        let access = landlock_accesses(rule.mode, abi, &target)?;
        created = created
            .add_rule(PathBeneath::new(PathFd::new(&target)?, access))
            .with_context(|| {
                format!(
                    "failed to add Landlock rule for profile `{profile_name}` target `{}`",
                    target.display()
                )
            })?;
    }

    let status = created
        .restrict_self()
        .with_context(|| format!("failed to restrict Landlock profile `{profile_name}`"))?;
    if status.ruleset == RulesetStatus::NotEnforced {
        bail!("Landlock profile `{profile_name}` was not enforced");
    }
    debug!(
        profile_name,
        landlock_status = ?status.landlock,
        ruleset_status = ?status.ruleset,
        "installed sandbox landlock profile"
    );
    Ok(status)
}

fn landlock_accesses(
    mode: LandlockPathMode,
    abi: ABI,
    target: &Path,
) -> Result<landlock::BitFlags<AccessFs>> {
    let metadata = target
        .metadata()
        .with_context(|| format!("failed to stat Landlock target `{}`", target.display()))?;
    let accesses = match mode {
        LandlockPathMode::ReadOnly => AccessFs::from_read(abi),
        LandlockPathMode::ReadWrite => AccessFs::from_all(abi),
    };
    if metadata.is_dir() {
        Ok(accesses)
    } else {
        Ok(accesses & AccessFs::from_file(abi))
    }
}

fn resolve_landlock_target(target: LandlockPathTarget, workspace_root: &Path) -> PathBuf {
    match target {
        LandlockPathTarget::Root => PathBuf::from("/"),
        LandlockPathTarget::Workspace => workspace_root.to_path_buf(),
        LandlockPathTarget::Tmp => PathBuf::from("/tmp"),
        LandlockPathTarget::DevPts => PathBuf::from("/dev/pts"),
        LandlockPathTarget::DevPtmx => PathBuf::from("/dev/ptmx"),
    }
}

fn builtin_landlock_profile(profile_name: &str) -> Option<LandlockBuiltinProfile> {
    const WORKSPACE_RW_RULES: &[LandlockBuiltinRule] = &[
        LandlockBuiltinRule {
            target: LandlockPathTarget::Root,
            mode: LandlockPathMode::ReadOnly,
        },
        LandlockBuiltinRule {
            target: LandlockPathTarget::Workspace,
            mode: LandlockPathMode::ReadWrite,
        },
        LandlockBuiltinRule {
            target: LandlockPathTarget::Tmp,
            mode: LandlockPathMode::ReadWrite,
        },
        LandlockBuiltinRule {
            target: LandlockPathTarget::DevPts,
            mode: LandlockPathMode::ReadWrite,
        },
        LandlockBuiltinRule {
            target: LandlockPathTarget::DevPtmx,
            mode: LandlockPathMode::ReadWrite,
        },
    ];
    const WORKSPACE_READONLY_RULES: &[LandlockBuiltinRule] = &[
        LandlockBuiltinRule {
            target: LandlockPathTarget::Root,
            mode: LandlockPathMode::ReadOnly,
        },
        LandlockBuiltinRule {
            target: LandlockPathTarget::Workspace,
            mode: LandlockPathMode::ReadOnly,
        },
        LandlockBuiltinRule {
            target: LandlockPathTarget::Tmp,
            mode: LandlockPathMode::ReadWrite,
        },
        LandlockBuiltinRule {
            target: LandlockPathTarget::DevPts,
            mode: LandlockPathMode::ReadWrite,
        },
        LandlockBuiltinRule {
            target: LandlockPathTarget::DevPtmx,
            mode: LandlockPathMode::ReadWrite,
        },
    ];

    match profile_name {
        DEFAULT_PROXY_LANDLOCK_PROFILE | DEFAULT_COMMAND_LANDLOCK_PROFILE => {
            Some(LandlockBuiltinProfile {
                rules: WORKSPACE_RW_RULES,
            })
        }
        COMMAND_READONLY_LANDLOCK_PROFILE => Some(LandlockBuiltinProfile {
            rules: WORKSPACE_READONLY_RULES,
        }),
        _ => None,
    }
}

fn install_seccomp_profile(profile_name: &str) -> Result<()> {
    let program = compile_seccomp_program(profile_name)
        .with_context(|| format!("failed to compile seccomp profile `{profile_name}`"))?;
    seccompiler::apply_filter(&program)
        .with_context(|| format!("failed to apply seccomp profile `{profile_name}`"))?;
    debug!(profile_name, "installed sandbox seccomp profile");
    Ok(())
}

fn compile_seccomp_program(profile_name: &str) -> Result<seccompiler::BpfProgram> {
    let target_arch = seccomp_target_arch()?;
    let rules = builtin_seccomp_syscalls(profile_name)
        .ok_or_else(|| anyhow::anyhow!("unknown sandbox seccomp profile `{profile_name}`"))?;
    let rule_map = rules
        .iter()
        .map(|syscall| {
            Ok((
                syscall_number(syscall).with_context(|| {
                    format!(
                        "failed to resolve syscall `{syscall}` for seccomp profile `{profile_name}`"
                    )
                })?,
                Vec::new(),
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let filter = SeccompFilter::new(
        rule_map,
        SeccompAction::Allow,
        SeccompAction::Errno(libc::EPERM as u32),
        target_arch,
    )
    .with_context(|| format!("failed to build seccomp filter `{profile_name}`"))?;
    filter
        .try_into()
        .with_context(|| format!("failed to compile seccomp filter `{profile_name}`"))
}

fn seccomp_target_arch() -> Result<TargetArch> {
    TargetArch::try_from(std::env::consts::ARCH).context("unsupported seccomp target arch")
}

fn builtin_seccomp_syscalls(profile_name: &str) -> Option<&'static [&'static str]> {
    const BASELINE_DENYLIST: &[&str] = &[
        "mount",
        "umount2",
        "pivot_root",
        "setns",
        "unshare",
        "bpf",
        "ptrace",
        "init_module",
        "finit_module",
        "delete_module",
        "userfaultfd",
        "keyctl",
        "kexec_load",
        "perf_event_open",
        "process_vm_readv",
        "process_vm_writev",
        "reboot",
        "swapon",
        "swapoff",
    ];

    match profile_name {
        DEFAULT_BASELINE_SECCOMP_PROFILE
        | DEFAULT_COMMAND_SECCOMP_PROFILE
        | COMMAND_READONLY_SECCOMP_PROFILE => Some(BASELINE_DENYLIST),
        _ => None,
    }
}

fn syscall_number(name: &str) -> Result<i64> {
    let value = match name {
        "mount" => libc::SYS_mount,
        "umount2" => libc::SYS_umount2,
        "pivot_root" => libc::SYS_pivot_root,
        "setns" => libc::SYS_setns,
        "unshare" => libc::SYS_unshare,
        "bpf" => libc::SYS_bpf,
        "ptrace" => libc::SYS_ptrace,
        "init_module" => libc::SYS_init_module,
        "finit_module" => libc::SYS_finit_module,
        "delete_module" => libc::SYS_delete_module,
        "userfaultfd" => libc::SYS_userfaultfd,
        "keyctl" => libc::SYS_keyctl,
        "kexec_load" => libc::SYS_kexec_load,
        "perf_event_open" => libc::SYS_perf_event_open,
        "process_vm_readv" => libc::SYS_process_vm_readv,
        "process_vm_writev" => libc::SYS_process_vm_writev,
        "reboot" => libc::SYS_reboot,
        "swapon" => libc::SYS_swapon,
        "swapoff" => libc::SYS_swapoff,
        other => bail!("unknown seccomp syscall `{other}`"),
    };
    Ok(value)
}

fn ensure_no_new_privs() -> Result<()> {
    let status = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
    if status != 0 {
        bail!(
            "prctl(PR_SET_NO_NEW_PRIVS) failed: {}",
            std::io::Error::last_os_error()
        );
    }
    Ok(())
}
