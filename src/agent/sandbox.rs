//! Sandbox backends, capability policy loading, and internal JSON-RPC proxy helpers.

mod kernel;

use super::tool::command::{
    CommandExecHelperSpec, CommandExecutionRequest, CommandExecutionResult, CommandLaunchOptions,
    CommandSessionManager, CommandTaskSnapshot, CommandWriteRequest,
};
use crate::cli::{InternalSandboxCommand, InternalSandboxExecutorKind};
use anyhow::{Context, Result, bail};
use kernel::{
    DEFAULT_BASELINE_SECCOMP_PROFILE, DEFAULT_COMMAND_LANDLOCK_PROFILE,
    DEFAULT_COMMAND_PROFILE_NAME, DEFAULT_COMMAND_SECCOMP_PROFILE, DEFAULT_PROXY_LANDLOCK_PROFILE,
    SandboxCommandChildProfilePlan, SandboxKernelEnforcementPlan, compile_kernel_enforcement_plan,
    install_executor_landlock, install_final_command_seccomp, install_proxy_landlock,
    install_proxy_seccomp, validate_kernel_enforcement_config,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::{
    collections::{BTreeMap, HashMap},
    env,
    ffi::OsString,
    fs,
    io::{BufRead, BufReader, BufWriter, Read, Write},
    path::{Component, Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{Arc, Mutex},
};
use tracing::{debug, info, warn};

/// Default workspace-relative location of the global sandbox capability file.
pub const DEFAULT_CAPABILITIES_CONFIG_PATH: &str = "config/capabilities.yaml";

const DEFAULT_WORKSPACE_SYNC_DIR: &str = ".";
const SANDBOX_WORKSPACE_MOUNT: &str = "/workspace";
const SANDBOX_TMP_DIR: &str = "/tmp";
#[cfg(unix)]
const SANDBOX_EXECUTOR_POLICY_FD: RawFd = 3;

/// Stable sandbox backend identifiers supported by the runtime.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxBackendKind {
    #[default]
    Disabled,
    Bubblewrap,
    Docker,
}

impl SandboxBackendKind {
    /// Return the stable backend label used in logs and tests.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::SandboxBackendKind;
    ///
    /// assert_eq!(SandboxBackendKind::Bubblewrap.as_str(), "bubblewrap");
    /// ```
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Bubblewrap => "bubblewrap",
            Self::Docker => "docker",
        }
    }
}

/// Bubblewrap-specific runtime options loaded from `config/capabilities.yaml`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct BubblewrapNamespaceConfig {
    user: bool,
    ipc: bool,
    pid: bool,
    uts: bool,
    net: bool,
}

impl Default for BubblewrapNamespaceConfig {
    fn default() -> Self {
        Self {
            user: true,
            ipc: true,
            pid: true,
            uts: true,
            net: true,
        }
    }
}

impl BubblewrapNamespaceConfig {
    /// Return whether bubblewrap should unshare the user namespace.
    pub fn user(&self) -> bool {
        self.user
    }

    /// Return whether bubblewrap should unshare the IPC namespace.
    pub fn ipc(&self) -> bool {
        self.ipc
    }

    /// Return whether bubblewrap should unshare the PID namespace.
    pub fn pid(&self) -> bool {
        self.pid
    }

    /// Return whether bubblewrap should unshare the UTS namespace.
    pub fn uts(&self) -> bool {
        self.uts
    }

    /// Return whether bubblewrap should unshare the network namespace.
    pub fn net(&self) -> bool {
        self.net
    }
}

/// One named command-child enforcement profile reference.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct BubblewrapCommandProfileConfig {
    landlock_profile: String,
    seccomp_profile: String,
}

impl Default for BubblewrapCommandProfileConfig {
    fn default() -> Self {
        Self {
            landlock_profile: DEFAULT_COMMAND_LANDLOCK_PROFILE.to_string(),
            seccomp_profile: DEFAULT_COMMAND_SECCOMP_PROFILE.to_string(),
        }
    }
}

impl BubblewrapCommandProfileConfig {
    /// Return the builtin Landlock profile name used for command children.
    pub fn landlock_profile(&self) -> &str {
        &self.landlock_profile
    }

    /// Return the builtin Seccomp profile name used for command children.
    pub fn seccomp_profile(&self) -> &str {
        &self.seccomp_profile
    }
}

/// Command-child profile selection policy for the bubblewrap runtime.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct BubblewrapCommandProfilesConfig {
    selected_profile: String,
    profiles: BTreeMap<String, BubblewrapCommandProfileConfig>,
}

impl Default for BubblewrapCommandProfilesConfig {
    fn default() -> Self {
        let mut profiles = BTreeMap::new();
        profiles.insert(
            DEFAULT_COMMAND_PROFILE_NAME.to_string(),
            BubblewrapCommandProfileConfig::default(),
        );
        Self {
            selected_profile: DEFAULT_COMMAND_PROFILE_NAME.to_string(),
            profiles,
        }
    }
}

impl BubblewrapCommandProfilesConfig {
    /// Return the logical command-child profile selected by default.
    pub fn selected_profile(&self) -> &str {
        &self.selected_profile
    }

    /// Return all declared command-child profile mappings.
    pub fn profiles(&self) -> &BTreeMap<String, BubblewrapCommandProfileConfig> {
        &self.profiles
    }
}

/// Compatibility requirements for bubblewrap kernel enforcement.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct BubblewrapCompatibilityConfig {
    require_landlock: bool,
    min_landlock_abi: u8,
    require_seccomp: bool,
    strict: bool,
}

impl Default for BubblewrapCompatibilityConfig {
    fn default() -> Self {
        Self {
            require_landlock: true,
            min_landlock_abi: 1,
            require_seccomp: true,
            strict: true,
        }
    }
}

impl BubblewrapCompatibilityConfig {
    /// Return whether Landlock support is mandatory for this policy.
    pub fn require_landlock(&self) -> bool {
        self.require_landlock
    }

    /// Return the minimum Landlock ABI required by this policy.
    pub fn min_landlock_abi(&self) -> u8 {
        self.min_landlock_abi
    }

    /// Return whether Seccomp support is mandatory for this policy.
    pub fn require_seccomp(&self) -> bool {
        self.require_seccomp
    }

    /// Return whether any enforcement downgrade should fail closed.
    pub fn strict(&self) -> bool {
        self.strict
    }
}

/// Bubblewrap-specific runtime options loaded from `config/capabilities.yaml`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct BubblewrapCapabilityConfig {
    executable: PathBuf,
    namespaces: BubblewrapNamespaceConfig,
    baseline_seccomp_profile: String,
    proxy_landlock_profile: String,
    command_profiles: BubblewrapCommandProfilesConfig,
    compatibility: BubblewrapCompatibilityConfig,
}

impl Default for BubblewrapCapabilityConfig {
    fn default() -> Self {
        Self {
            executable: PathBuf::from("bwrap"),
            namespaces: BubblewrapNamespaceConfig::default(),
            baseline_seccomp_profile: DEFAULT_BASELINE_SECCOMP_PROFILE.to_string(),
            proxy_landlock_profile: DEFAULT_PROXY_LANDLOCK_PROFILE.to_string(),
            command_profiles: BubblewrapCommandProfilesConfig::default(),
            compatibility: BubblewrapCompatibilityConfig::default(),
        }
    }
}

impl BubblewrapCapabilityConfig {
    /// Return the configured `bwrap` executable path or command name.
    pub fn executable(&self) -> &Path {
        &self.executable
    }

    /// Return the namespace policy used when configuring bubblewrap.
    pub fn namespaces(&self) -> &BubblewrapNamespaceConfig {
        &self.namespaces
    }

    /// Return the builtin baseline Seccomp profile name for the proxy process.
    pub fn baseline_seccomp_profile(&self) -> &str {
        &self.baseline_seccomp_profile
    }

    /// Return the builtin proxy Landlock profile name.
    pub fn proxy_landlock_profile(&self) -> &str {
        &self.proxy_landlock_profile
    }

    /// Return the command-child profile selection mapping.
    pub fn command_profiles(&self) -> &BubblewrapCommandProfilesConfig {
        &self.command_profiles
    }

    /// Return the compatibility requirements for kernel enforcement.
    pub fn compatibility(&self) -> &BubblewrapCompatibilityConfig {
        &self.compatibility
    }

    fn validate(&self) -> Result<()> {
        if self.executable.as_os_str().is_empty() {
            bail!("sandbox.bubblewrap.executable must not be blank");
        }
        validate_kernel_enforcement_config(self)
    }
}

/// Docker-specific runtime options reserved for future backend support.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct DockerCapabilityConfig {
    image: Option<String>,
}

impl DockerCapabilityConfig {
    /// Return the optional Docker image hint reserved for future use.
    pub fn image(&self) -> Option<&str> {
        self.image.as_deref()
    }
}

/// Global sandbox capability policy loaded from `config/capabilities.yaml`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct SandboxCapabilityConfig {
    sandbox: SandboxPolicyConfig,
}

impl Default for SandboxCapabilityConfig {
    fn default() -> Self {
        Self {
            sandbox: SandboxPolicyConfig::default(),
        }
    }
}

