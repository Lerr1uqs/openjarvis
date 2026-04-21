//! Command-line parsing for the OpenJarvis binary and local protocol helpers.

use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Parsed command-line arguments for the main OpenJarvis binary.
#[derive(Debug, Clone, Parser)]
#[command(name = "openjarvis")]
pub struct OpenJarvisCli {
    /// Force richer debug logging on stderr for the current process.
    #[arg(long, global = true)]
    pub debug: bool,
    /// Force ANSI colors on stderr logs for the current process.
    #[arg(long = "log-color", global = true)]
    pub log_color: bool,
    /// Load demo-only builtin MCP servers for local verification.
    #[arg(long, global = true)]
    pub builtin_mcp: bool,
    /// Test-only: preload one or more local skills from `.openjarvis/skills` into this process so
    /// the running agent can use them.
    ///
    /// This flag is intended for local verification and smoke tests. It does not print the skill
    /// body; it starts the normal runtime and restricts the enabled local skills to the selected
    /// names for the current process.
    #[arg(long = "load-skill", global = true, value_name = "NAME", hide = true)]
    pub load_skills: Vec<String>,
    /// Optional internal subcommands reserved for local protocol helpers.
    #[command(subcommand)]
    pub command: Option<OpenJarvisCommand>,
}

impl OpenJarvisCli {
    /// Return the parsed internal MCP command when the binary is running in helper mode.
    ///
    /// # 示例
    /// ```rust
    /// use clap::Parser;
    /// use openjarvis::cli::OpenJarvisCli;
    ///
    /// let cli = OpenJarvisCli::parse_from(["openjarvis", "--builtin-mcp"]);
    /// assert!(cli.command.is_none());
    /// assert!(cli.builtin_mcp);
    /// assert!(!cli.debug);
    /// ```
    pub fn internal_mcp_command(&self) -> Option<&InternalMcpCommand> {
        match &self.command {
            Some(OpenJarvisCommand::InternalMcp(arguments)) => Some(&arguments.command),
            _ => None,
        }
    }

    /// Return the parsed internal browser command when the binary is running in helper mode.
    ///
    /// # 示例
    /// ```rust
    /// use clap::Parser;
    /// use openjarvis::cli::{InternalBrowserCommand, OpenJarvisCli};
    ///
    /// let cli = OpenJarvisCli::parse_from([
    ///     "openjarvis",
    ///     "internal-browser",
    ///     "mock-sidecar",
    /// ]);
    /// assert!(matches!(
    ///     cli.internal_browser_command(),
    ///     Some(InternalBrowserCommand::MockSidecar)
    /// ));
    /// ```
    pub fn internal_browser_command(&self) -> Option<&InternalBrowserCommand> {
        match &self.command {
            Some(OpenJarvisCommand::InternalBrowser(arguments)) => Some(&arguments.command),
            _ => None,
        }
    }

    /// Return the parsed internal `obswiki` command when the binary is running in helper mode.
    pub fn internal_obswiki_command(&self) -> Option<&InternalObswikiCommand> {
        match &self.command {
            Some(OpenJarvisCommand::InternalObswiki(arguments)) => Some(&arguments.command),
            _ => None,
        }
    }

    /// Return the parsed internal sandbox command when the binary is running in helper mode.
    pub fn internal_sandbox_command(&self) -> Option<&InternalSandboxCommand> {
        match &self.command {
            Some(OpenJarvisCommand::InternalSandbox(arguments)) => Some(&arguments.command),
            _ => None,
        }
    }

    /// Return the parsed top-level skill command when present.
    ///
    /// # 示例
    /// ```rust
    /// use clap::Parser;
    /// use openjarvis::cli::{OpenJarvisCli, SkillCommand};
    ///
    /// let cli = OpenJarvisCli::parse_from(["openjarvis", "skill", "install", "acpx"]);
    /// assert!(matches!(
    ///     cli.skill_command(),
    ///     Some(SkillCommand::Install { name }) if name == "acpx"
    /// ));
    /// ```
    pub fn skill_command(&self) -> Option<&SkillCommand> {
        match &self.command {
            Some(OpenJarvisCommand::Skill(arguments)) => Some(&arguments.command),
            _ => None,
        }
    }
}

