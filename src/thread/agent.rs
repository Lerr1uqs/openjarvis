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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SubagentCatalogEntry {
    pub kind: ThreadAgentKind,
    pub subagent_key: &'static str,
    pub role_summary: &'static str,
    pub when_to_use: &'static str,
    pub when_not_to_use: &'static str,
}

const AVAILABLE_SUBAGENT_CATALOG: [SubagentCatalogEntry; 1] = [SubagentCatalogEntry {
    kind: ThreadAgentKind::Browser,
    subagent_key: "browser",
    role_summary: "负责浏览器观察与页面交互，在独立 child thread 中完成多步网页操作。",
    when_to_use: "任务明确需要打开页面、观察网页状态、执行连续页面动作，或者希望复用浏览器 child thread 的上下文。",
    when_not_to_use: "主线程已经能直接完成任务，或者只需要一次简单工具调用时，不要额外启动 browser subagent。",
}];

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

    /// Resolve one subagent profile key back to its thread-agent kind.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::thread::ThreadAgentKind;
    ///
    /// assert_eq!(
    ///     ThreadAgentKind::from_subagent_key("browser"),
    ///     Some(ThreadAgentKind::Browser)
    /// );
    /// assert_eq!(ThreadAgentKind::from_subagent_key("unknown"), None);
    /// ```
    pub fn from_subagent_key(value: &str) -> Option<Self> {
        let value = value.trim();
        Self::available_subagent_catalog()
            .iter()
            .find(|entry| entry.subagent_key == value)
            .map(|entry| entry.kind)
    }

    /// Return the stable subagent profile key owned by this thread-agent kind.
    pub fn subagent_key(self) -> Option<&'static str> {
        Self::available_subagent_catalog()
            .iter()
            .find(|entry| entry.kind == self)
            .map(|entry| entry.subagent_key)
    }

    /// Return whether this thread-agent kind is one subagent profile.
    pub fn is_subagent(self) -> bool {
        self.subagent_key().is_some()
    }

    pub(crate) fn available_subagent_catalog() -> &'static [SubagentCatalogEntry] {
        &AVAILABLE_SUBAGENT_CATALOG
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