impl SandboxCapabilityConfig {
    /// Load the workspace-global sandbox capability file from `config/capabilities.yaml`.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use openjarvis::agent::SandboxCapabilityConfig;
    ///
    /// let config = SandboxCapabilityConfig::load_for_workspace(".")
    ///     .expect("sandbox capability config should load");
    /// assert!(!config.sandbox().workspace_sync_dir().as_os_str().is_empty());
    /// ```
    pub fn load_for_workspace(workspace_root: impl AsRef<Path>) -> Result<Self> {
        let workspace_root = workspace_root.as_ref();
        let path = workspace_root.join(DEFAULT_CAPABILITIES_CONFIG_PATH);
        if !path.exists() {
            let mut config = Self::default();
            config.resolve_paths(workspace_root);
            config.validate()?;
            info!(
                workspace_root = %workspace_root.display(),
                config_path = %path.display(),
                backend = config.sandbox.backend().as_str(),
                "sandbox capability config not found, using defaults"
            );
            return Ok(config);
        }

        let raw = fs::read_to_string(&path).with_context(|| {
            format!(
                "failed to read sandbox capability config {}",
                path.display()
            )
        })?;
        let mut config = serde_yaml::from_str::<Self>(&raw).with_context(|| {
            format!(
                "failed to parse sandbox capability config {}",
                path.display()
            )
        })?;
        config.resolve_paths(workspace_root);
        config.validate().with_context(|| {
            format!(
                "failed to validate sandbox capability config {}",
                path.display()
            )
        })?;
        info!(
            workspace_root = %workspace_root.display(),
            config_path = %path.display(),
            backend = config.sandbox.backend().as_str(),
            workspace_sync_dir = %config.sandbox.workspace_sync_dir().display(),
            "loaded sandbox capability config"
        );
        Ok(config)
    }

    /// Parse one capability policy from YAML and resolve relative paths against the workspace root.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::{SandboxBackendKind, SandboxCapabilityConfig};
    ///
    /// let config = SandboxCapabilityConfig::from_yaml_str(
    ///     "sandbox:\n  backend: bubblewrap\n",
    ///     "/tmp/openjarvis-demo",
    /// )
    /// .expect("sandbox capability config should parse");
    ///
    /// assert_eq!(config.sandbox().backend(), SandboxBackendKind::Bubblewrap);
    /// ```
    pub fn from_yaml_str(yaml: &str, workspace_root: impl AsRef<Path>) -> Result<Self> {
        let workspace_root = workspace_root.as_ref();
        let mut config = serde_yaml::from_str::<Self>(yaml)
            .context("failed to parse sandbox capability config from yaml string")?;
        config.resolve_paths(workspace_root);
        config
            .validate()
            .context("failed to validate sandbox capability config from yaml string")?;
        Ok(config)
    }

    /// Return the resolved sandbox policy section.
    pub fn sandbox(&self) -> &SandboxPolicyConfig {
        &self.sandbox
    }

    fn resolve_paths(&mut self, workspace_root: &Path) {
        self.sandbox.resolve_paths(workspace_root);
    }

    fn validate(&self) -> Result<()> {
        self.sandbox.validate()
    }
}

/// Sandbox policy shared by all users of the current workspace.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct SandboxPolicyConfig {
    backend: SandboxBackendKind,
    workspace_sync_dir: PathBuf,
    restricted_host_paths: Vec<PathBuf>,
    allow_parent_access: bool,
    bubblewrap: BubblewrapCapabilityConfig,
    docker: DockerCapabilityConfig,
}

impl Default for SandboxPolicyConfig {
    fn default() -> Self {
        Self {
            backend: SandboxBackendKind::Disabled,
            workspace_sync_dir: PathBuf::from(DEFAULT_WORKSPACE_SYNC_DIR),
            restricted_host_paths: vec![
                PathBuf::from("~/.ssh"),
                PathBuf::from("~/.gnupg"),
                PathBuf::from("/etc"),
                PathBuf::from("/proc"),
                PathBuf::from("/sys"),
                PathBuf::from("/dev"),
            ],
            allow_parent_access: false,
            bubblewrap: BubblewrapCapabilityConfig::default(),
            docker: DockerCapabilityConfig::default(),
        }
    }
}

impl SandboxPolicyConfig {
    /// Return the selected backend kind.
    pub fn backend(&self) -> SandboxBackendKind {
        self.backend
    }

    /// Return the resolved synchronized workspace root on the host.
    pub fn workspace_sync_dir(&self) -> &Path {
        &self.workspace_sync_dir
    }

    /// Return the resolved restricted host paths.
    pub fn restricted_host_paths(&self) -> &[PathBuf] {
        &self.restricted_host_paths
    }

    /// Return whether path resolution may escape above the synchronized workspace root.
    pub fn allow_parent_access(&self) -> bool {
        self.allow_parent_access
    }

    /// Return the bubblewrap-specific policy section.
    pub fn bubblewrap(&self) -> &BubblewrapCapabilityConfig {
        &self.bubblewrap
    }

    /// Return the docker-specific policy section.
    pub fn docker(&self) -> &DockerCapabilityConfig {
        &self.docker
    }

    fn resolve_paths(&mut self, workspace_root: &Path) {
        self.workspace_sync_dir = resolve_config_path(&self.workspace_sync_dir, workspace_root);
        self.restricted_host_paths = self
            .restricted_host_paths
            .iter()
            .map(|path| resolve_config_path(path, workspace_root))
            .collect::<Vec<_>>();
    }

    fn validate(&self) -> Result<()> {
        if self.workspace_sync_dir.as_os_str().is_empty() {
            bail!("sandbox.workspace_sync_dir must not be blank");
        }
        if self
            .restricted_host_paths
            .iter()
            .any(|path| path.as_os_str().is_empty())
        {
            bail!("sandbox.restricted_host_paths must not contain blank entries");
        }
        if self.backend == SandboxBackendKind::Bubblewrap {
            self.bubblewrap.validate()?;
        }
        Ok(())
    }
}

/// Stable JSON-RPC request sent between the host and the internal sandbox proxy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SandboxJsonRpcRequest {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

impl SandboxJsonRpcRequest {
    /// Build one JSON-RPC 2.0 request.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::SandboxJsonRpcRequest;
    /// use serde_json::json;
    ///
    /// let request = SandboxJsonRpcRequest::new(1, "rpc.ping", json!({}));
    /// assert_eq!(request.jsonrpc, "2.0");
    /// ```
    pub fn new(id: u64, method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            method: method.into(),
            params,
        }
    }
}

/// Structured JSON-RPC error payload returned by the sandbox proxy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SandboxJsonRpcError {
    pub code: i64,
    pub message: String,
}

impl SandboxJsonRpcError {
    fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Stable JSON-RPC response sent back by the internal sandbox proxy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SandboxJsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<SandboxJsonRpcError>,
}