/// Top-level OpenJarvis subcommands.
#[derive(Debug, Clone, Subcommand)]
pub enum OpenJarvisCommand {
    /// Local skill management commands.
    #[command(name = "skill")]
    Skill(SkillArgs),
    /// Internal demo-only MCP helpers used by tests and local protocol verification.
    #[command(name = "internal-mcp", hide = true)]
    InternalMcp(InternalMcpArgs),
    /// Internal browser helpers used by local smoke verification and tests.
    #[command(name = "internal-browser", hide = true)]
    InternalBrowser(InternalBrowserArgs),
    /// Internal obswiki helpers used by local vault verification and tests.
    #[command(name = "internal-obswiki", hide = true)]
    InternalObswiki(InternalObswikiArgs),
    /// Internal sandbox helpers used by the bubblewrap runtime.
    #[command(name = "internal-sandbox", hide = true)]
    InternalSandbox(InternalSandboxArgs),
}

impl OpenJarvisCommand {
    /// Return the stable top-level subcommand name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Skill(_) => "skill",
            Self::InternalMcp(_) => "internal-mcp",
            Self::InternalBrowser(_) => "internal-browser",
            Self::InternalObswiki(_) => "internal-obswiki",
            Self::InternalSandbox(_) => "internal-sandbox",
        }
    }
}

/// Arguments for the public `skill` command namespace.
#[derive(Debug, Clone, Args)]
pub struct SkillArgs {
    #[command(subcommand)]
    pub command: SkillCommand,
}

/// Public skill management commands.
#[derive(Debug, Clone, Subcommand, PartialEq, Eq)]
pub enum SkillCommand {
    /// Install one curated local skill into the current workspace.
    #[command(name = "install")]
    Install {
        /// Stable curated skill name, for example `acpx`.
        name: String,
    },
    /// Uninstall one local skill from the current workspace.
    #[command(name = "uninstall")]
    Uninstall {
        /// Exact local skill name to remove, for example `acpx`.
        name: String,
    },
}

/// Arguments for the hidden `internal-mcp` helper namespace.
#[derive(Debug, Clone, Args)]
pub struct InternalMcpArgs {
    #[command(subcommand)]
    pub command: InternalMcpCommand,
}

/// Demo-only internal MCP server commands.
#[derive(Debug, Clone, Subcommand)]
pub enum InternalMcpCommand {
    /// Run the demo MCP server over stdio.
    #[command(name = "demo-stdio")]
    DemoStdio,
    /// Run the demo MCP server over Streamable HTTP.
    #[command(name = "demo-http")]
    DemoHttp {
        /// Bind address for the demo Streamable HTTP server.
        #[arg(long, default_value = "127.0.0.1:39090")]
        bind: String,
    },
}

impl InternalMcpCommand {
    /// Return the bind address when the command is `demo-http`.
    pub fn bind_address(&self) -> Option<&str> {
        match self {
            Self::DemoStdio => None,
            Self::DemoHttp { bind } => Some(bind.as_str()),
        }
    }
}

/// Arguments for the hidden `internal-browser` helper namespace.
#[derive(Debug, Clone, Args)]
pub struct InternalBrowserArgs {
    #[command(subcommand)]
    pub command: InternalBrowserCommand,
}

/// Arguments for the hidden `internal-obswiki` helper namespace.
#[derive(Debug, Clone, Args)]
pub struct InternalObswikiArgs {
    #[command(subcommand)]
    pub command: InternalObswikiCommand,
}

/// Arguments for the hidden `internal-sandbox` helper namespace.
#[derive(Debug, Clone, Args)]
pub struct InternalSandboxArgs {
    #[command(subcommand)]
    pub command: InternalSandboxCommand,
}

/// Browser open mode used by hidden browser helpers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum InternalBrowserMode {
    Launch,
    Attach,
}

