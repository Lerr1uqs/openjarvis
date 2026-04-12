//! Host OS and shell facts injected into the stable thread `System` prefix.

use chrono::Utc;
use std::{ffi::OsStr, path::Path};
use tracing::info;

/// Stable shell and host environment snapshot injected into each thread initialization prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellEnv {
    os_family: String,
    default_shell: String,
    command_execution_shell: String,
    path_style: String,
}

impl ShellEnv {
    /// Detect the current host OS and shell facts for thread initialization.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::ShellEnv;
    ///
    /// let shell_env = ShellEnv::detect();
    /// assert!(!shell_env.render_prompt().is_empty());
    /// ```
    pub fn detect() -> Self {
        let shell_env = Self {
            os_family: normalize_os_family(std::env::consts::OS),
            default_shell: detect_default_shell(),
            command_execution_shell: default_command_execution_shell(),
            path_style: default_path_style(),
        };
        info!(
            detected_at = %Utc::now(),
            os_family = %shell_env.os_family,
            default_shell = %shell_env.default_shell,
            command_execution_shell = %shell_env.command_execution_shell,
            path_style = %shell_env.path_style,
            "built shell env snapshot"
        );
        shell_env
    }

    /// Render the stable system prompt injected into the thread prefix.
    pub fn render_prompt(&self) -> String {
        format!(
            "Runtime environment for this thread:\n- os_family: {}\n- default_shell: {}\n- command_execution_shell: {}\n- path_style: {}\nOnly rely on these facts. If a field is `unknown`, do not guess a replacement shell or OS.",
            self.os_family, self.default_shell, self.command_execution_shell, self.path_style
        )
    }

    #[cfg(test)]
    pub(crate) fn from_facts(
        os_family: impl Into<String>,
        default_shell: impl Into<String>,
        command_execution_shell: impl Into<String>,
        path_style: impl Into<String>,
    ) -> Self {
        Self {
            os_family: os_family.into(),
            default_shell: default_shell.into(),
            command_execution_shell: command_execution_shell.into(),
            path_style: path_style.into(),
        }
    }
}

fn normalize_os_family(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "linux" => "linux".to_string(),
        "macos" => "macos".to_string(),
        "windows" => "windows".to_string(),
        other if !other.is_empty() => other.to_string(),
        _ => "unknown".to_string(),
    }
}

fn detect_default_shell() -> String {
    #[cfg(windows)]
    {
        basename_or_unknown(std::env::var_os("COMSPEC").as_deref())
    }

    #[cfg(not(windows))]
    {
        basename_or_unknown(std::env::var_os("SHELL").as_deref())
    }
}

fn default_command_execution_shell() -> String {
    #[cfg(windows)]
    {
        "powershell".to_string()
    }

    #[cfg(not(windows))]
    {
        "sh".to_string()
    }
}

fn default_path_style() -> String {
    #[cfg(windows)]
    {
        "windows".to_string()
    }

    #[cfg(not(windows))]
    {
        "posix".to_string()
    }
}

fn basename_or_unknown(value: Option<&OsStr>) -> String {
    let Some(value) = value else {
        return "unknown".to_string();
    };
    let shell_name = Path::new(value)
        .file_name()
        .map(|segment| segment.to_string_lossy().trim().to_string())
        .filter(|segment| !segment.is_empty());
    shell_name.unwrap_or_else(|| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::ShellEnv;

    #[test]
    fn shell_env_renders_unknown_shell_explicitly() {
        let shell_env = ShellEnv::from_facts("linux", "unknown", "sh", "posix");
        let prompt = shell_env.render_prompt();

        assert!(prompt.contains("default_shell: unknown"));
        assert!(prompt.contains("command_execution_shell: sh"));
    }
}