impl SandboxJsonRpcResponse {
    fn success(id: u64, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: Some(id),
            result: Some(result),
            error: None,
        }
    }

    fn failure(id: Option<u64>, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(SandboxJsonRpcError::new(code, message)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SandboxWriteTextParams {
    path: String,
    content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SandboxReadTextParams {
    path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SandboxEditTextParams {
    path: String,
    old_text: String,
    new_text: String,
}

/// Structured result returned by sandbox-native single-executor text edits.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SandboxEditTextResult {
    pub match_count: usize,
    pub replaced_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SandboxCommandExecParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    command_profile: Option<String>,
    request: CommandExecutionRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SandboxCommandWriteParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    thread_id: Option<String>,
    request: CommandWriteRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SandboxCommandListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    thread_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SandboxPolicyProfileParams {
    command_profile: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum SandboxExecutorActionKind {
    Read,
    Write,
    Edit,
    CommandOnce,
    CommandSession,
}

impl SandboxExecutorActionKind {
    fn is_command(self) -> bool {
        matches!(self, Self::CommandOnce | Self::CommandSession)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SandboxExecutorPolicySnapshot {
    read_paths: Vec<PathBuf>,
    write_paths: Vec<PathBuf>,
    allow_tmp: bool,
    action: SandboxExecutorActionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    seccomp_tier: Option<String>,
    compatibility: kernel::KernelEnforcementCompatibilityPlan,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    final_command_profile: Option<SandboxCommandChildProfilePlan>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SandboxExecutorBootstrap {
    snapshot: SandboxExecutorPolicySnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SandboxReadTextResult {
    path: String,
    content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SandboxWriteTextResult {
    path: String,
    bytes_written: usize,
}

#[derive(Debug, Clone)]
struct SandboxExecutorPolicySource {
    command_profile: String,
}

impl SandboxExecutorPolicySource {
    fn new(command_profile: String) -> Self {
        Self { command_profile }
    }

    fn command_profile(&self) -> &str {
        &self.command_profile
    }

    fn set_command_profile(&mut self, command_profile: String) {
        self.command_profile = command_profile;
    }
}

#[derive(Debug, Clone)]
struct SandboxPathPolicy {
    workspace_root: PathBuf,
    restricted_host_paths: Vec<PathBuf>,
    allow_parent_access: bool,
}

#[derive(Debug, Clone)]
struct SandboxProxyRuntimeContext {
    path_policy: SandboxPathPolicy,
    kernel_enforcement: Option<SandboxKernelEnforcementPlan>,
}

struct SandboxSessionExecutorClient {
    thread_id: String,
    child: Child,
    transport: BubblewrapJsonRpcTransport,
}

struct SandboxProxyState {
    path_policy: SandboxPathPolicy,
    kernel_enforcement: Option<SandboxKernelEnforcementPlan>,
    executor_policy_source: SandboxExecutorPolicySource,
    session_executors: HashMap<String, SandboxSessionExecutorClient>,
}

impl SandboxPathPolicy {
    fn from_policy(config: &SandboxPolicyConfig) -> Self {
        Self {
            workspace_root: normalize_path(config.workspace_sync_dir()),
            restricted_host_paths: config
                .restricted_host_paths()
                .iter()
                .map(|path| normalize_path(path))
                .collect::<Vec<_>>(),
            allow_parent_access: config.allow_parent_access(),
        }
    }

    fn resolve_request_path(&self, requested_path: &Path) -> Result<PathBuf> {
        let normalized_requested_path = normalize_path(requested_path);
        let is_explicit_tmp_request = normalized_requested_path.is_absolute()
            && path_is_within(&normalized_requested_path, Path::new(SANDBOX_TMP_DIR));
        let candidate = if normalized_requested_path.is_absolute()
            && path_is_within(
                &normalized_requested_path,
                Path::new(SANDBOX_WORKSPACE_MOUNT),
            ) {
            let workspace_relative_path = normalized_requested_path
                .strip_prefix(Path::new(SANDBOX_WORKSPACE_MOUNT))
                .expect("sandbox workspace mount path should strip cleanly");
            normalize_path(&self.workspace_root.join(workspace_relative_path))
        } else if normalized_requested_path.is_absolute() {
            normalized_requested_path
        } else {
            normalize_path(&self.workspace_root.join(requested_path))
        };

        if self
            .restricted_host_paths
            .iter()
            .any(|restricted| path_is_within(&candidate, restricted))
        {
            bail!(
                "path `{}` targets a restricted host directory",
                candidate.display()
            );
        }

        if !self.allow_parent_access
            && !path_is_within(&candidate, &self.workspace_root)
            && !is_explicit_tmp_request
        {
            bail!(
                "path `{}` escapes synchronized workspace `{}` and is not under `{}`",
                candidate.display(),
                self.workspace_root.display(),
                SANDBOX_TMP_DIR
            );
        }

        Ok(candidate)
    }
}

/// Shared sandbox interface held by the agent worker.
pub trait Sandbox: Send + Sync {
    /// Return the stable backend label used for diagnostics and tests.
    fn kind(&self) -> &'static str;

    /// Return the resolved host-visible synchronized workspace root.
    fn workspace_root(&self) -> &Path;

    /// Return the capability policy used to initialize this sandbox.
    fn capabilities(&self) -> &SandboxCapabilityConfig;

    /// Write one text file inside the synchronized workspace.
    fn write_workspace_text(&self, relative_path: &Path, content: &str) -> Result<()>;

    /// Read one text file inside the synchronized workspace.
    fn read_workspace_text(&self, relative_path: &Path) -> Result<String>;

    /// Edit one text file inside the synchronized workspace in a single sandbox executor.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use openjarvis::agent::{SandboxCapabilityConfig, build_sandbox};
    /// use std::path::Path;
    ///
    /// let sandbox = build_sandbox(SandboxCapabilityConfig::load_for_workspace(".")?)?;
    /// let result = sandbox.edit_workspace_text(Path::new("demo.txt"), "old", "new")?;
    /// assert_eq!(result.replaced_count, 1);
    /// # anyhow::Ok::<(), anyhow::Error>(())
    /// ```
    fn edit_workspace_text(
        &self,
        relative_path: &Path,
        old_text: &str,
        new_text: &str,
    ) -> Result<SandboxEditTextResult>;

    /// Execute one command session request inside the sandbox.
    fn exec_command(
        &self,
        thread_id: Option<&str>,
        request: CommandExecutionRequest,
    ) -> Result<CommandExecutionResult>;

    /// Continue one existing command session inside the sandbox.
    fn write_command_stdin(
        &self,
        thread_id: Option<&str>,
        request: CommandWriteRequest,
    ) -> Result<CommandExecutionResult>;

    /// List unread command sessions visible to the current thread inside the sandbox.
    fn list_unread_command_tasks(
        &self,
        thread_id: Option<&str>,
    ) -> Result<Vec<CommandTaskSnapshot>>;
}

/// Build one sandbox instance from the resolved capability policy.
///
/// # 示例
/// ```rust,no_run
/// use openjarvis::agent::{SandboxCapabilityConfig, build_sandbox};
///
/// let capabilities = SandboxCapabilityConfig::load_for_workspace(".")
///     .expect("sandbox capability config should load");
/// let sandbox = build_sandbox(capabilities).expect("sandbox should build");
/// assert!(!sandbox.kind().is_empty());
/// ```
pub fn build_sandbox(capabilities: SandboxCapabilityConfig) -> Result<Arc<dyn Sandbox>> {
    let backend = capabilities.sandbox().backend();
    info!(
        backend = backend.as_str(),
        workspace_sync_dir = %capabilities.sandbox().workspace_sync_dir().display(),
        "building sandbox backend from capability policy"
    );
    match backend {
        SandboxBackendKind::Disabled => Ok(Arc::new(DisabledSandbox::new(capabilities))),
        SandboxBackendKind::Bubblewrap => Ok(Arc::new(BubblewrapSandbox::new(capabilities)?)),
        SandboxBackendKind::Docker => {
            bail!("docker sandbox backend is not implemented yet")
        }
    }
}

/// Run one hidden internal sandbox helper command.
pub async fn run_internal_sandbox_command(command: &InternalSandboxCommand) -> Result<()> {
    let command = command.clone();
    tokio::task::spawn_blocking(move || match command {
        InternalSandboxCommand::Proxy {
            workspace_root,
            enforcement_plan_json,
            restricted_host_paths,
            allow_parent_access,
        } => {
            let kernel_enforcement = enforcement_plan_json
                .as_deref()
                .map(|raw| {
                    serde_json::from_str::<SandboxKernelEnforcementPlan>(raw)
                        .context("failed to decode sandbox proxy kernel enforcement plan")
                })
                .transpose()?;
            run_sandbox_proxy(SandboxProxyRuntimeContext {
                path_policy: SandboxPathPolicy {
                    workspace_root: normalize_path(&workspace_root),
                    restricted_host_paths: restricted_host_paths
                        .iter()
                        .map(|path| normalize_path(path))
                        .collect::<Vec<_>>(),
                    allow_parent_access,
                },
                kernel_enforcement,
            })
        }
        InternalSandboxCommand::Exec {
            workspace_root,
            profile_json,
            workdir,
            program,
            args,
        } => {
            let profile = serde_json::from_str::<SandboxCommandChildProfilePlan>(&profile_json)
                .context("failed to decode sandbox command-child profile")?;
            run_sandbox_exec(
                normalize_path(&workspace_root),
                profile,
                workdir.map(|path| normalize_path(&path)),
                program,
                args,
            )
        }
        InternalSandboxCommand::Executor { kind, snapshot_fd } => {
            run_sandbox_executor(kind, snapshot_fd)
        }
    })
    .await
    .context("internal sandbox helper task failed to join")?
}

#[derive(Debug, Clone)]
pub struct DisabledSandbox {
    capabilities: SandboxCapabilityConfig,
    workspace_root: PathBuf,
}

impl DisabledSandbox {
    fn new(capabilities: SandboxCapabilityConfig) -> Self {
        let workspace_root = capabilities.sandbox().workspace_sync_dir().to_path_buf();
        Self {
            capabilities,
            workspace_root,
        }
    }
}

impl Sandbox for DisabledSandbox {
    fn kind(&self) -> &'static str {
        SandboxBackendKind::Disabled.as_str()
    }

    fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    fn capabilities(&self) -> &SandboxCapabilityConfig {
        &self.capabilities
    }

    fn write_workspace_text(&self, _relative_path: &Path, _content: &str) -> Result<()> {
        bail!("sandbox backend is disabled")
    }

    fn read_workspace_text(&self, _relative_path: &Path) -> Result<String> {
        bail!("sandbox backend is disabled")
    }

    fn edit_workspace_text(
        &self,
        _relative_path: &Path,
        _old_text: &str,
        _new_text: &str,
    ) -> Result<SandboxEditTextResult> {
        bail!("sandbox backend is disabled")
    }

    fn exec_command(
        &self,
        _thread_id: Option<&str>,
        _request: CommandExecutionRequest,
    ) -> Result<CommandExecutionResult> {
        bail!("sandbox backend is disabled")
    }

    fn write_command_stdin(
        &self,
        _thread_id: Option<&str>,
        _request: CommandWriteRequest,
    ) -> Result<CommandExecutionResult> {
        bail!("sandbox backend is disabled")
    }

    fn list_unread_command_tasks(
        &self,
        _thread_id: Option<&str>,
    ) -> Result<Vec<CommandTaskSnapshot>> {
        bail!("sandbox backend is disabled")
    }
}

#[derive(Debug)]
struct BubblewrapJsonRpcTransport {
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl BubblewrapJsonRpcTransport {
    fn new(stdin: ChildStdin, stdout: ChildStdout) -> Self {
        Self {
            stdin: BufWriter::new(stdin),
            stdout: BufReader::new(stdout),
            next_id: 1,
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let request = SandboxJsonRpcRequest::new(self.next_id, method, params);
        self.next_id += 1;
        let raw = serde_json::to_string(&request)
            .with_context(|| format!("failed to serialize sandbox request `{method}`"))?;
        debug!(
            request_id = request.id,
            method = request.method,
            raw_json = %raw,
            "sending sandbox jsonrpc request"
        );
        self.stdin
            .write_all(raw.as_bytes())
            .context("failed to write sandbox jsonrpc request")?;
        self.stdin
            .write_all(b"\n")
            .context("failed to terminate sandbox jsonrpc request")?;
        self.stdin
            .flush()
            .context("failed to flush sandbox jsonrpc request")?;

        let mut line = String::new();
        let read_bytes = self
            .stdout
            .read_line(&mut line)
            .context("failed to read sandbox jsonrpc response")?;
        if read_bytes == 0 {
            bail!("sandbox jsonrpc proxy closed before replying to `{method}`");
        }
        debug!(
            request_id = request.id,
            method = request.method,
            raw_json = %line.trim_end(),
            "received sandbox jsonrpc response"
        );
        let response = serde_json::from_str::<SandboxJsonRpcResponse>(&line)
            .with_context(|| format!("failed to parse sandbox jsonrpc response for `{method}`"))?;
        if response.id != Some(request.id) {
            bail!(
                "sandbox jsonrpc response id mismatch for `{method}`: expected {}, got {:?}",
                request.id,
                response.id
            );
        }
        if let Some(error) = response.error {
            bail!(
                "sandbox jsonrpc `{method}` failed with code {}: {}",
                error.code,
                error.message
            );
        }
        Ok(response.result.unwrap_or(Value::Null))
    }
}

/// Bubblewrap-backed sandbox runtime bridged through a long-lived JSON-RPC proxy.
pub struct BubblewrapSandbox {
    capabilities: SandboxCapabilityConfig,
    workspace_root: PathBuf,
    path_policy: SandboxPathPolicy,
    child: Mutex<Option<Child>>,
    transport: Mutex<BubblewrapJsonRpcTransport>,
}

impl BubblewrapSandbox {
    fn new(capabilities: SandboxCapabilityConfig) -> Result<Self> {
        if !cfg!(target_os = "linux") {
            bail!("bubblewrap sandbox backend is only supported on Linux");
        }

        let workspace_root = capabilities.sandbox().workspace_sync_dir().to_path_buf();
        fs::create_dir_all(&workspace_root).with_context(|| {
            format!(
                "failed to create synchronized sandbox workspace {}",
                workspace_root.display()
            )
        })?;
        let path_policy = SandboxPathPolicy::from_policy(capabilities.sandbox());
        let kernel_enforcement =
            compile_kernel_enforcement_plan(capabilities.sandbox().bubblewrap())
                .context("failed to compile bubblewrap kernel enforcement plan")?;
        let bwrap_executable = resolve_command_path(
            capabilities.sandbox().bubblewrap().executable(),
        )
        .with_context(|| {
            format!(
                "failed to resolve bubblewrap executable `{}`",
                capabilities.sandbox().bubblewrap().executable().display()
            )
        })?;
        let current_executable = resolve_sandbox_helper_executable()
            .context("failed to resolve sandbox helper executable")?;
        let current_executable_dir = current_executable
            .parent()
            .context("current executable must have a parent directory")?
            .to_path_buf();
        let current_executable_name = current_executable
            .file_name()
            .context("current executable must have a file name")?
            .to_os_string();
        let enforcement_plan_json = serde_json::to_string(&kernel_enforcement)
            .context("failed to serialize bubblewrap kernel enforcement plan")?;

        let mut command = Command::new(&bwrap_executable);
        configure_bubblewrap_command(
            &mut command,
            &current_executable_dir,
            &current_executable_name,
            &workspace_root,
            capabilities.sandbox(),
            &kernel_enforcement,
            &enforcement_plan_json,
        );
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        info!(
            executable = %bwrap_executable.display(),
            workspace_root = %workspace_root.display(),
            "launching bubblewrap sandbox proxy"
        );
        let mut child = command
            .spawn()
            .context("failed to spawn bubblewrap sandbox proxy")?;
        let stdin = child
            .stdin
            .take()
            .context("bubblewrap sandbox proxy missing stdin pipe")?;
        let stdout = child
            .stdout
            .take()
            .context("bubblewrap sandbox proxy missing stdout pipe")?;
        let mut transport = BubblewrapJsonRpcTransport::new(stdin, stdout);
        let ping = transport.call("rpc.ping", json!({}))?;
        debug!(ping = ?ping, "bubblewrap sandbox proxy replied to handshake");

        Ok(Self {
            capabilities,
            workspace_root,
            path_policy,
            child: Mutex::new(Some(child)),
            transport: Mutex::new(transport),
        })
    }

    fn call_jsonrpc(&self, method: &str, params: Value) -> Result<Value> {
        self.transport
            .lock()
            .expect("bubblewrap transport lock should not be poisoned")
            .call(method, params)
    }

    fn encode_proxy_path(&self, requested_path: &Path) -> Result<PathBuf> {
        let normalized_requested_path = normalize_path(requested_path);
        if normalized_requested_path.is_absolute()
            && path_is_within(
                &normalized_requested_path,
                Path::new(SANDBOX_WORKSPACE_MOUNT),
            )
        {
            // Preserve sandbox-visible `/workspace/...` paths so outputs like `pwd` or `ls`
            // can be reused directly in later tool calls without forcing the agent to
            // translate paths between host and sandbox mental models.
            let workspace_relative_path = normalized_requested_path
                .strip_prefix(Path::new(SANDBOX_WORKSPACE_MOUNT))
                .expect("sandbox workspace mount path should strip cleanly");
            let host_workspace_path =
                normalize_path(&self.workspace_root.join(workspace_relative_path));
            self.path_policy
                .resolve_request_path(&host_workspace_path)?;
            return Ok(normalized_requested_path);
        }

        let resolved_host_path = self.path_policy.resolve_request_path(requested_path)?;
        if path_is_within(&resolved_host_path, &self.workspace_root) {
            return Ok(resolved_host_path
                .strip_prefix(&self.workspace_root)
                .expect("resolved workspace path should remain under workspace root")
                .to_path_buf());
        }
        if path_is_within(&resolved_host_path, Path::new(SANDBOX_TMP_DIR)) {
            return Ok(resolved_host_path);
        }
        bail!(
            "resolved path `{}` is not encodable for the sandbox proxy",
            resolved_host_path.display()
        )
    }

    fn normalize_command_request(
        &self,
        mut request: CommandExecutionRequest,
    ) -> Result<CommandExecutionRequest> {
        request.workdir = request
            .workdir
            .as_deref()
            .map(|path| self.encode_proxy_path(path))
            .transpose()?;
        Ok(request)
    }
}

impl Drop for BubblewrapSandbox {
    fn drop(&mut self) {
        let Some(mut child) = self
            .child
            .lock()
            .expect("bubblewrap child lock should not be poisoned")
            .take()
        else {
            return;
        };
        if let Err(error) = child.kill() {
            warn!(error = %error, "failed to kill bubblewrap sandbox proxy on drop");
        }
        if let Err(error) = child.wait() {
            warn!(error = %error, "failed to wait for bubblewrap sandbox proxy on drop");
        }
    }
}

impl Sandbox for BubblewrapSandbox {
    fn kind(&self) -> &'static str {
        SandboxBackendKind::Bubblewrap.as_str()
    }

    fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    fn capabilities(&self) -> &SandboxCapabilityConfig {
        &self.capabilities
    }

    fn write_workspace_text(&self, relative_path: &Path, content: &str) -> Result<()> {
        let relative_to_workspace = self.encode_proxy_path(relative_path)?;
        let params = SandboxWriteTextParams {
            path: relative_to_workspace.display().to_string(),
            content: content.to_string(),
        };
        self.call_jsonrpc("fs.write_text", serde_json::to_value(params)?)
            .context("bubblewrap sandbox write_text request failed")?;
        Ok(())
    }

    fn read_workspace_text(&self, relative_path: &Path) -> Result<String> {
        let relative_to_workspace = self.encode_proxy_path(relative_path)?;
        let params = SandboxReadTextParams {
            path: relative_to_workspace.display().to_string(),
        };
        let result = self
            .call_jsonrpc("fs.read_text", serde_json::to_value(params)?)
            .context("bubblewrap sandbox read_text request failed")?;
        let result = serde_json::from_value::<SandboxReadTextResult>(result)
            .context("failed to decode sandbox read_text result")?;
        Ok(result.content)
    }

    fn edit_workspace_text(
        &self,
        relative_path: &Path,
        old_text: &str,
        new_text: &str,
    ) -> Result<SandboxEditTextResult> {
        let relative_to_workspace = self.encode_proxy_path(relative_path)?;
        let params = SandboxEditTextParams {
            path: relative_to_workspace.display().to_string(),
            old_text: old_text.to_string(),
            new_text: new_text.to_string(),
        };
        let result = self
            .call_jsonrpc("fs.edit_text", serde_json::to_value(params)?)
            .context("bubblewrap sandbox edit_text request failed")?;
        serde_json::from_value(result).context("failed to decode sandbox edit_text result")
    }

    fn exec_command(
        &self,
        thread_id: Option<&str>,
        request: CommandExecutionRequest,
    ) -> Result<CommandExecutionResult> {
        let params = SandboxCommandExecParams {
            thread_id: thread_id.map(str::to_string),
            command_profile: None,
            request: self.normalize_command_request(request)?,
        };
        let result = self
            .call_jsonrpc("command.exec", serde_json::to_value(params)?)
            .context("bubblewrap sandbox exec_command request failed")?;
        serde_json::from_value(result).context("failed to decode sandbox exec_command result")
    }

    fn write_command_stdin(
        &self,
        thread_id: Option<&str>,
        request: CommandWriteRequest,
    ) -> Result<CommandExecutionResult> {
        let params = SandboxCommandWriteParams {
            thread_id: thread_id.map(str::to_string),
            request,
        };
        let result = self
            .call_jsonrpc("command.write_stdin", serde_json::to_value(params)?)
            .context("bubblewrap sandbox write_stdin request failed")?;
        serde_json::from_value(result).context("failed to decode sandbox write_stdin result")
    }

    fn list_unread_command_tasks(
        &self,
        thread_id: Option<&str>,
    ) -> Result<Vec<CommandTaskSnapshot>> {
        let params = SandboxCommandListParams {
            thread_id: thread_id.map(str::to_string),
        };
        let result = self
            .call_jsonrpc("command.list_unread_tasks", serde_json::to_value(params)?)
            .context("bubblewrap sandbox list_unread_command_tasks request failed")?;
        serde_json::from_value(result)
            .context("failed to decode sandbox list_unread_command_tasks result")
    }
}

fn build_executor_policy_source(
    kernel_enforcement: Option<&SandboxKernelEnforcementPlan>,
) -> SandboxExecutorPolicySource {
    let command_profile = kernel_enforcement
        .map(SandboxKernelEnforcementPlan::default_command_profile)
        .unwrap_or(DEFAULT_COMMAND_PROFILE_NAME)
        .to_string();
    SandboxExecutorPolicySource::new(command_profile)
}

fn build_file_policy_snapshot(
    path_policy: &SandboxPathPolicy,
    kernel_enforcement: Option<&SandboxKernelEnforcementPlan>,
    action: SandboxExecutorActionKind,
    target: &Path,
) -> Result<SandboxExecutorPolicySnapshot> {
    let kernel_enforcement =
        kernel_enforcement.context("sandbox file executor requires kernel enforcement plan")?;
    let scope_root = executor_scope_root(path_policy, target);
    Ok(SandboxExecutorPolicySnapshot {
        read_paths: vec![scope_root.clone()],
        write_paths: matches!(
            action,
            SandboxExecutorActionKind::Write | SandboxExecutorActionKind::Edit
        )
        .then_some(scope_root)
        .into_iter()
        .collect(),
        allow_tmp: path_is_within(target, Path::new(SANDBOX_TMP_DIR)),
        action,
        session_id: None,
        seccomp_tier: None,
        compatibility: kernel_enforcement.compatibility().clone(),
        final_command_profile: None,
    })
}

fn build_command_policy_snapshot(
    state: &SandboxProxyState,
    command_profile: Option<&str>,
    request: &CommandExecutionRequest,
    action: SandboxExecutorActionKind,
    session_id: String,
) -> Result<SandboxExecutorPolicySnapshot> {
    let kernel_enforcement = state
        .kernel_enforcement
        .as_ref()
        .context("sandbox command executor requires kernel enforcement plan")?;
    let profile_name = command_profile.unwrap_or(state.executor_policy_source.command_profile());
    let profile = kernel_enforcement
        .command_profile(Some(profile_name))?
        .clone();
    let mut read_paths = vec![state.path_policy.workspace_root.clone()];
    let mut write_paths = Vec::new();
    if profile.landlock_profile() == kernel::COMMAND_READONLY_LANDLOCK_PROFILE {
        write_paths.push(PathBuf::from(SANDBOX_TMP_DIR));
    } else {
        write_paths.push(state.path_policy.workspace_root.clone());
        write_paths.push(PathBuf::from(SANDBOX_TMP_DIR));
    }

    if let Some(workdir) = request.workdir.as_deref() {
        let scope_root = executor_scope_root(&state.path_policy, workdir);
        read_paths.push(scope_root.clone());
        if profile.landlock_profile() != kernel::COMMAND_READONLY_LANDLOCK_PROFILE
            || path_is_within(&scope_root, Path::new(SANDBOX_TMP_DIR))
        {
            write_paths.push(scope_root);
        }
    }

    Ok(SandboxExecutorPolicySnapshot {
        read_paths: normalize_executor_paths(read_paths),
        write_paths: normalize_executor_paths(write_paths),
        allow_tmp: true,
        action,
        session_id: Some(session_id),
        seccomp_tier: Some(profile.seccomp_profile().to_string()),
        compatibility: kernel_enforcement.compatibility().clone(),
        final_command_profile: Some(profile),
    })
}

fn normalize_executor_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut paths = paths
        .into_iter()
        .map(|path| normalize_path(&path))
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    paths
}

fn executor_scope_root(path_policy: &SandboxPathPolicy, target: &Path) -> PathBuf {
    if path_is_within(target, &path_policy.workspace_root) {
        return path_policy.workspace_root.clone();
    }
    if path_is_within(target, Path::new(SANDBOX_TMP_DIR)) {
        return PathBuf::from(SANDBOX_TMP_DIR);
    }
    nearest_existing_ancestor(target)
}

fn nearest_existing_ancestor(target: &Path) -> PathBuf {
    for ancestor in target.ancestors() {
        if ancestor.exists() {
            return ancestor.to_path_buf();
        }
    }
    PathBuf::from("/")
}

fn normalize_proxy_thread_id(thread_id: Option<&str>) -> String {
    thread_id
        .map(str::trim)
        .filter(|thread_id| !thread_id.is_empty())
        .unwrap_or("__standalone_command_thread__")
        .to_string()
}

fn proxy_command_executor_kind(request: &CommandExecutionRequest) -> InternalSandboxExecutorKind {
    // 短 yield 或显式 PTY 往往意味着后续还会续写 stdin，因此直接走 session executor。
    if request.tty || request.yield_time_ms <= 100 {
        InternalSandboxExecutorKind::CommandSession
    } else {
        InternalSandboxExecutorKind::CommandOnce
    }
}

#[cfg(unix)]
fn spawn_executor_transport(
    kind: InternalSandboxExecutorKind,
    snapshot: &SandboxExecutorPolicySnapshot,
) -> Result<(Child, BubblewrapJsonRpcTransport)> {
    let helper_executable =
        resolve_sandbox_helper_executable().context("failed to resolve sandbox executor helper")?;
    let snapshot_fd = build_executor_snapshot_fd(&SandboxExecutorBootstrap {
        snapshot: snapshot.clone(),
    })?;
    let mut command = Command::new(helper_executable);
    command
        .arg("internal-sandbox")
        .arg("executor")
        .arg("--kind")
        .arg(kind.as_str())
        .arg("--snapshot-fd")
        .arg(SANDBOX_EXECUTOR_POLICY_FD.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    attach_snapshot_fd(&mut command, &snapshot_fd)?;
    let mut child = command
        .spawn()
        .context("failed to spawn sandbox executor")?;
    drop(snapshot_fd);
    let stdin = child
        .stdin
        .take()
        .context("sandbox executor missing stdin pipe")?;
    let stdout = child
        .stdout
        .take()
        .context("sandbox executor missing stdout pipe")?;
    Ok((child, BubblewrapJsonRpcTransport::new(stdin, stdout)))
}

#[cfg(not(unix))]
fn spawn_executor_transport(
    _kind: InternalSandboxExecutorKind,
    _snapshot: &SandboxExecutorPolicySnapshot,
) -> Result<(Child, BubblewrapJsonRpcTransport)> {
    bail!("sandbox executor helpers are only supported on unix")
}

#[cfg(unix)]
fn build_executor_snapshot_fd(bootstrap: &SandboxExecutorBootstrap) -> Result<OwnedFd> {
    let raw =
        serde_json::to_vec(bootstrap).context("failed to serialize sandbox executor bootstrap")?;
    let mut pipe_fds = [0; 2];
    let status = unsafe { libc::pipe2(pipe_fds.as_mut_ptr(), libc::O_CLOEXEC) };
    if status != 0 {
        bail!(
            "failed to create sandbox executor snapshot pipe: {}",
            std::io::Error::last_os_error()
        );
    }
    let read_fd = unsafe { OwnedFd::from_raw_fd(pipe_fds[0]) };
    let mut write_file = unsafe { fs::File::from_raw_fd(pipe_fds[1]) };
    write_file
        .write_all(&raw)
        .context("failed to write sandbox executor bootstrap")?;
    write_file
        .flush()
        .context("failed to flush sandbox executor bootstrap")?;
    drop(write_file);
    Ok(read_fd)
}

#[cfg(unix)]
fn attach_snapshot_fd(command: &mut Command, snapshot_fd: &OwnedFd) -> Result<()> {
    let source_fd = snapshot_fd.as_raw_fd();
    unsafe {
        command.pre_exec(move || {
            if source_fd == SANDBOX_EXECUTOR_POLICY_FD {
                let flags = libc::fcntl(SANDBOX_EXECUTOR_POLICY_FD, libc::F_GETFD);
                if flags < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::fcntl(
                    SANDBOX_EXECUTOR_POLICY_FD,
                    libc::F_SETFD,
                    flags & !libc::FD_CLOEXEC,
                ) != 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                return Ok(());
            }

            if libc::dup2(source_fd, SANDBOX_EXECUTOR_POLICY_FD) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(())
}

fn wait_for_executor_exit(child: &mut Child, subject: &str) {
    match child.wait() {
        Ok(status) => {
            debug!(subject, exit_status = ?status, "sandbox executor exited");
        }
        Err(error) => {
            warn!(subject, error = %error, "failed to wait for sandbox executor");
        }
    }
}

fn stop_session_executor(session_id: &str, client: &mut SandboxSessionExecutorClient) {
    if let Err(error) = client.child.kill() {
        debug!(
            session_id,
            error = %error,
            "session executor kill returned before proxy cleanup"
        );
    }
    wait_for_executor_exit(&mut client.child, "session-executor");
}

fn execute_file_request<T>(
    state: &SandboxProxyState,
    action: SandboxExecutorActionKind,
    method: &str,
    params: impl Serialize,
    target: &Path,
) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let snapshot = build_file_policy_snapshot(
        &state.path_policy,
        state.kernel_enforcement.as_ref(),
        action,
        target,
    )?;
    debug!(
        method,
        action = ?snapshot.action,
        target = %target.display(),
        read_paths = ?snapshot.read_paths,
        write_paths = ?snapshot.write_paths,
        "spawning sandbox file executor"
    );
    let (mut child, mut transport) =
        spawn_executor_transport(InternalSandboxExecutorKind::File, &snapshot)?;
    let result = transport
        .call(method, serde_json::to_value(params)?)
        .with_context(|| format!("sandbox file executor `{method}` failed"))?;
    wait_for_executor_exit(&mut child, "file-executor");
    serde_json::from_value(result)
        .with_context(|| format!("failed to decode sandbox file executor result for `{method}`"))
}

fn execute_command_request(
    state: &mut SandboxProxyState,
    thread_id: Option<&str>,
    command_profile: Option<&str>,
    request: CommandExecutionRequest,
) -> Result<CommandExecutionResult> {
    let normalized_thread_id = normalize_proxy_thread_id(thread_id);
    let session_id = format!("command-session-{}", uuid::Uuid::new_v4());
    let executor_kind = proxy_command_executor_kind(&request);
    let snapshot = build_command_policy_snapshot(
        state,
        command_profile,
        &request,
        match executor_kind {
            InternalSandboxExecutorKind::CommandOnce => SandboxExecutorActionKind::CommandOnce,
            InternalSandboxExecutorKind::CommandSession => {
                SandboxExecutorActionKind::CommandSession
            }
            InternalSandboxExecutorKind::File => {
                bail!("file executor kind is invalid for command requests")
            }
        },
        session_id.clone(),
    )?;
    debug!(
        executor_kind = executor_kind.as_str(),
        thread_id = %normalized_thread_id,
        session_id,
        seccomp_tier = ?snapshot.seccomp_tier,
        read_paths = ?snapshot.read_paths,
        write_paths = ?snapshot.write_paths,
        "spawning sandbox command executor"
    );
    let (child, mut transport) = spawn_executor_transport(executor_kind, &snapshot)?;
    let params = SandboxCommandExecParams {
        thread_id: Some(normalized_thread_id.clone()),
        command_profile: None,
        request,
    };
    let result = transport
        .call("command.exec", serde_json::to_value(params)?)
        .context("sandbox command executor failed to start command")?;
    let result = serde_json::from_value::<CommandExecutionResult>(result)
        .context("failed to decode sandbox command executor result")?;

    if result.running {
        let returned_session_id = result
            .session_id
            .clone()
            .context("sandbox command executor returned running result without session id")?;
        state.session_executors.insert(
            returned_session_id,
            SandboxSessionExecutorClient {
                thread_id: normalized_thread_id,
                child,
                transport,
            },
        );
        return Ok(result);
    }

    drop(transport);
    let mut child = child;
    wait_for_executor_exit(&mut child, "command-executor");
    Ok(result)
}

fn write_command_stdin_via_executor(
    state: &mut SandboxProxyState,
    thread_id: Option<&str>,
    request: CommandWriteRequest,
) -> Result<CommandExecutionResult> {
    let normalized_thread_id = normalize_proxy_thread_id(thread_id);
    let session_id = request.session_id.clone();
    let mut client = state
        .session_executors
        .remove(&session_id)
        .ok_or_else(|| anyhow::anyhow!("unknown command session `{session_id}`"))?;
    if client.thread_id != normalized_thread_id {
        state.session_executors.insert(session_id, client);
        bail!(
            "command session `{}` does not belong to thread `{}`",
            request.session_id,
            normalized_thread_id
        );
    }

    let result = client
        .transport
        .call(
            "command.write_stdin",
            serde_json::to_value(SandboxCommandWriteParams {
                thread_id: Some(normalized_thread_id.clone()),
                request,
            })?,
        )
        .context("sandbox session executor failed to write stdin")?;
    let result = serde_json::from_value::<CommandExecutionResult>(result)
        .context("failed to decode sandbox session write_stdin result")?;
    if result.running {
        let session_id = result
            .session_id
            .clone()
            .context("running session result missing session id")?;
        state.session_executors.insert(session_id, client);
    } else {
        stop_session_executor(&session_id, &mut client);
    }
    Ok(result)
}

fn list_unread_tasks_via_executor(
    state: &mut SandboxProxyState,
    thread_id: Option<&str>,
) -> Vec<CommandTaskSnapshot> {
    let normalized_thread_id = normalize_proxy_thread_id(thread_id);
    let session_ids = state.session_executors.keys().cloned().collect::<Vec<_>>();
    let mut tasks = Vec::new();
    for session_id in session_ids {
        let Some(mut client) = state.session_executors.remove(&session_id) else {
            continue;
        };
        let snapshot = match client.transport.call("command.snapshot", json!({})) {
            Ok(result) => serde_json::from_value::<CommandTaskSnapshot>(result)
                .context("failed to decode sandbox session snapshot"),
            Err(error) => Err(error).context("sandbox session executor snapshot request failed"),
        };
        match snapshot {
            Ok(snapshot) => {
                if snapshot.thread_id == normalized_thread_id && snapshot.has_unread_output {
                    tasks.push(snapshot.clone());
                }
                state.session_executors.insert(session_id, client);
            }
            Err(error) => {
                warn!(
                    session_id,
                    error = %error,
                    "dropping broken sandbox session executor"
                );
                stop_session_executor(&session_id, &mut client);
            }
        }
    }
    tasks.sort_by(|left, right| left.updated_at.cmp(&right.updated_at));
    tasks
}

fn run_sandbox_proxy(context: SandboxProxyRuntimeContext) -> Result<()> {
    info!(
        workspace_root = %context.path_policy.workspace_root.display(),
        allow_parent_access = context.path_policy.allow_parent_access,
        has_kernel_enforcement = context.kernel_enforcement.is_some(),
        "sandbox proxy started"
    );
    if let Some(plan) = context.kernel_enforcement.as_ref() {
        install_proxy_landlock(plan, &context.path_policy.workspace_root)
            .context("failed to install sandbox proxy landlock enforcement")?;
    }
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = BufWriter::new(stdout.lock());
    let mut state = SandboxProxyState {
        path_policy: context.path_policy.clone(),
        kernel_enforcement: context.kernel_enforcement.clone(),
        executor_policy_source: build_executor_policy_source(context.kernel_enforcement.as_ref()),
        session_executors: HashMap::new(),
    };
    if let Some(plan) = context.kernel_enforcement.as_ref() {
        install_proxy_seccomp(plan).context("failed to install sandbox proxy baseline seccomp")?;
    }

    loop {
        let mut line = String::new();
        let bytes = reader
            .read_line(&mut line)
            .context("failed to read sandbox proxy request")?;
        if bytes == 0 {
            break;
        }
        if line.trim().is_empty() {
            continue;
        }
        debug!(
            raw_json = %line.trim_end(),
            "received sandbox proxy jsonrpc request"
        );

        let response = match serde_json::from_str::<SandboxJsonRpcRequest>(&line) {
            Ok(request) => handle_sandbox_proxy_request(&mut state, request),
            Err(error) => SandboxJsonRpcResponse::failure(
                None,
                -32700,
                format!("failed to parse request: {error}"),
            ),
        };
        let raw_response = serde_json::to_string(&response)
            .context("failed to serialize sandbox proxy response")?;
        debug!(
            raw_json = %raw_response,
            "sending sandbox proxy jsonrpc response"
        );
        writer
            .write_all(raw_response.as_bytes())
            .context("failed to write sandbox proxy response")?;
        writer
            .write_all(b"\n")
            .context("failed to terminate sandbox proxy response")?;
        writer
            .flush()
            .context("failed to flush sandbox proxy response")?;
    }

    for (session_id, mut client) in state.session_executors.drain() {
        stop_session_executor(&session_id, &mut client);
    }
    info!("sandbox proxy stopped after stdin closed");
    Ok(())
}

#[cfg(unix)]
fn run_sandbox_executor(kind: InternalSandboxExecutorKind, snapshot_fd: RawFd) -> Result<()> {
    let bootstrap = read_executor_bootstrap(snapshot_fd)?;
    info!(
        executor_kind = kind.as_str(),
        action = ?bootstrap.snapshot.action,
        session_id = ?bootstrap.snapshot.session_id,
        read_paths = ?bootstrap.snapshot.read_paths,
        write_paths = ?bootstrap.snapshot.write_paths,
        "sandbox executor started"
    );
    match kind {
        InternalSandboxExecutorKind::File => run_sandbox_file_executor(bootstrap.snapshot),
        InternalSandboxExecutorKind::CommandOnce => {
            run_sandbox_command_executor(bootstrap.snapshot, false)
        }
        InternalSandboxExecutorKind::CommandSession => {
            run_sandbox_command_executor(bootstrap.snapshot, true)
        }
    }
}

#[cfg(not(unix))]
fn run_sandbox_executor(_kind: InternalSandboxExecutorKind, _snapshot_fd: i32) -> Result<()> {
    bail!("internal-sandbox executor is only supported on unix")
}

#[cfg(unix)]
fn read_executor_bootstrap(snapshot_fd: RawFd) -> Result<SandboxExecutorBootstrap> {
    let mut snapshot_file = unsafe { fs::File::from_raw_fd(snapshot_fd) };
    let mut raw = Vec::new();
    snapshot_file
        .read_to_end(&mut raw)
        .context("failed to read sandbox executor bootstrap")?;
    drop(snapshot_file);
    serde_json::from_slice(&raw).context("failed to decode sandbox executor bootstrap")
}

fn run_sandbox_file_executor(snapshot: SandboxExecutorPolicySnapshot) -> Result<()> {
    if snapshot.action.is_command() {
        bail!("file executor received command policy snapshot");
    }
    install_executor_landlock(&snapshot, false)
        .context("failed to install sandbox file executor landlock")?;
    run_single_executor_request(|request| handle_sandbox_file_executor_request(&snapshot, request))
}

fn handle_sandbox_file_executor_request(
    snapshot: &SandboxExecutorPolicySnapshot,
    request: SandboxJsonRpcRequest,
) -> SandboxJsonRpcResponse {
    let outcome = match request.method.as_str() {
        "rpc.ping" => Ok(json!({ "status": "ok" })),
        "fs.read_text" => {
            let params = serde_json::from_value::<SandboxReadTextParams>(request.params)
                .context("invalid fs.read_text params");
            params.and_then(|params| {
                if snapshot.action != SandboxExecutorActionKind::Read {
                    bail!("file executor snapshot only allows `{:?}`", snapshot.action);
                }
                let target = PathBuf::from(params.path);
                let content = fs::read_to_string(&target)
                    .with_context(|| format!("failed to read sandbox file {}", target.display()))?;
                serde_json::to_value(SandboxReadTextResult {
                    path: target.display().to_string(),
                    content,
                })
                .context("failed to encode sandbox read result")
            })
        }
        "fs.write_text" => {
            let params = serde_json::from_value::<SandboxWriteTextParams>(request.params)
                .context("invalid fs.write_text params");
            params.and_then(|params| {
                if snapshot.action != SandboxExecutorActionKind::Write {
                    bail!("file executor snapshot only allows `{:?}`", snapshot.action);
                }
                let target = PathBuf::from(params.path);
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).with_context(|| {
                        format!("failed to create sandbox file parent {}", parent.display())
                    })?;
                }
                fs::write(&target, params.content.as_bytes()).with_context(|| {
                    format!("failed to write sandbox file {}", target.display())
                })?;
                serde_json::to_value(SandboxWriteTextResult {
                    path: target.display().to_string(),
                    bytes_written: params.content.len(),
                })
                .context("failed to encode sandbox write result")
            })
        }
        "fs.edit_text" => {
            let params = serde_json::from_value::<SandboxEditTextParams>(request.params)
                .context("invalid fs.edit_text params");
            params.and_then(|params| {
                if snapshot.action != SandboxExecutorActionKind::Edit {
                    bail!("file executor snapshot only allows `{:?}`", snapshot.action);
                }
                let target = PathBuf::from(params.path);
                let content = fs::read_to_string(&target)
                    .with_context(|| format!("failed to read sandbox file {}", target.display()))?;
                let match_count = content.matches(&params.old_text).count();
                if match_count == 0 {
                    bail!("edit tool did not find target text in {}", target.display());
                }
                let updated = content.replacen(&params.old_text, &params.new_text, 1);
                fs::write(&target, updated.as_bytes()).with_context(|| {
                    format!("failed to write sandbox file {}", target.display())
                })?;
                serde_json::to_value(SandboxEditTextResult {
                    match_count,
                    replaced_count: 1,
                })
                .context("failed to encode sandbox edit result")
            })
        }
        other => Err(anyhow::anyhow!(
            "unsupported sandbox file executor method `{other}`"
        )),
    };
    match outcome {
        Ok(result) => SandboxJsonRpcResponse::success(request.id, result),
        Err(error) => SandboxJsonRpcResponse::failure(Some(request.id), -32000, error.to_string()),
    }
}

fn run_single_executor_request(
    handler: impl Fn(SandboxJsonRpcRequest) -> SandboxJsonRpcResponse,
) -> Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = BufWriter::new(stdout.lock());
    loop {
        let mut line = String::new();
        let bytes = reader
            .read_line(&mut line)
            .context("failed to read sandbox executor request")?;
        if bytes == 0 {
            bail!("sandbox executor closed before receiving request");
        }
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<SandboxJsonRpcRequest>(&line) {
            Ok(request) => handler(request),
            Err(error) => SandboxJsonRpcResponse::failure(
                None,
                -32700,
                format!("failed to parse request: {error}"),
            ),
        };
        let raw = serde_json::to_string(&response)
            .context("failed to serialize sandbox executor response")?;
        writer
            .write_all(raw.as_bytes())
            .context("failed to write sandbox executor response")?;
        writer
            .write_all(b"\n")
            .context("failed to terminate sandbox executor response")?;
        writer
            .flush()
            .context("failed to flush sandbox executor response")?;
        return Ok(());
    }
}

fn run_sandbox_command_executor(
    snapshot: SandboxExecutorPolicySnapshot,
    _force_session_executor: bool,
) -> Result<()> {
    if !snapshot.action.is_command() {
        bail!("command executor received file policy snapshot");
    }
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = BufWriter::new(stdout.lock());
    let command_runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to build sandbox command executor runtime")?;
    let command_sessions = CommandSessionManager::new();
    let launch_options = build_executor_launch_options(&snapshot)?;
    let active_session_id = snapshot.session_id.clone();
    let mut landlock_installed = false;

    loop {
        let mut line = String::new();
        let bytes = reader
            .read_line(&mut line)
            .context("failed to read sandbox command executor request")?;
        if bytes == 0 {
            break;
        }
        if line.trim().is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<SandboxJsonRpcRequest>(&line) {
            Ok(request) => handle_sandbox_command_executor_request(
                &snapshot,
                &command_runtime,
                &command_sessions,
                &launch_options,
                active_session_id.as_deref(),
                &mut landlock_installed,
                request,
            ),
            Err(error) => SandboxJsonRpcResponse::failure(
                None,
                -32700,
                format!("failed to parse request: {error}"),
            ),
        };
        let raw = serde_json::to_string(&response)
            .context("failed to serialize sandbox command executor response")?;
        writer
            .write_all(raw.as_bytes())
            .context("failed to write sandbox command executor response")?;
        writer
            .write_all(b"\n")
            .context("failed to terminate sandbox command executor response")?;
        writer
            .flush()
            .context("failed to flush sandbox command executor response")?;
    }
    Ok(())
}

fn handle_sandbox_command_executor_request(
    snapshot: &SandboxExecutorPolicySnapshot,
    command_runtime: &tokio::runtime::Runtime,
    command_sessions: &CommandSessionManager,
    launch_options: &CommandLaunchOptions,
    active_session_id: Option<&str>,
    landlock_installed: &mut bool,
    request: SandboxJsonRpcRequest,
) -> SandboxJsonRpcResponse {
    let outcome = match request.method.as_str() {
        "rpc.ping" => Ok(json!({ "status": "ok" })),
        "command.exec" => {
            let params = serde_json::from_value::<SandboxCommandExecParams>(request.params)
                .context("invalid command.exec params");
            params.and_then(|params| {
                if !*landlock_installed {
                    install_executor_landlock(snapshot, params.request.tty)
                        .context("failed to install sandbox command executor landlock")?;
                    *landlock_installed = true;
                }
                command_runtime
                    .block_on(command_sessions.exec_command_from_context_with_options(
                        params.thread_id.as_deref(),
                        params.request,
                        launch_options.clone(),
                    ))
                    .and_then(|result| {
                        serde_json::to_value(result)
                            .context("failed to encode sandbox command.exec result")
                    })
            })
        }
        "command.write_stdin" => {
            let params = serde_json::from_value::<SandboxCommandWriteParams>(request.params)
                .context("invalid command.write_stdin params");
            params.and_then(|params| {
                command_runtime
                    .block_on(
                        command_sessions
                            .write_stdin_from_context(params.thread_id.as_deref(), params.request),
                    )
                    .and_then(|result| {
                        serde_json::to_value(result)
                            .context("failed to encode sandbox command.write_stdin result")
                    })
            })
        }
        "command.snapshot" => active_session_id
            .and_then(|session_id| {
                command_runtime
                    .block_on(command_sessions.export_task_snapshots())
                    .into_iter()
                    .find(|snapshot| snapshot.session_id == session_id)
            })
            .map(|snapshot| {
                serde_json::to_value(snapshot).context("failed to encode sandbox command snapshot")
            })
            .unwrap_or_else(|| {
                Err(anyhow::anyhow!(
                    "sandbox command executor has no active session"
                ))
            }),
        other => Err(anyhow::anyhow!(
            "unsupported sandbox command executor method `{other}`"
        )),
    };
    match outcome {
        Ok(result) => SandboxJsonRpcResponse::success(request.id, result),
        Err(error) => SandboxJsonRpcResponse::failure(Some(request.id), -32000, error.to_string()),
    }
}

fn build_executor_launch_options(
    snapshot: &SandboxExecutorPolicySnapshot,
) -> Result<CommandLaunchOptions> {
    let Some(profile) = snapshot.final_command_profile.as_ref() else {
        return Ok(CommandLaunchOptions::default());
    };
    let helper_executable =
        resolve_sandbox_helper_executable().context("failed to resolve sandbox command helper")?;
    Ok(CommandLaunchOptions {
        exec_helper: Some(CommandExecHelperSpec {
            helper_executable,
            workspace_root: snapshot
                .read_paths
                .first()
                .cloned()
                .unwrap_or_else(|| PathBuf::from(SANDBOX_WORKSPACE_MOUNT)),
            profile_json: serde_json::to_string(profile)
                .context("failed to serialize sandbox command profile")?,
        }),
        session_id_override: snapshot.session_id.clone(),
    })
}

#[cfg(unix)]
fn run_sandbox_exec(
    _workspace_root: PathBuf,
    profile: SandboxCommandChildProfilePlan,
    workdir: Option<PathBuf>,
    program: String,
    args: Vec<String>,
) -> Result<()> {
    install_final_command_seccomp(&profile).with_context(|| {
        format!(
            "failed to install sandbox final seccomp `{}`",
            profile.name()
        )
    })?;
    if let Some(workdir) = workdir.as_deref() {
        env::set_current_dir(workdir).with_context(|| {
            format!(
                "failed to enter sandbox command workdir `{}`",
                workdir.display()
            )
        })?;
    }

    let error = Command::new(&program).args(&args).exec();
    bail!("failed to exec sandbox command `{program}`: {error}");
}

#[cfg(not(unix))]
fn run_sandbox_exec(
    _workspace_root: PathBuf,
    _profile: SandboxCommandChildProfilePlan,
    _workdir: Option<PathBuf>,
    _program: String,
    _args: Vec<String>,
) -> Result<()> {
    bail!("internal-sandbox exec is only supported on unix")
}

fn handle_sandbox_proxy_request(
    state: &mut SandboxProxyState,
    request: SandboxJsonRpcRequest,
) -> SandboxJsonRpcResponse {
    debug!(
        request_id = request.id,
        method = request.method,
        "handling sandbox proxy request"
    );
    let outcome = match request.method.as_str() {
        "rpc.ping" => Ok(json!({ "status": "ok" })),
        "policy.set_command_profile" => {
            let params = serde_json::from_value::<SandboxPolicyProfileParams>(request.params)
                .context("invalid policy.set_command_profile params");
            params.and_then(|params| {
                if let Some(kernel_enforcement) = state.kernel_enforcement.as_ref() {
                    kernel_enforcement.command_profile(Some(&params.command_profile))?;
                }
                info!(
                    command_profile = params.command_profile,
                    "updated sandbox executor policy source"
                );
                state
                    .executor_policy_source
                    .set_command_profile(params.command_profile.clone());
                Ok(json!({
                    "command_profile": params.command_profile,
                }))
            })
        }
        "fs.write_text" => {
            let params = serde_json::from_value::<SandboxWriteTextParams>(request.params)
                .context("invalid fs.write_text params");
            params.and_then(|params| {
                let target = state
                    .path_policy
                    .resolve_request_path(Path::new(&params.path))?;
                let result = execute_file_request::<SandboxWriteTextResult>(
                    state,
                    SandboxExecutorActionKind::Write,
                    "fs.write_text",
                    SandboxWriteTextParams {
                        path: target.display().to_string(),
                        content: params.content,
                    },
                    &target,
                )?;
                serde_json::to_value(result).context("failed to encode sandbox file write result")
            })
        }
        "fs.read_text" => {
            let params = serde_json::from_value::<SandboxReadTextParams>(request.params)
                .context("invalid fs.read_text params");
            params.and_then(|params| {
                let target = state
                    .path_policy
                    .resolve_request_path(Path::new(&params.path))?;
                let result = execute_file_request::<SandboxReadTextResult>(
                    state,
                    SandboxExecutorActionKind::Read,
                    "fs.read_text",
                    SandboxReadTextParams {
                        path: target.display().to_string(),
                    },
                    &target,
                )?;
                serde_json::to_value(result).context("failed to encode sandbox file read result")
            })
        }
        "fs.edit_text" => {
            let params = serde_json::from_value::<SandboxEditTextParams>(request.params)
                .context("invalid fs.edit_text params");
            params.and_then(|params| {
                let target = state
                    .path_policy
                    .resolve_request_path(Path::new(&params.path))?;
                let result = execute_file_request::<SandboxEditTextResult>(
                    state,
                    SandboxExecutorActionKind::Edit,
                    "fs.edit_text",
                    SandboxEditTextParams {
                        path: target.display().to_string(),
                        old_text: params.old_text,
                        new_text: params.new_text,
                    },
                    &target,
                )?;
                serde_json::to_value(result).context("failed to encode sandbox file edit result")
            })
        }
        "command.exec" => {
            let params = serde_json::from_value::<SandboxCommandExecParams>(request.params)
                .context("invalid command.exec params");
            params.and_then(|params| {
                let mut request = params.request;
                request.workdir = request
                    .workdir
                    .as_deref()
                    .map(|path| state.path_policy.resolve_request_path(path))
                    .transpose()?;
                execute_command_request(
                    state,
                    params.thread_id.as_deref(),
                    params.command_profile.as_deref(),
                    request,
                )
                .and_then(|result| {
                    serde_json::to_value(result)
                        .context("failed to encode sandbox command.exec result")
                })
            })
        }
        "command.write_stdin" => {
            let params = serde_json::from_value::<SandboxCommandWriteParams>(request.params)
                .context("invalid command.write_stdin params");
            params.and_then(|params| {
                write_command_stdin_via_executor(state, params.thread_id.as_deref(), params.request)
                    .and_then(|result| {
                        serde_json::to_value(result)
                            .context("failed to encode sandbox command.write_stdin result")
                    })
            })
        }
        "command.list_unread_tasks" => {
            let params = serde_json::from_value::<SandboxCommandListParams>(request.params)
                .context("invalid command.list_unread_tasks params");
            params.and_then(|params| {
                let tasks = list_unread_tasks_via_executor(state, params.thread_id.as_deref());
                serde_json::to_value(tasks)
                    .context("failed to encode sandbox command.list_unread_tasks result")
            })
        }
        other => Err(anyhow::anyhow!(
            "unsupported sandbox jsonrpc method `{other}`"
        )),
    };

    match outcome {
        Ok(result) => SandboxJsonRpcResponse::success(request.id, result),
        Err(error) => SandboxJsonRpcResponse::failure(Some(request.id), -32000, error.to_string()),
    }
}

fn configure_bubblewrap_command(
    command: &mut Command,
    current_executable_dir: &Path,
    current_executable_name: &OsString,
    workspace_root: &Path,
    policy: &SandboxPolicyConfig,
    kernel_enforcement: &SandboxKernelEnforcementPlan,
    enforcement_plan_json: &str,
) {
    command
        .arg("--die-with-parent")
        .arg("--clearenv")
        .arg("--proc")
        .arg("/proc")
        .arg("--dev")
        .arg("/dev");

    if kernel_enforcement.namespace().user() {
        command
            .arg("--unshare-user")
            .arg("--uid")
            .arg("0")
            .arg("--gid")
            .arg("0");
    }
    if kernel_enforcement.namespace().ipc() {
        command.arg("--unshare-ipc");
    }
    if kernel_enforcement.namespace().pid() {
        command.arg("--unshare-pid");
    }
    if kernel_enforcement.namespace().uts() {
        command
            .arg("--unshare-uts")
            .arg("--hostname")
            .arg("openjarvis-sandbox");
    }
    if kernel_enforcement.namespace().net() {
        command.arg("--unshare-net");
    }

    for ro_dir in [
        Path::new("/usr"),
        Path::new("/bin"),
        Path::new("/lib"),
        Path::new("/lib64"),
        Path::new("/etc"),
    ] {
        if ro_dir.exists() {
            command.arg("--ro-bind").arg(ro_dir).arg(ro_dir);
        }
    }

    if Path::new(SANDBOX_TMP_DIR).exists() {
        command
            .arg("--bind")
            .arg(Path::new(SANDBOX_TMP_DIR))
            .arg(Path::new(SANDBOX_TMP_DIR));
    }

    command
        .arg("--ro-bind")
        .arg(current_executable_dir)
        .arg("/openjarvis-bin")
        .arg("--bind")
        .arg(workspace_root)
        .arg(SANDBOX_WORKSPACE_MOUNT)
        .arg("--chdir")
        .arg(SANDBOX_WORKSPACE_MOUNT)
        .arg("--setenv")
        .arg("PATH")
        .arg("/usr/bin:/bin:/openjarvis-bin")
        .arg("--")
        .arg(Path::new("/openjarvis-bin").join(Path::new(current_executable_name)))
        .arg("internal-sandbox")
        .arg("proxy")
        .arg("--workspace-root")
        .arg(SANDBOX_WORKSPACE_MOUNT)
        .arg("--enforcement-plan-json")
        .arg(enforcement_plan_json);

    for restricted_path in policy.restricted_host_paths() {
        command
            .arg("--restricted-host-path")
            .arg(restricted_path.display().to_string());
    }
    if policy.allow_parent_access() {
        command.arg("--allow-parent-access");
    }
}

fn resolve_command_path(command: &Path) -> Result<PathBuf> {
    if command.is_absolute() || command.components().count() > 1 {
        if command.exists() {
            return Ok(command.to_path_buf());
        }
        bail!("command `{}` does not exist", command.display());
    }

    let Some(path_env) = env::var_os("PATH") else {
        bail!(
            "PATH is not available when resolving `{}`",
            command.display()
        );
    };
    for segment in env::split_paths(&path_env) {
        let candidate = segment.join(command);
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    bail!("command `{}` was not found in PATH", command.display())
}

fn resolve_sandbox_helper_executable() -> Result<PathBuf> {
    let current_executable = env::current_exe().context("failed to resolve current executable")?;
    let current_name = current_executable
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if current_name == env!("CARGO_PKG_NAME") {
        return Ok(current_executable);
    }

    let candidate = current_executable
        .parent()
        .and_then(Path::parent)
        .map(|debug_dir| debug_dir.join(env!("CARGO_PKG_NAME")));
    if let Some(candidate) = candidate
        && candidate.exists()
    {
        return Ok(candidate);
    }

    Ok(current_executable)
}

fn resolve_config_path(path: &Path, workspace_root: &Path) -> PathBuf {
    let expanded = expand_tilde(path);
    if expanded.is_absolute() {
        return normalize_path(&expanded);
    }
    normalize_path(&workspace_root.join(expanded))
}

fn expand_tilde(path: &Path) -> PathBuf {
    let raw = path.as_os_str().to_string_lossy();
    if !raw.starts_with("~/") && raw != "~" {
        return path.to_path_buf();
    }

    let Some(home) = env::var_os("HOME") else {
        return path.to_path_buf();
    };
    let mut expanded = PathBuf::from(home);
    if raw.len() > 2 {
        expanded.push(&raw[2..]);
    }
    expanded
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    let mut parts: Vec<OsString> = Vec::new();
    let mut is_absolute = false;

    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => is_absolute = true,
            Component::CurDir => {}
            Component::ParentDir => {
                if let Some(last) = parts.last()
                    && last != ".."
                {
                    parts.pop();
                    continue;
                }
                if !is_absolute {
                    parts.push(OsString::from(".."));
                }
            }
            Component::Normal(part) => parts.push(part.to_os_string()),
        }
    }

    if is_absolute {
        normalized.push(Path::new("/"));
    }
    for part in parts {
        normalized.push(part);
    }

    if normalized.as_os_str().is_empty() && is_absolute {
        PathBuf::from("/")
    } else if normalized.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized
    }
}

fn path_is_within(path: &Path, root: &Path) -> bool {
    let normalized_path = normalize_path(path);
    let normalized_root = normalize_path(root);
    normalized_path == normalized_root || normalized_path.starts_with(&normalized_root)
}