/// Demo-only internal browser helper commands.
#[derive(Debug, Clone, Subcommand)]
pub enum InternalBrowserCommand {
    /// Run a manual smoke flow through the browser sidecar.
    #[command(name = "smoke")]
    Smoke {
        /// Target URL used by the smoke flow.
        #[arg(long)]
        url: String,
        /// Browser session source mode used before the smoke actions run.
        #[arg(long = "mode", value_enum, default_value_t = InternalBrowserMode::Launch)]
        mode: InternalBrowserMode,
        /// Explicit CDP endpoint used when `--mode attach`.
        #[arg(long)]
        cdp_endpoint: Option<String>,
        /// Run the browser in headless mode.
        #[arg(long, default_value_t = false)]
        headless: bool,
        /// Optional root directory used to retain smoke artifacts.
        #[arg(long)]
        output_dir: Option<PathBuf>,
        /// Override the Node.js executable used to launch the sidecar.
        #[arg(long, default_value = "node")]
        node_bin: String,
        /// Override the browser sidecar script path.
        #[arg(long)]
        script_path: Option<PathBuf>,
        /// Optional explicit Chrome executable path.
        #[arg(long)]
        chrome_path: Option<PathBuf>,
        /// Optional cookies state file used by helper-driven browser session reuse.
        #[arg(long)]
        cookies_state_file: Option<PathBuf>,
        /// Auto-load cookies from the configured state file when launch-mode open starts.
        #[arg(long, default_value_t = false)]
        load_cookies_on_open: bool,
        /// Auto-save cookies into the configured state file when close runs.
        #[arg(long, default_value_t = false)]
        save_cookies_on_close: bool,
    },
    /// Run a structured multi-step browser script from a JSON file.
    #[command(name = "script")]
    Script {
        /// JSON file containing a list of browser actions to execute in order.
        #[arg(long)]
        steps_file: PathBuf,
        /// Default browser session source mode used by helper-triggered open.
        #[arg(long = "mode", value_enum, default_value_t = InternalBrowserMode::Launch)]
        mode: InternalBrowserMode,
        /// Explicit CDP endpoint used when `--mode attach`.
        #[arg(long)]
        cdp_endpoint: Option<String>,
        /// Run the browser in headless mode.
        #[arg(long, default_value_t = false)]
        headless: bool,
        /// Optional root directory used to retain script artifacts.
        #[arg(long)]
        output_dir: Option<PathBuf>,
        /// Override the Node.js executable used to launch the sidecar.
        #[arg(long, default_value = "node")]
        node_bin: String,
        /// Override the browser sidecar script path.
        #[arg(long)]
        script_path: Option<PathBuf>,
        /// Optional explicit Chrome executable path.
        #[arg(long)]
        chrome_path: Option<PathBuf>,
        /// Optional cookies state file used by helper-driven browser session reuse.
        #[arg(long)]
        cookies_state_file: Option<PathBuf>,
        /// Auto-load cookies from the configured state file when launch-mode open starts.
        #[arg(long, default_value_t = false)]
        load_cookies_on_open: bool,
        /// Auto-save cookies into the configured state file when close runs.
        #[arg(long, default_value_t = false)]
        save_cookies_on_close: bool,
    },
    /// Test-only mock sidecar that speaks the same JSON-line protocol as the Node sidecar.
    #[command(name = "mock-sidecar", hide = true)]
    MockSidecar,
}

/// Demo-only internal obswiki helper commands.
#[derive(Debug, Clone, Subcommand)]
pub enum InternalObswikiCommand {
    /// Execute one prompt directly against the `obswiki` child-thread profile.
    #[command(name = "prompt")]
    Prompt {
        /// Prompt content that should be sent to the `obswiki` child thread.
        #[arg(long)]
        content: String,
    },
}

/// Demo-only internal sandbox helper commands.
#[derive(Debug, Clone, Subcommand)]
pub enum InternalSandboxCommand {
    /// Run the JSON-RPC proxy used by the bubblewrap sandbox backend.
    #[command(name = "proxy")]
    Proxy {
        /// Host-visible synchronized workspace root mounted into the sandbox.
        #[arg(long)]
        workspace_root: PathBuf,
        /// Structured sandbox kernel enforcement plan serialized as JSON.
        #[arg(long, hide = true)]
        enforcement_plan_json: Option<String>,
        /// Restricted host paths that the proxy must refuse to access.
        #[arg(long = "restricted-host-path")]
        restricted_host_paths: Vec<PathBuf>,
        /// Allow JSON-RPC paths to escape above the synchronized workspace root.
        #[arg(long, default_value_t = false)]
        allow_parent_access: bool,
    },
    /// Run one hidden command-child helper that installs enforcement before `execve`.
    #[command(name = "exec", hide = true)]
    Exec {
        /// Host-visible synchronized workspace root mounted into the sandbox.
        #[arg(long)]
        workspace_root: PathBuf,
        /// Structured command-child enforcement profile serialized as JSON.
        #[arg(long)]
        profile_json: String,
        /// Optional working directory that should be entered before executing the real command.
        #[arg(long)]
        workdir: Option<PathBuf>,
        /// Real program that should be executed after installing enforcement.
        #[arg(long)]
        program: String,
        /// Arguments forwarded to the real program.
        #[arg(long = "arg", allow_hyphen_values = true)]
        args: Vec<String>,
    },
}
