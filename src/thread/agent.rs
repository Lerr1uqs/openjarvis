//! Thread-agent kinds, bundled prompt templates,
//! and capability profile bindings loaded from one
//! static predefined agent catalog.

use super::{Feature, Features, normalize_loaded_toolsets};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, sync::OnceLock};
use tracing::debug;

/// Bundled main-thread system prompt template used for the default assistant thread.
pub const DEFAULT_ASSISTANT_SYSTEM_PROMPT: &str =
    include_str!("../../resources/prompts/thread_agent/main.md");

/// Bundled browser-thread system prompt template used for browser worker threads.
pub const DEFAULT_BROWSER_THREAD_SYSTEM_PROMPT: &str =
    include_str!("../../resources/prompts/thread_agent/browser.md");

/// Bundled `obswiki` thread system prompt template used for Obsidian-backed wiki worker threads.
pub const DEFAULT_OBSWIKI_THREAD_SYSTEM_PROMPT: &str =
    include_str!("../../resources/prompts/thread_agent/obswiki.md");

const PREDEFINED_THREAD_AGENT_CATALOG_YAML: &str = include_str!("../../config/agents.yaml");

/// Closed set of thread agent kinds used to select stable initialization profiles.
#[derive(
    Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum ThreadAgentKind {
    #[default]
    Main,
    Browser,
    Obswiki,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ThreadAgentFeatureInitPolicy {
    Resolver,
    Defaults,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThreadAgentAlwaysVisibleToolAccess {
    AllRegistered,
    Only(&'static [String]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThreadAgentOptionalToolsetAccess {
    AllRegisteredExcept(&'static [String]),
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ThreadAgentCapabilityProfile {
    pub kind: ThreadAgentKind,
    pub system_prompt: &'static str,
    pub default_bound_toolsets: &'static [String],
    pub default_features: &'static [Feature],
    pub allowed_features: &'static [Feature],
    pub feature_init_policy: ThreadAgentFeatureInitPolicy,
    pub always_visible_tool_access: ThreadAgentAlwaysVisibleToolAccess,
    pub optional_toolset_access: ThreadAgentOptionalToolsetAccess,
}

impl ThreadAgentCapabilityProfile {
    pub(crate) fn default_bound_toolsets(self) -> Vec<String> {
        self.default_bound_toolsets.to_vec()
    }

    pub(crate) fn default_features(self) -> Features {
        Features::from_iter(self.default_features.iter().copied())
    }

    pub(crate) fn allowed_features(self) -> Features {
        Features::from_iter(self.allowed_features.iter().copied())
    }

    pub(crate) fn binds_toolset(self, toolset_name: &str) -> bool {
        self.default_bound_toolsets
            .iter()
            .any(|candidate| candidate == toolset_name.trim())
    }

    pub(crate) fn allows_optional_toolset(self, toolset_name: &str) -> bool {
        let toolset_name = toolset_name.trim();
        if toolset_name.is_empty() || self.binds_toolset(toolset_name) {
            return false;
        }

        match self.optional_toolset_access {
            ThreadAgentOptionalToolsetAccess::AllRegisteredExcept(denylist) => {
                !denylist.iter().any(|candidate| candidate == toolset_name)
            }
            ThreadAgentOptionalToolsetAccess::None => false,
        }
    }

    pub(crate) fn allows_loaded_toolset(self, toolset_name: &str) -> bool {
        self.binds_toolset(toolset_name) || self.allows_optional_toolset(toolset_name)
    }

    pub(crate) fn allows_always_visible_tool(self, tool_name: &str) -> bool {
        let tool_name = tool_name.trim();
        if tool_name.is_empty() {
            return false;
        }

        match self.always_visible_tool_access {
            ThreadAgentAlwaysVisibleToolAccess::AllRegistered => true,
            ThreadAgentAlwaysVisibleToolAccess::Only(allowed) => {
                allowed.iter().any(|candidate| candidate == tool_name)
            }
        }
    }

    pub(crate) fn resolves_features_from_resolver(self) -> bool {
        self.feature_init_policy == ThreadAgentFeatureInitPolicy::Resolver
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ThreadAgentSystemPromptKey {
    Main,
    Browser,
    Obswiki,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum RawThreadAgentVisibleToolAccessMode {
    AllRegistered,
    Only,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum RawThreadAgentOptionalToolsetAccessMode {
    AllRegisteredExcept,
    None,
}

#[derive(Debug, Deserialize)]
struct RawThreadAgentCatalog {
    main: RawThreadAgentProfile,
    browser: RawThreadAgentProfile,
    obswiki: RawThreadAgentProfile,
}

#[derive(Debug, Deserialize)]
struct RawThreadAgentProfile {
    system_prompt: ThreadAgentSystemPromptKey,
    features: RawThreadAgentFeatureProfile,
    visible_tools: RawThreadAgentVisibleToolProfile,
    toolsets: RawThreadAgentToolsetProfile,
}

#[derive(Debug, Deserialize)]
struct RawThreadAgentFeatureProfile {
    init_policy: ThreadAgentFeatureInitPolicy,
    #[serde(default)]
    default: Vec<Feature>,
    #[serde(default)]
    allowed: Vec<Feature>,
}

#[derive(Debug, Deserialize)]
struct RawThreadAgentVisibleToolProfile {
    mode: RawThreadAgentVisibleToolAccessMode,
    #[serde(default)]
    names: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RawThreadAgentToolsetProfile {
    #[serde(default)]
    bound: Vec<String>,
    optional: RawThreadAgentOptionalToolsetProfile,
}

#[derive(Debug, Deserialize)]
struct RawThreadAgentOptionalToolsetProfile {
    mode: RawThreadAgentOptionalToolsetAccessMode,
    #[serde(default)]
    names: Vec<String>,
}

#[derive(Debug)]
struct PredefinedThreadAgentCatalog {
    main: PredefinedThreadAgentProfile,
    browser: PredefinedThreadAgentProfile,
    obswiki: PredefinedThreadAgentProfile,
}

impl PredefinedThreadAgentCatalog {
    fn profile(&self, kind: ThreadAgentKind) -> &PredefinedThreadAgentProfile {
        match kind {
            ThreadAgentKind::Main => &self.main,
            ThreadAgentKind::Browser => &self.browser,
            ThreadAgentKind::Obswiki => &self.obswiki,
        }
    }
}

#[derive(Debug)]
struct PredefinedThreadAgentProfile {
    system_prompt: &'static str,
    default_bound_toolsets: Vec<String>,
    default_features: Vec<Feature>,
    allowed_features: Vec<Feature>,
    feature_init_policy: ThreadAgentFeatureInitPolicy,
    visible_tools: PredefinedThreadAgentVisibleToolAccess,
    optional_toolsets: PredefinedThreadAgentOptionalToolsetAccess,
}

#[derive(Debug)]
enum PredefinedThreadAgentVisibleToolAccess {
    AllRegistered,
    Only(Vec<String>),
}

#[derive(Debug)]
enum PredefinedThreadAgentOptionalToolsetAccess {
    AllRegisteredExcept(Vec<String>),
    None,
}

fn predefined_thread_agent_catalog() -> &'static PredefinedThreadAgentCatalog {
    static PREDEFINED_THREAD_AGENT_CATALOG: OnceLock<PredefinedThreadAgentCatalog> =
        OnceLock::new();
    PREDEFINED_THREAD_AGENT_CATALOG.get_or_init(|| {
        load_predefined_thread_agent_catalog().unwrap_or_else(|error| {
            panic!(
                "failed to load static predefined agent catalog from config/agents.yaml: {error:#}"
            )
        })
    })
}

fn load_predefined_thread_agent_catalog() -> Result<PredefinedThreadAgentCatalog> {
    debug!(
        path = "config/agents.yaml",
        "loading predefined thread agent catalog"
    );
    let raw: RawThreadAgentCatalog = serde_yaml::from_str(PREDEFINED_THREAD_AGENT_CATALOG_YAML)
        .context("agents.yaml should parse as thread-agent catalog")?;
    let catalog = raw.resolve()?;
    debug!(
        path = "config/agents.yaml",
        main_default_feature_count = catalog.main.default_features.len(),
        browser_bound_toolset_count = catalog.browser.default_bound_toolsets.len(),
        "loaded predefined thread agent catalog"
    );
    Ok(catalog)
}

impl RawThreadAgentCatalog {
    fn resolve(self) -> Result<PredefinedThreadAgentCatalog> {
        Ok(PredefinedThreadAgentCatalog {
            main: self.main.resolve("main")?,
            browser: self.browser.resolve("browser")?,
            obswiki: self.obswiki.resolve("obswiki")?,
        })
    }
}

impl RawThreadAgentProfile {
    fn resolve(self, agent_kind: &'static str) -> Result<PredefinedThreadAgentProfile> {
        let default_bound_toolsets =
            normalize_profile_names(self.toolsets.bound, agent_kind, "toolsets.bound")?;
        let default_features =
            normalize_feature_list(self.features.default, agent_kind, "features.default");
        let allowed_features =
            normalize_feature_list(self.features.allowed, agent_kind, "features.allowed");
        ensure_default_features_within_allowed(&default_features, &allowed_features, agent_kind)?;

        Ok(PredefinedThreadAgentProfile {
            system_prompt: resolve_thread_agent_system_prompt(self.system_prompt),
            default_bound_toolsets,
            default_features,
            allowed_features,
            feature_init_policy: self.features.init_policy,
            visible_tools: self.visible_tools.resolve(agent_kind)?,
            optional_toolsets: self.toolsets.optional.resolve(agent_kind)?,
        })
    }
}

impl RawThreadAgentVisibleToolProfile {
    fn resolve(self, agent_kind: &'static str) -> Result<PredefinedThreadAgentVisibleToolAccess> {
        let names = normalize_profile_names(self.names, agent_kind, "visible_tools.names")?;
        match self.mode {
            RawThreadAgentVisibleToolAccessMode::AllRegistered => {
                if !names.is_empty() {
                    bail!(
                        "agent `{}` uses `visible_tools.mode = all_registered`, so `visible_tools.names` must be empty",
                        agent_kind
                    );
                }
                Ok(PredefinedThreadAgentVisibleToolAccess::AllRegistered)
            }
            RawThreadAgentVisibleToolAccessMode::Only => {
                Ok(PredefinedThreadAgentVisibleToolAccess::Only(names))
            }
        }
    }
}

impl RawThreadAgentOptionalToolsetProfile {
    fn resolve(
        self,
        agent_kind: &'static str,
    ) -> Result<PredefinedThreadAgentOptionalToolsetAccess> {
        let names = normalize_profile_names(self.names, agent_kind, "toolsets.optional.names")?;
        match self.mode {
            RawThreadAgentOptionalToolsetAccessMode::AllRegisteredExcept => {
                Ok(PredefinedThreadAgentOptionalToolsetAccess::AllRegisteredExcept(names))
            }
            RawThreadAgentOptionalToolsetAccessMode::None => {
                if !names.is_empty() {
                    bail!(
                        "agent `{}` uses `toolsets.optional.mode = none`, so `toolsets.optional.names` must be empty",
                        agent_kind
                    );
                }
                Ok(PredefinedThreadAgentOptionalToolsetAccess::None)
            }
        }
    }
}

fn normalize_profile_names(
    names: Vec<String>,
    agent_kind: &'static str,
    field_name: &'static str,
) -> Result<Vec<String>> {
    let mut normalized = Vec::new();
    let mut seen = BTreeSet::new();
    for raw_name in names {
        let name = raw_name.trim();
        if name.is_empty() {
            bail!(
                "agent `{}` contains one blank entry in `{}`",
                agent_kind,
                field_name
            );
        }
        if seen.insert(name.to_string()) {
            normalized.push(name.to_string());
        }
    }
    normalized.sort();
    Ok(normalized)
}

fn normalize_feature_list(
    features: Vec<Feature>,
    _agent_kind: &'static str,
    _field_name: &'static str,
) -> Vec<Feature> {
    let mut normalized = BTreeSet::new();
    for feature in features {
        normalized.insert(feature);
    }
    normalized.into_iter().collect()
}

fn ensure_default_features_within_allowed(
    default_features: &[Feature],
    allowed_features: &[Feature],
    agent_kind: &'static str,
) -> Result<()> {
    let allowed = allowed_features.iter().copied().collect::<BTreeSet<_>>();
    for feature in default_features {
        if !allowed.contains(feature) {
            bail!(
                "agent `{}` declares default feature `{}` outside `features.allowed`",
                agent_kind,
                feature.as_str()
            );
        }
    }
    Ok(())
}

fn resolve_thread_agent_system_prompt(key: ThreadAgentSystemPromptKey) -> &'static str {
    match key {
        ThreadAgentSystemPromptKey::Main => DEFAULT_ASSISTANT_SYSTEM_PROMPT.trim(),
        ThreadAgentSystemPromptKey::Browser => DEFAULT_BROWSER_THREAD_SYSTEM_PROMPT.trim(),
        ThreadAgentSystemPromptKey::Obswiki => DEFAULT_OBSWIKI_THREAD_SYSTEM_PROMPT.trim(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SubagentCatalogEntry {
    pub kind: ThreadAgentKind,
    pub subagent_key: &'static str,
    pub role_summary: &'static str,
    pub when_to_use: &'static str,
    pub when_not_to_use: &'static str,
}

const AVAILABLE_SUBAGENT_CATALOG: [SubagentCatalogEntry; 2] = [
    SubagentCatalogEntry {
        kind: ThreadAgentKind::Browser,
        subagent_key: "browser",
        role_summary: "负责浏览器观察与页面交互，在独立 child thread 中完成多步网页操作。",
        when_to_use: "任务明确需要打开页面、观察网页状态、执行连续页面动作，或者希望复用浏览器 child thread 的上下文。",
        when_not_to_use: "主线程已经能直接完成任务，或者只需要一次简单工具调用时，不要额外启动 browser subagent。",
    },
    SubagentCatalogEntry {
        kind: ThreadAgentKind::Obswiki,
        subagent_key: "obswiki",
        role_summary: "负责受控 Obsidian vault 检索、阅读、Raw 导入与 wiki/schema 页面维护，通常更适合用 persist 模式复用同一个 child thread。",
        when_to_use: "任务需要查询或整理本地 Obsidian 知识库，且希望在独立 child thread 中复用 vault 约束与索引上下文时，优先使用 persist 模式。",
        when_not_to_use: "问题不依赖本地 wiki 资产，或者只需要普通文件工具即可完成时，不要额外启动 obswiki subagent。",
    },
];

impl ThreadAgentKind {
    /// Return the stable label used by logs and persisted thread state.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::thread::ThreadAgentKind;
    ///
    /// assert_eq!(ThreadAgentKind::Main.as_str(), "main");
    /// assert_eq!(ThreadAgentKind::Browser.as_str(), "browser");
    /// assert_eq!(ThreadAgentKind::Obswiki.as_str(), "obswiki");
    /// ```
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Browser => "browser",
            Self::Obswiki => "obswiki",
        }
    }

    /// Resolve the stable capability boundary
    /// owned by this thread-agent kind.
    pub(crate) fn capability_profile(self) -> ThreadAgentCapabilityProfile {
        let profile = predefined_thread_agent_catalog().profile(self);
        ThreadAgentCapabilityProfile {
            kind: self,
            system_prompt: profile.system_prompt,
            default_bound_toolsets: profile.default_bound_toolsets.as_slice(),
            default_features: profile.default_features.as_slice(),
            allowed_features: profile.allowed_features.as_slice(),
            feature_init_policy: profile.feature_init_policy,
            always_visible_tool_access: match &profile.visible_tools {
                PredefinedThreadAgentVisibleToolAccess::AllRegistered => {
                    ThreadAgentAlwaysVisibleToolAccess::AllRegistered
                }
                PredefinedThreadAgentVisibleToolAccess::Only(names) => {
                    ThreadAgentAlwaysVisibleToolAccess::Only(names.as_slice())
                }
            },
            optional_toolset_access: match &profile.optional_toolsets {
                PredefinedThreadAgentOptionalToolsetAccess::AllRegisteredExcept(names) => {
                    ThreadAgentOptionalToolsetAccess::AllRegisteredExcept(names.as_slice())
                }
                PredefinedThreadAgentOptionalToolsetAccess::None => {
                    ThreadAgentOptionalToolsetAccess::None
                }
            },
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
        self.capability_profile().system_prompt
    }

    pub(crate) fn default_bound_toolsets(self) -> Vec<String> {
        self.capability_profile().default_bound_toolsets()
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
