//! Thread-agent kinds, bundled prompt templates, and default tool bindings.

use super::normalize_loaded_toolsets;
use serde::{Deserialize, Serialize};

/// Bundled main-thread system prompt template used for the default assistant thread.
pub const DEFAULT_ASSISTANT_SYSTEM_PROMPT: &str =
    include_str!("../../resources/prompts/thread_agent/main.md");

/// Bundled browser-thread system prompt template used for browser worker threads.
pub const DEFAULT_BROWSER_THREAD_SYSTEM_PROMPT: &str =
    include_str!("../../resources/prompts/thread_agent/browser.md");

/// Closed set of thread agent kinds used to select stable initialization profiles.
#[derive(
    Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum ThreadAgentKind {
    #[default]
    Main,
    Browser,
}

impl ThreadAgentKind {
    /// Return the stable label used by logs and persisted thread state.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::thread::ThreadAgentKind;
    ///
    /// assert_eq!(ThreadAgentKind::Main.as_str(), "main");
    /// assert_eq!(ThreadAgentKind::Browser.as_str(), "browser");
    /// ```
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Browser => "browser",
        }
    }

    /// Return the bundled system prompt template bound to this thread agent kind.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::thread::{DEFAULT_ASSISTANT_SYSTEM_PROMPT, ThreadAgentKind};
    ///
    /// assert_eq!(
    ///     ThreadAgentKind::Main.system_prompt().trim(),
    ///     DEFAULT_ASSISTANT_SYSTEM_PROMPT.trim()
    /// );
    /// assert!(!ThreadAgentKind::Browser.system_prompt().trim().is_empty());
    /// ```
    pub fn system_prompt(self) -> &'static str {
        match self {
            Self::Main => DEFAULT_ASSISTANT_SYSTEM_PROMPT.trim(),
            Self::Browser => DEFAULT_BROWSER_THREAD_SYSTEM_PROMPT.trim(),
        }
    }

    pub(crate) fn default_bound_toolsets(self) -> Vec<String> {
        match self {
            Self::Main => Vec::new(),
            Self::Browser => vec!["browser".to_string()],
        }
    }
}

/// Persisted thread agent profile that owns the thread role and its default tool bindings.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadAgent {
    #[serde(default)]
    pub kind: ThreadAgentKind,
    #[serde(default)]
    pub bound_toolsets: Vec<String>,
}

impl ThreadAgent {
    /// Build the persisted default profile for one thread agent kind.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::thread::{ThreadAgent, ThreadAgentKind};
    ///
    /// let browser = ThreadAgent::from_kind(ThreadAgentKind::Browser);
    /// assert_eq!(browser.kind, ThreadAgentKind::Browser);
    /// assert_eq!(browser.bound_toolsets, vec!["browser".to_string()]);
    /// ```
    pub fn from_kind(kind: ThreadAgentKind) -> Self {
        Self::new(kind, kind.default_bound_toolsets())
    }

    /// Build one explicit thread agent profile with normalized toolset bindings.
    pub fn new(kind: ThreadAgentKind, bound_toolsets: Vec<String>) -> Self {
        Self {
            kind,
            bound_toolsets: normalize_loaded_toolsets(bound_toolsets),
        }
    }

    /// Return the bundled system prompt selected by this persisted thread agent profile.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::thread::{ThreadAgent, ThreadAgentKind};
    ///
    /// let browser = ThreadAgent::from_kind(ThreadAgentKind::Browser);
    /// assert_eq!(browser.system_prompt(), ThreadAgentKind::Browser.system_prompt());
    /// ```
    pub fn system_prompt(&self) -> &'static str {
        self.kind.system_prompt()
    }
}
