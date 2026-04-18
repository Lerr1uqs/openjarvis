//! Thread-first aggregate model and atomic thread-owned persistence helpers.

mod agent;

use crate::agent::{
    FeatureResolver, MemoryRepository, ShellEnv, ToolCallRequest, ToolCallResult, ToolDefinition,
    ToolRegistry, feature,
};
use crate::config::{AgentCompactConfig, try_global_config};
use crate::context::{ChatMessage, ChatMessageRole};
use crate::session::SessionManager;
use anyhow::{Result, bail};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use tracing::{debug, info, warn};
use uuid::Uuid;

pub(crate) use agent::SubagentCatalogEntry;
pub use agent::{
    DEFAULT_ASSISTANT_SYSTEM_PROMPT, DEFAULT_BROWSER_THREAD_SYSTEM_PROMPT, ThreadAgent,
    ThreadAgentKind,
};

const OPENJARVIS_THREAD_ID_NAMESPACE: Uuid =
    Uuid::from_u128(0x7f4b2e8d_5d33_4f51_9c27_9c5d7d76c1a1);
const TOOL_USE_MODE_PROMPT: &str = "You are running in OpenJarvis tool-use mode. Use the provided tools when needed. You may also provide a short user-visible reply before calling a tool.";

/// Derive the stable internal thread id from one normalized thread key.
///
/// `thread_key` follows the contract `user:channel:external_thread_id`.
///
/// # 示例
/// ```rust
/// use openjarvis::thread::derive_internal_thread_id;
///
/// let thread_id = derive_internal_thread_id("ou_xxx:feishu:thread_ext");
/// assert_eq!(
///     thread_id,
///     derive_internal_thread_id("ou_xxx:feishu:thread_ext")
/// );
/// ```
pub fn derive_internal_thread_id(thread_key: &str) -> Uuid {
    Uuid::new_v5(&OPENJARVIS_THREAD_ID_NAMESPACE, thread_key.as_bytes())
}

/// Stable child-thread lifecycle mode used by subagent flows.
#[derive(
    Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SubagentSpawnMode {
    Persist,
    Yolo,
}

impl SubagentSpawnMode {
    /// Return the stable spawn-mode label used by logs and persistence.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::thread::SubagentSpawnMode;
    ///
    /// assert_eq!(SubagentSpawnMode::Persist.as_str(), "persist");
    /// assert_eq!(SubagentSpawnMode::Yolo.as_str(), "yolo");
    /// ```
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Persist => "persist",
            Self::Yolo => "yolo",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "persist" => Some(Self::Persist),
            "yolo" => Some(Self::Yolo),
            _ => None,
        }
    }
}

/// Stable child-thread identity reused across recovery and repeated prepare calls.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ChildThreadIdentity {
    pub parent_thread_id: String,
    pub subagent_key: String,
    pub spawn_mode: SubagentSpawnMode,
}

impl ChildThreadIdentity {
    /// Build one explicit child-thread identity.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::thread::{ChildThreadIdentity, SubagentSpawnMode};
    ///
    /// let identity = ChildThreadIdentity::new(
    ///     "parent-thread",
    ///     "browser",
    ///     SubagentSpawnMode::Persist,
    /// );
    /// assert_eq!(identity.parent_thread_id, "parent-thread");
    /// assert_eq!(identity.subagent_key, "browser");
    /// ```
    pub fn new(
        parent_thread_id: impl Into<String>,
        subagent_key: impl Into<String>,
        spawn_mode: SubagentSpawnMode,
    ) -> Self {
        Self {
            parent_thread_id: parent_thread_id.into(),
            subagent_key: subagent_key.into(),
            spawn_mode,
        }
    }

    pub fn storage_key(&self) -> String {
        format!(
            "child:{}:{}",
            self.parent_thread_id.trim(),
            self.subagent_key.trim()
        )
    }
}

/// Derive the stable child thread id from one parent thread id and subagent profile key.
///
/// `spawn_mode` is excluded so the same parent/profile always resolves to the same child thread.
///
/// # 示例
/// ```rust
/// use openjarvis::thread::derive_child_thread_id;
///
/// let thread_id = derive_child_thread_id("parent-thread", "browser");
/// assert_eq!(thread_id, derive_child_thread_id("parent-thread", "browser"));
/// ```
pub fn derive_child_thread_id(parent_thread_id: &str, subagent_key: &str) -> Uuid {
    derive_internal_thread_id(&format!(
        "child:{}:{}",
        parent_thread_id.trim(),
        subagent_key.trim()
    ))
}

fn default_tool_event_metadata() -> Value {
    json!({})
}

fn normalize_loaded_toolsets<I>(loaded_toolsets: I) -> Vec<String>
where
    I: IntoIterator<Item = String>,
{
    let mut names = loaded_toolsets
        .into_iter()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    names
}

fn build_default_feature_resolver(compact_config: &AgentCompactConfig) -> FeatureResolver {
    let mut available_features =
        Features::from_iter([Feature::Memory, Feature::Skill, Feature::Subagent]);
    if feature::init::auto_compact::is_available(compact_config) {
        available_features.insert(Feature::AutoCompact);
    }

    if let Some(config) = try_global_config() {
        info!("building thread feature resolver from installed global config");
        FeatureResolver::from_app_config(config, available_features)
    } else {
        info!(
            enabled_features = ?available_features.names(),
            "building thread feature resolver without installed global config"
        );
        FeatureResolver::development_default(available_features)
    }
}

/// One thread feature that can inject stable prompts or runtime capability during initialization.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Feature {
    Memory,
    Skill,
    Subagent,
    AutoCompact,
}

impl Feature {
    /// Return the stable feature label used by logs and config parsing.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::thread::Feature;
    ///
    /// assert_eq!(Feature::Memory.as_str(), "memory");
    /// assert_eq!(Feature::Subagent.as_str(), "subagent");
    /// ```
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Skill => "skill",
            Self::Subagent => "subagent",
            Self::AutoCompact => "auto_compact",
        }
    }
}

/// Stable ordered thread feature set persisted as thread state truth.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct Features {
    enabled: BTreeSet<Feature>,
}

impl Features {
    /// Return all currently defined thread features in stable order.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::thread::{Feature, Features};
    ///
    /// let features = Features::all();
    /// assert!(features.contains(Feature::Memory));
    /// assert!(features.contains(Feature::Skill));
    /// assert!(features.contains(Feature::Subagent));
    /// assert!(features.contains(Feature::AutoCompact));
    /// ```
    pub fn all() -> Self {
        Self::from_iter([
            Feature::Memory,
            Feature::Skill,
            Feature::Subagent,
            Feature::AutoCompact,
        ])
    }

    /// Return whether the target feature is enabled in this set.
    pub fn contains(&self, feature: Feature) -> bool {
        self.enabled.contains(&feature)
    }

    /// Insert one feature into the set.
    pub fn insert(&mut self, feature: Feature) -> bool {
        self.enabled.insert(feature)
    }

    /// Remove one feature from the set.
    pub fn remove(&mut self, feature: Feature) -> bool {
        self.enabled.remove(&feature)
    }

    /// Return whether the set is empty.
    pub fn is_empty(&self) -> bool {
        self.enabled.is_empty()
    }

    /// Return the stable ordered feature names for logs and assertions.
    pub fn names(&self) -> Vec<&'static str> {
        self.enabled.iter().copied().map(Feature::as_str).collect()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = Feature> + '_ {
        self.enabled.iter().copied()
    }

    pub(crate) fn intersect(&self, allowed: &Features) -> Features {
        Self::from_iter(
            self.enabled
                .iter()
                .filter(|feature| allowed.contains(**feature))
                .copied(),
        )
    }
}

impl FromIterator<Feature> for Features {
    fn from_iter<T: IntoIterator<Item = Feature>>(iter: T) -> Self {
        Self {
            enabled: iter.into_iter().collect(),
        }
    }
}

impl IntoIterator for Features {
    type Item = Feature;
    type IntoIter = std::collections::btree_set::IntoIter<Feature>;

    fn into_iter(self) -> Self::IntoIter {
        self.enabled.into_iter()
    }
}

impl<'a> IntoIterator for &'a Features {
    type Item = Feature;
    type IntoIter = std::iter::Copied<std::collections::btree_set::Iter<'a, Feature>>;

    fn into_iter(self) -> Self::IntoIter {
        self.enabled.iter().copied()
    }
}

/// Stable runtime locator shared by session, command, and agent execution on one thread.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadContextLocator {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub channel: String,
    pub user_id: String,
    pub external_thread_id: String,
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_thread: Option<ChildThreadIdentity>,
}

impl ThreadContextLocator {
    /// Build one explicit thread-context locator.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use openjarvis::thread::ThreadContextLocator;
    ///
    /// let locator = ThreadContextLocator::new(
    ///     Some("session-1".to_string()),
    ///     "feishu",
    ///     "ou_xxx",
    ///     "thread_ext",
    ///     "3d71df7b-8708-5b55-a1a8-4627ae6e30c1",
    /// );
    /// assert_eq!(locator.external_thread_id, "thread_ext");
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn new(
        session_id: Option<String>,
        channel: impl Into<String>,
        user_id: impl Into<String>,
        external_thread_id: impl Into<String>,
        thread_id: impl Into<String>,
    ) -> Self {
        Self {
            session_id,
            channel: channel.into(),
            user_id: user_id.into(),
            external_thread_id: external_thread_id.into(),
            thread_id: thread_id.into(),
            child_thread: None,
        }
    }

    /// Attach one child-thread identity to the locator.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::thread::{ChildThreadIdentity, SubagentSpawnMode, ThreadContextLocator};
    ///
    /// let locator = ThreadContextLocator::new(
    ///     Some("session-1".to_string()),
    ///     "feishu",
    ///     "ou_xxx",
    ///     "thread_ext",
    ///     "thread_internal",
    /// )
    /// .with_child_thread(ChildThreadIdentity::new(
    ///     "parent-thread",
    ///     "browser",
    ///     SubagentSpawnMode::Persist,
    /// ));
    ///
    /// assert_eq!(
    ///     locator.child_thread.as_ref().map(|value| value.subagent_key.as_str()),
    ///     Some("browser")
    /// );
    /// ```
    pub fn with_child_thread(mut self, child_thread: ChildThreadIdentity) -> Self {
        self.child_thread = Some(child_thread);
        self
    }

    /// Return the normalized persistence key used to derive or store this thread.
    ///
    /// Main-thread keys follow `user:channel:external_thread_id`. Child-thread keys follow
    /// `child:parent_thread_id:subagent_key`.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use openjarvis::thread::ThreadContextLocator;
    ///
    /// let locator = ThreadContextLocator::new(
    ///     Some("session-1".to_string()),
    ///     "feishu",
    ///     "ou_xxx",
    ///     "thread_ext",
    ///     "00000000-0000-0000-0000-000000000001",
    /// );
    /// assert_eq!(locator.thread_key(), "ou_xxx:feishu:thread_ext");
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn thread_key(&self) -> String {
        self.child_thread
            .as_ref()
            .map(ChildThreadIdentity::storage_key)
            .unwrap_or_else(|| {
                format!(
                    "{}:{}:{}",
                    self.user_id, self.channel, self.external_thread_id
                )
            })
    }
}

/// Thread lifecycle state that belongs to persisted thread state.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadLifecycleState {
    #[serde(default)]
    pub initialized: bool,
}

/// Thread feature flags persisted as thread-owned runtime truth.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadFeatureState {
    #[serde(default)]
    pub enabled_features: Features,
}

/// Thread-scoped tool runtime state owned by `Thread`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ThreadToolState {
    #[serde(default)]
    pub loaded_toolsets: Vec<String>,
}

/// One pending approval request reserved for future thread-scoped policy flows.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadApprovalRequest {
    pub id: Uuid,
    pub action: String,
    #[serde(default = "default_tool_event_metadata")]
    pub metadata: Value,
    pub requested_at: DateTime<Utc>,
}

/// One persisted approval decision reserved for future thread-scoped policy flows.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadApprovalDecision {
    pub request_id: Uuid,
    pub approved: bool,
    #[serde(default = "default_tool_event_metadata")]
    pub metadata: Value,
    pub decided_at: DateTime<Utc>,
}

/// Thread-scoped approval state reserved for policy and approval workflows.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ThreadApprovalState {
    #[serde(default)]
    pub pending: Vec<ThreadApprovalRequest>,
    #[serde(default)]
    pub decisions: Vec<ThreadApprovalDecision>,
}

/// Full thread-scoped non-message state.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ThreadState {
    #[serde(default)]
    pub lifecycle: ThreadLifecycleState,
    #[serde(default)]
    pub agent: ThreadAgent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_thread: Option<ChildThreadIdentity>,
    #[serde(default)]
    pub features: ThreadFeatureState,
    #[serde(default)]
    pub tools: ThreadToolState,
    #[serde(default)]
    pub approval: ThreadApprovalState,
}

/// Persisted message domain owned by one thread aggregate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadContext {
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ThreadContext {
    /// Create one empty persisted thread context.
    pub fn new(now: DateTime<Utc>) -> Self {
        Self {
            messages: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Export all persisted messages currently owned by the thread.
    pub fn messages(&self) -> Vec<ChatMessage> {
        self.messages.clone()
    }
}

/// Persisted thread snapshot written by thread-owned CAS mutations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PersistedThreadSnapshot {
    pub thread: ThreadContext,
    #[serde(default)]
    pub state: ThreadState,
}

impl PersistedThreadSnapshot {
    /// Create one empty persisted snapshot at the provided timestamp.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::thread::PersistedThreadSnapshot;
    ///
    /// let snapshot = PersistedThreadSnapshot::new(Utc::now());
    /// assert!(snapshot.thread.messages.is_empty());
    /// ```
    pub fn new(now: DateTime<Utc>) -> Self {
        Self {
            thread: ThreadContext::new(now),
            state: ThreadState::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct ActiveRequestState {
    external_message_id: Option<String>,
    started_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
struct RequestRuntimeState {
    sessions: SessionManager,
}

/// Persistence boundary used by live `Thread` handles.
#[async_trait]
pub trait ThreadSnapshotStore: Send + Sync + fmt::Debug {
    /// Persist one thread snapshot using compare-and-swap revision semantics.
    async fn save_thread_snapshot(
        &self,
        locator: &ThreadContextLocator,
        snapshot: &PersistedThreadSnapshot,
        expected_revision: u64,
    ) -> Result<u64>;
}

/// Dedicated runtime container for thread initialization and runtime service access.
#[derive(Clone)]
pub struct ThreadRuntime {
    tool_registry: Arc<ToolRegistry>,
    memory_repository: Arc<MemoryRepository>,
    compact_config: AgentCompactConfig,
    feature_resolver: FeatureResolver,
}

impl ThreadRuntime {
    /// Build one thread runtime from shared runtime services.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use openjarvis::{
    ///     agent::{MemoryRepository, ToolRegistry},
    ///     config::AgentCompactConfig,
    ///     thread::ThreadRuntime,
    /// };
    /// use std::sync::Arc;
    ///
    /// let tool_registry = Arc::new(ToolRegistry::new());
    /// let memory_repository = Arc::new(MemoryRepository::new("."));
    /// let _runtime = ThreadRuntime::new(tool_registry, memory_repository, AgentCompactConfig::default());
    /// ```
    pub fn new(
        tool_registry: Arc<ToolRegistry>,
        memory_repository: Arc<MemoryRepository>,
        compact_config: AgentCompactConfig,
    ) -> Self {
        let feature_resolver = build_default_feature_resolver(&compact_config);
        Self::with_feature_resolver(
            tool_registry,
            memory_repository,
            compact_config,
            feature_resolver,
        )
    }

    /// Build one thread runtime with one explicit feature resolver.
    pub fn with_feature_resolver(
        tool_registry: Arc<ToolRegistry>,
        memory_repository: Arc<MemoryRepository>,
        compact_config: AgentCompactConfig,
        feature_resolver: FeatureResolver,
    ) -> Self {
        Self {
            tool_registry,
            memory_repository,
            compact_config,
            feature_resolver,
        }
    }

    /// Return the shared tool registry.
    pub fn tool_registry(&self) -> Arc<ToolRegistry> {
        Arc::clone(&self.tool_registry)
    }

    /// Return the shared memory repository.
    pub fn memory_repository(&self) -> Arc<MemoryRepository> {
        Arc::clone(&self.memory_repository)
    }

    /// Ensure built-in tools are ready before the thread is served.
    pub async fn ensure_tool_registry_ready(&self) -> Result<()> {
        self.tool_registry.register_builtin_tools().await
    }

    fn filter_enabled_features_for_thread(
        &self,
        thread_context: &Thread,
        thread_agent: &ThreadAgent,
        mut features: Features,
    ) -> Features {
        if (thread_context.child_thread_identity().is_some() || thread_agent.kind.is_subagent())
            && features.remove(Feature::Subagent)
        {
            info!(
                thread_id = %thread_context.locator.thread_id,
                thread_agent_kind = thread_agent.kind.as_str(),
                enabled_features = ?features.names(),
                "removed parent-only subagent feature from child thread feature snapshot"
            );
        }
        features
    }

    fn resolve_enabled_features(
        &self,
        thread_context: &Thread,
        thread_agent: &ThreadAgent,
    ) -> Features {
        let persisted_features = thread_context.enabled_features();
        if !persisted_features.is_empty() {
            info!(
                thread_id = %thread_context.locator.thread_id,
                enabled_features = ?persisted_features.names(),
                "using persisted thread feature snapshot"
            );
            return self.filter_enabled_features_for_thread(
                thread_context,
                thread_agent,
                persisted_features,
            );
        }

        let resolved = self
            .feature_resolver
            .resolve_for_locator(&thread_context.locator);
        info!(
            thread_id = %thread_context.locator.thread_id,
            enabled_features = ?resolved.names(),
            "resolved thread feature snapshot for initialization"
        );
        self.filter_enabled_features_for_thread(thread_context, thread_agent, resolved)
    }

    fn resolve_thread_agent(
        &self,
        thread_context: &Thread,
        requested_kind: ThreadAgentKind,
    ) -> ThreadAgent {
        let persisted_agent = thread_context.thread_agent();
        if thread_context.is_initialized() {
            if persisted_agent.kind != requested_kind {
                warn!(
                    thread_id = %thread_context.locator.thread_id,
                    persisted_thread_agent_kind = persisted_agent.kind.as_str(),
                    requested_thread_agent_kind = requested_kind.as_str(),
                    "initialized thread already owns a persisted thread agent; ignoring requested kind"
                );
            }
            return persisted_agent;
        }

        ThreadAgent::from_kind(requested_kind)
    }

    fn preloaded_toolsets(&self, thread_agent: &ThreadAgent, features: &Features) -> Vec<String> {
        let mut toolsets = thread_agent.bound_toolsets.clone();
        for feature in features.iter() {
            match feature {
                Feature::Memory => toolsets.extend(feature::init::memory::toolsets()),
                Feature::Skill | Feature::Subagent | Feature::AutoCompact => {}
            }
        }
        toolsets.sort();
        toolsets.dedup();
        toolsets
    }

    async fn build_predefined_role_messages(
        &self,
        thread_agent: &ThreadAgent,
        preloaded_toolsets: &[String],
        initialized_at: DateTime<Utc>,
    ) -> Result<Vec<ChatMessage>> {
        let mut messages = Vec::new();
        let system_prompt = thread_agent.system_prompt().trim();
        if !system_prompt.is_empty() {
            messages.push(ChatMessage::new(
                ChatMessageRole::System,
                system_prompt,
                initialized_at,
            ));
        }
        messages.push(ChatMessage::new(
            ChatMessageRole::System,
            TOOL_USE_MODE_PROMPT,
            initialized_at,
        ));
        if let Some(catalog_prompt) = self
            .tool_registry
            .render_toolset_catalog_prompt(preloaded_toolsets)
            .await
        {
            messages.push(ChatMessage::new(
                ChatMessageRole::System,
                catalog_prompt,
                initialized_at,
            ));
        }
        Ok(messages)
    }

    /// Persist one initialized thread prefix before the thread enters normal request handling.
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn demo() -> anyhow::Result<()> {
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     agent::{MemoryRepository, ToolRegistry},
    ///     config::AgentCompactConfig,
    ///     thread::{Thread, ThreadContextLocator, ThreadRuntime},
    /// };
    /// use std::sync::Arc;
    ///
    /// let tool_registry = Arc::new(ToolRegistry::new());
    /// let memory_repository = Arc::new(MemoryRepository::new("."));
    /// let runtime = ThreadRuntime::new(
    ///     tool_registry,
    ///     memory_repository,
    ///     AgentCompactConfig::default(),
    /// );
    /// let mut thread = Thread::new(
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_ext", "thread_internal"),
    ///     Utc::now(),
    /// );
    ///
    /// use openjarvis::thread::ThreadAgentKind;
    ///
    /// runtime
    ///     .initialize_thread(&mut thread, ThreadAgentKind::Main)
    ///     .await?;
    /// assert!(thread.is_initialized());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn initialize_thread(
        &self,
        thread_context: &mut Thread,
        requested_thread_agent_kind: ThreadAgentKind,
    ) -> Result<bool> {
        self.ensure_tool_registry_ready().await?;

        let resolved_thread_agent =
            self.resolve_thread_agent(thread_context, requested_thread_agent_kind);
        let resolved_features =
            self.resolve_enabled_features(thread_context, &resolved_thread_agent);
        let preloaded_toolsets =
            self.preloaded_toolsets(&resolved_thread_agent, &resolved_features);

        if thread_context.is_initialized() {
            let backfilled = thread_context
                .reconcile_initialized_feature_state(
                    resolved_thread_agent.clone(),
                    resolved_features,
                    preloaded_toolsets,
                )
                .await?;
            if backfilled {
                info!(
                    thread_id = %thread_context.locator.thread_id,
                    thread_agent_kind = thread_context.thread_agent_kind().as_str(),
                    enabled_features = ?thread_context.enabled_features().names(),
                    "backfilled persisted thread feature state for initialized thread"
                );
            }
            return Ok(false);
        }

        let existing_system_prefix_at = thread_context
            .thread
            .messages
            .iter()
            .find(|message| message.role == ChatMessageRole::System)
            .map(|message| message.created_at);
        if let Some(initialized_at) = existing_system_prefix_at {
            thread_context
                .mark_initialized_with_state(
                    initialized_at,
                    resolved_thread_agent.clone(),
                    resolved_features,
                    preloaded_toolsets,
                )
                .await?;
            info!(
                thread_id = %thread_context.locator.thread_id,
                external_thread_id = %thread_context.locator.external_thread_id,
                thread_agent_kind = resolved_thread_agent.kind.as_str(),
                initialized_at = %initialized_at,
                "marked existing thread as initialized from persisted system prefix"
            );
            return Ok(true);
        }

        let initialized_at = Utc::now();
        let mut initialized_messages = self
            .build_predefined_role_messages(
                &resolved_thread_agent,
                &preloaded_toolsets,
                initialized_at,
            )
            .await?;
        initialized_messages.push(ChatMessage::new(
            ChatMessageRole::System,
            ShellEnv::detect().render_prompt(),
            initialized_at,
        ));

        thread_context.replace_thread_agent(resolved_thread_agent.clone());
        thread_context.replace_enabled_features(Features::default());
        for feature in &resolved_features {
            thread_context.enable_feature(feature);
        }

        if thread_context.is_enabled(Feature::Memory)
            && let Some(prompt) = feature::init::memory::usage(
                &thread_context.locator.thread_id,
                &self.memory_repository,
            )?
        {
            initialized_messages.push(ChatMessage::new(
                ChatMessageRole::System,
                prompt,
                initialized_at,
            ));
        }

        if thread_context.is_enabled(Feature::Skill)
            && let Some(prompt) =
                feature::init::skill::usage(&thread_context.locator.thread_id, &self.tool_registry)
                    .await
        {
            initialized_messages.push(ChatMessage::new(
                ChatMessageRole::System,
                prompt,
                initialized_at,
            ));
        }

        if thread_context.is_enabled(Feature::Subagent)
            && let Some(prompt) = feature::init::subagent::usage(
                &thread_context.locator.thread_id,
                &self.tool_registry,
            )
            .await
        {
            initialized_messages.push(ChatMessage::new(
                ChatMessageRole::System,
                prompt,
                initialized_at,
            ));
        }

        if thread_context.is_enabled(Feature::AutoCompact)
            && let Some(prompt) = feature::init::auto_compact::usage(&self.compact_config)
        {
            initialized_messages.push(ChatMessage::new(
                ChatMessageRole::System,
                prompt,
                initialized_at,
            ));
        }
        if thread_context.is_enabled(Feature::AutoCompact)
            && let Some(prompt) =
                feature::init::auto_compact::tool_visibility_prompt(&self.compact_config)
        {
            initialized_messages.push(ChatMessage::new(
                ChatMessageRole::System,
                prompt,
                initialized_at,
            ));
        }

        thread_context
            .initialize_with_messages(
                initialized_messages,
                initialized_at,
                resolved_thread_agent,
                resolved_features,
                preloaded_toolsets,
            )
            .await?;
        info!(
            thread_id = %thread_context.locator.thread_id,
            external_thread_id = %thread_context.locator.external_thread_id,
            thread_agent_kind = thread_context.thread_agent_kind().as_str(),
            initialized_message_count = thread_context
                .thread
                .messages
                .iter()
                .filter(|message| message.role == ChatMessageRole::System)
                .count(),
            enabled_features = ?thread_context.enabled_features().names(),
            "persisted thread initialization prefix"
        );
        Ok(true)
    }
}

impl fmt::Debug for ThreadRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ThreadRuntime")
            .field("tool_registry", &"Arc<ToolRegistry>")
            .field("memory_repository", &"Arc<MemoryRepository>")
            .field("compact_config", &self.compact_config)
            .field("feature_resolver", &self.feature_resolver)
            .finish()
    }
}

/// Live thread handle. Persisted state is separated from live request-only state.
#[derive(Clone)]
pub struct Thread {
    pub locator: ThreadContextLocator,
    pub thread: ThreadContext,
    pub state: ThreadState,
    revision: u64,
    active_request: Option<ActiveRequestState>,
    request_runtime: Option<RequestRuntimeState>,
    store: Option<Arc<dyn ThreadSnapshotStore>>,
}

impl Thread {
    /// Create one empty live thread handle.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::thread::{Thread, ThreadContextLocator};
    ///
    /// let thread = Thread::new(
    ///     ThreadContextLocator::new(
    ///         Some("session-1".to_string()),
    ///         "feishu",
    ///         "ou_xxx",
    ///         "thread_ext",
    ///         "3d71df7b-8708-5b55-a1a8-4627ae6e30c1",
    ///     ),
    ///     Utc::now(),
    /// );
    /// assert_eq!(thread.locator.external_thread_id, "thread_ext");
    /// ```
    pub fn new(locator: ThreadContextLocator, now: DateTime<Utc>) -> Self {
        Self {
            locator,
            thread: ThreadContext::new(now),
            state: ThreadState::default(),
            revision: 0,
            active_request: None,
            request_runtime: None,
            store: None,
        }
    }

    /// Rebuild one live thread handle from one persisted snapshot and revision.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::thread::{PersistedThreadSnapshot, Thread, ThreadContextLocator};
    ///
    /// let locator =
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_ext", "thread_internal");
    /// let thread = Thread::from_persisted(locator.clone(), PersistedThreadSnapshot::new(Utc::now()), 7);
    /// assert_eq!(thread.locator, locator);
    /// ```
    pub fn from_persisted(
        locator: ThreadContextLocator,
        snapshot: PersistedThreadSnapshot,
        revision: u64,
    ) -> Self {
        Self {
            locator,
            thread: snapshot.thread,
            state: snapshot.state,
            revision,
            active_request: None,
            request_runtime: None,
            store: None,
        }
    }

    /// Return the persisted snapshot currently owned by this live handle.
    pub fn persisted_snapshot(&self) -> PersistedThreadSnapshot {
        PersistedThreadSnapshot {
            thread: self.thread.clone(),
            state: self.state.clone(),
        }
    }

    /// Rebind the runtime locator while keeping persisted thread state intact.
    pub fn rebind_locator(&mut self, locator: ThreadContextLocator) {
        self.locator = locator;
    }

    /// Bind one thread snapshot store to the live handle.
    pub fn bind_store(&mut self, store: Arc<dyn ThreadSnapshotStore>) {
        self.store = Some(store);
    }

    /// Bind one request-scoped runtime context used by tools that need session access.
    #[doc(hidden)]
    pub fn bind_request_runtime(&mut self, sessions: SessionManager) {
        self.request_runtime = Some(RequestRuntimeState { sessions });
    }

    /// Export the thread-owned formal message sequence.
    pub fn messages(&self) -> Vec<ChatMessage> {
        self.thread.messages()
    }

    /// Export the compact source message sequence used by runtime compaction.
    pub fn compact_source_messages(&self) -> Vec<ChatMessage> {
        self.thread
            .messages()
            .into_iter()
            .filter(|message| message.role != ChatMessageRole::System)
            .collect()
    }

    /// Return whether this thread already owns a persisted initialized prefix.
    pub fn is_initialized(&self) -> bool {
        self.state.lifecycle.initialized
    }

    /// Return the persisted thread agent profile for the current thread.
    pub fn thread_agent(&self) -> ThreadAgent {
        self.state.agent.clone()
    }

    /// Return the persisted thread agent kind for the current thread.
    pub fn thread_agent_kind(&self) -> ThreadAgentKind {
        self.state.agent.kind
    }

    /// Return the persisted child-thread identity when this thread belongs to a parent thread.
    pub fn child_thread_identity(&self) -> Option<&ChildThreadIdentity> {
        self.state.child_thread.as_ref()
    }

    /// Return the persisted enabled feature set for the current thread.
    pub fn enabled_features(&self) -> Features {
        self.state.features.enabled_features.clone()
    }

    /// Return whether the target feature is enabled for the current thread.
    pub fn is_enabled(&self, feature: Feature) -> bool {
        self.state.features.enabled_features.contains(feature)
    }

    /// Return the persisted loaded toolsets for the thread.
    pub fn load_toolsets(&self) -> Vec<String> {
        self.state.tools.loaded_toolsets.clone()
    }

    /// Return whether the thread currently owns one active request-local state.
    pub fn has_active_request(&self) -> bool {
        self.active_request.is_some()
    }

    /// Return the current active request external message id.
    pub fn current_request_external_message_id(&self) -> Option<String> {
        self.active_request
            .as_ref()
            .and_then(|request| request.external_message_id.clone())
    }

    /// Start one request-local live state without creating any persisted turn structure.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::thread::{Thread, ThreadContextLocator};
    ///
    /// let now = Utc::now();
    /// let mut thread = Thread::new(
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_ext", "thread_internal"),
    ///     now,
    /// );
    /// thread
    ///     .begin_request(Some("msg_1".to_string()), now)
    ///     .expect("request should start");
    /// assert!(thread.has_active_request());
    /// ```
    pub fn begin_request(
        &mut self,
        external_message_id: Option<String>,
        started_at: DateTime<Utc>,
    ) -> Result<()> {
        if self.active_request.is_some() {
            bail!(
                "thread `{}` already owns one unfinished request",
                self.locator.thread_id
            );
        }

        info!(
            thread_id = %self.locator.thread_id,
            external_thread_id = %self.locator.external_thread_id,
            external_message_id = ?external_message_id,
            started_at = %started_at,
            "started thread-owned active request"
        );
        self.active_request = Some(ActiveRequestState {
            external_message_id,
            started_at,
        });
        Ok(())
    }

    /// Finish the current request-local live state.
    pub fn finish_request(&mut self, completed_at: DateTime<Utc>, succeeded: bool) -> Result<()> {
        let Some(active_request) = self.active_request.take() else {
            bail!(
                "thread `{}` does not own one active request",
                self.locator.thread_id
            );
        };
        info!(
            thread_id = %self.locator.thread_id,
            external_thread_id = %self.locator.external_thread_id,
            external_message_id = ?active_request.external_message_id,
            request_started_at = %active_request.started_at,
            completed_at = %completed_at,
            succeeded,
            "finished thread-owned active request"
        );
        self.request_runtime = None;
        Ok(())
    }

    /// Persist one child-thread identity snapshot as part of the thread truth.
    pub async fn persist_child_thread_identity(
        &mut self,
        child_thread: ChildThreadIdentity,
    ) -> Result<bool> {
        let updated_at = Utc::now();
        let changed = self
            .apply_persisted_mutation("persist_child_thread_identity", |snapshot| {
                if snapshot.state.child_thread.as_ref() == Some(&child_thread) {
                    return Ok(false);
                }
                snapshot.state.child_thread = Some(child_thread.clone());
                snapshot.thread.updated_at = updated_at;
                Ok(true)
            })
            .await?;
        if changed {
            self.locator.child_thread = Some(child_thread.clone());
            info!(
                thread_id = %self.locator.thread_id,
                parent_thread_id = %child_thread.parent_thread_id,
                subagent_key = %child_thread.subagent_key,
                spawn_mode = child_thread.spawn_mode.as_str(),
                "persisted child-thread identity"
            );
        }
        Ok(changed)
    }

    async fn apply_persisted_mutation<F, R>(
        &mut self,
        mutation_name: &'static str,
        mutate: F,
    ) -> Result<R>
    where
        F: FnOnce(&mut PersistedThreadSnapshot) -> Result<R>,
    {
        let mut next = self.persisted_snapshot();
        let result = mutate(&mut next)?;
        let updated_at = next.thread.updated_at;
        let expected_revision = self.revision;

        if let Some(store) = self.store.as_ref() {
            let new_revision = store
                .save_thread_snapshot(&self.locator, &next, expected_revision)
                .await?;
            self.revision = new_revision;
        }

        self.thread = next.thread;
        self.state = next.state;
        debug!(
            thread_id = %self.locator.thread_id,
            revision = self.revision,
            updated_at = %updated_at,
            mutation = mutation_name,
            "applied thread-owned persisted mutation"
        );
        Ok(result)
    }

    pub(crate) async fn mark_initialized_with_state(
        &mut self,
        initialized_at: DateTime<Utc>,
        thread_agent: ThreadAgent,
        enabled_features: Features,
        preloaded_toolsets: Vec<String>,
    ) -> Result<()> {
        self.apply_persisted_mutation("mark_initialized_with_state", |snapshot| {
            snapshot.state.lifecycle.initialized = true;
            snapshot.state.agent = thread_agent.clone();
            snapshot.state.features.enabled_features = enabled_features.clone();
            let mut merged_toolsets = snapshot.state.tools.loaded_toolsets.clone();
            merged_toolsets.extend(preloaded_toolsets.clone());
            snapshot.state.tools.loaded_toolsets = normalize_loaded_toolsets(merged_toolsets);
            if snapshot.thread.created_at > initialized_at {
                snapshot.thread.created_at = initialized_at;
            }
            if snapshot.thread.updated_at < initialized_at {
                snapshot.thread.updated_at = initialized_at;
            }
            Ok(())
        })
        .await
    }

    pub(crate) async fn initialize_with_messages(
        &mut self,
        initialized_messages: Vec<ChatMessage>,
        initialized_at: DateTime<Utc>,
        thread_agent: ThreadAgent,
        enabled_features: Features,
        preloaded_toolsets: Vec<String>,
    ) -> Result<()> {
        self.apply_persisted_mutation("initialize_thread", |snapshot| {
            if !initialized_messages.is_empty() {
                snapshot.thread.messages = initialized_messages;
                snapshot.thread.created_at = initialized_at;
                snapshot.thread.updated_at = initialized_at;
            }
            snapshot.state.lifecycle.initialized = true;
            snapshot.state.agent = thread_agent.clone();
            snapshot.state.features.enabled_features = enabled_features.clone();
            snapshot.state.tools.loaded_toolsets = normalize_loaded_toolsets(preloaded_toolsets);
            Ok(())
        })
        .await
    }

    pub(crate) async fn reconcile_initialized_feature_state(
        &mut self,
        thread_agent: ThreadAgent,
        enabled_features: Features,
        preloaded_toolsets: Vec<String>,
    ) -> Result<bool> {
        let now = Utc::now();
        self.apply_persisted_mutation("reconcile_initialized_feature_state", |snapshot| {
            let mut changed = false;
            if snapshot.state.agent != thread_agent {
                snapshot.state.agent = thread_agent.clone();
                changed = true;
            }
            if snapshot.state.features.enabled_features != enabled_features {
                snapshot.state.features.enabled_features = enabled_features.clone();
                changed = true;
            }

            let mut merged_toolsets = snapshot.state.tools.loaded_toolsets.clone();
            merged_toolsets.extend(preloaded_toolsets.clone());
            let normalized_toolsets = normalize_loaded_toolsets(merged_toolsets);
            if snapshot.state.tools.loaded_toolsets != normalized_toolsets {
                snapshot.state.tools.loaded_toolsets = normalized_toolsets;
                changed = true;
            }

            if changed {
                snapshot.thread.updated_at = now;
            }
            Ok(changed)
        })
        .await
    }

    /// Push one formal message into the thread-owned persisted message sequence.
    ///
    /// 调用成功返回时，这条消息已经通过 thread-owned CAS 语义完成落盘。
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn demo() -> anyhow::Result<()> {
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     context::{ChatMessage, ChatMessageRole},
    ///     thread::{Thread, ThreadContextLocator},
    /// };
    ///
    /// let now = Utc::now();
    /// let mut thread = Thread::new(
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_ext", "thread_internal"),
    ///     now,
    /// );
    /// thread
    ///     .push_message(ChatMessage::new(ChatMessageRole::User, "hello", now))
    ///     .await?;
    ///
    /// assert_eq!(thread.messages()[0].content, "hello");
    /// # Ok(())
    /// # }
    /// ```
    pub async fn push_message(&mut self, message: ChatMessage) -> Result<()> {
        let external_message_id = self.current_request_external_message_id();
        self.apply_persisted_mutation("push_message", |snapshot| {
            if snapshot.thread.created_at > message.created_at {
                snapshot.thread.created_at = message.created_at;
            }
            snapshot.thread.updated_at = message.created_at;
            snapshot.thread.messages.push(message.clone());
            Ok(())
        })
        .await?;
        info!(
            thread_id = %self.locator.thread_id,
            external_thread_id = %self.locator.external_thread_id,
            external_message_id = ?external_message_id,
            role = message.role.as_label(),
            tool_call_count = message.tool_calls.len(),
            has_tool_call_id = message.tool_call_id.is_some(),
            "persisted formal thread message"
        );
        Ok(())
    }

    /// Replace the persisted non-system history after one compact rewrite.
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn demo() -> anyhow::Result<()> {
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     context::{ChatMessage, ChatMessageRole},
    ///     thread::{Thread, ThreadContextLocator},
    /// };
    ///
    /// let now = Utc::now();
    /// let mut thread = Thread::new(
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_ext", "thread_internal"),
    ///     now,
    /// );
    /// thread
    ///     .push_message(ChatMessage::new(ChatMessageRole::User, "hello", now))
    ///     .await?;
    /// thread
    ///     .replace_messages_after_compaction(vec![
    ///         ChatMessage::new(ChatMessageRole::Assistant, "这是压缩后的上下文", now),
    ///         ChatMessage::new(ChatMessageRole::User, "继续", now),
    ///     ])
    ///     .await?;
    ///
    /// assert_eq!(thread.messages()[0].content, "这是压缩后的上下文");
    /// # Ok(())
    /// # }
    /// ```
    pub async fn replace_messages_after_compaction(
        &mut self,
        compacted_messages: Vec<ChatMessage>,
    ) -> Result<()> {
        let external_message_id = self.current_request_external_message_id();
        self.apply_persisted_mutation("replace_messages_after_compaction", |snapshot| {
            let mut persisted_messages = snapshot
                .thread
                .messages
                .iter()
                .filter(|message| message.role == ChatMessageRole::System)
                .cloned()
                .collect::<Vec<_>>();
            let updated_at = compacted_messages
                .last()
                .map(|message| message.created_at)
                .unwrap_or_else(Utc::now);
            persisted_messages.extend(compacted_messages.clone());
            snapshot.thread.messages = persisted_messages;
            if snapshot.thread.created_at > updated_at {
                snapshot.thread.created_at = updated_at;
            }
            snapshot.thread.updated_at = updated_at;
            Ok(())
        })
        .await?;
        info!(
            thread_id = %self.locator.thread_id,
            external_thread_id = %self.locator.external_thread_id,
            external_message_id = ?external_message_id,
            compacted_message_count = self
                .thread
                .messages
                .iter()
                .filter(|message| message.role != ChatMessageRole::System)
                .count(),
            "rewrote persisted thread non-system history after compaction"
        );
        Ok(())
    }

    /// Replace the thread's loaded toolset state with a normalized snapshot in memory only.
    pub fn replace_loaded_toolsets(&mut self, loaded_toolsets: Vec<String>) {
        self.state.tools.loaded_toolsets = normalize_loaded_toolsets(loaded_toolsets);
    }

    /// Replace the enabled feature state with one explicit snapshot in memory only.
    pub fn replace_enabled_features(&mut self, enabled_features: Features) {
        self.state.features.enabled_features = enabled_features;
    }

    /// Replace the thread agent profile with one explicit snapshot in memory only.
    pub fn replace_thread_agent(&mut self, thread_agent: ThreadAgent) {
        self.state.agent = thread_agent;
    }

    /// Persist one explicit feature snapshot immediately.
    pub async fn persist_enabled_features(&mut self, enabled_features: Features) -> Result<()> {
        let updated_at = Utc::now();
        let feature_names = enabled_features.names();
        self.apply_persisted_mutation("persist_enabled_features", |snapshot| {
            snapshot.state.features.enabled_features = enabled_features.clone();
            snapshot.thread.updated_at = updated_at;
            Ok(())
        })
        .await?;
        info!(
            thread_id = %self.locator.thread_id,
            external_thread_id = %self.locator.external_thread_id,
            enabled_features = ?feature_names,
            "persisted thread enabled feature snapshot"
        );
        Ok(())
    }

    /// Persist one feature flag toggle immediately.
    pub async fn persist_feature_enabled(
        &mut self,
        feature: Feature,
        enabled: bool,
    ) -> Result<bool> {
        let updated_at = Utc::now();
        let changed = self
            .apply_persisted_mutation("persist_feature_enabled", |snapshot| {
                let changed = if enabled {
                    snapshot.state.features.enabled_features.insert(feature)
                } else {
                    snapshot.state.features.enabled_features.remove(feature)
                };
                if changed {
                    snapshot.thread.updated_at = updated_at;
                }
                Ok(changed)
            })
            .await?;
        info!(
            thread_id = %self.locator.thread_id,
            external_thread_id = %self.locator.external_thread_id,
            feature = feature.as_str(),
            enabled,
            changed,
            "persisted thread feature toggle"
        );
        Ok(changed)
    }

    /// Mark one toolset as loaded for the current thread and persist the change atomically.
    pub async fn load_toolset(&mut self, toolset_name: &str) -> Result<bool> {
        let toolset_name = toolset_name.trim().to_string();
        if toolset_name.is_empty() {
            return Ok(false);
        }

        let inserted = self
            .apply_persisted_mutation("load_toolset", |snapshot| {
                let already_loaded = snapshot
                    .state
                    .tools
                    .loaded_toolsets
                    .binary_search_by(|candidate| candidate.as_str().cmp(&toolset_name))
                    .is_ok();
                if !already_loaded {
                    snapshot
                        .state
                        .tools
                        .loaded_toolsets
                        .push(toolset_name.clone());
                    snapshot.state.tools.loaded_toolsets.sort();
                    snapshot.state.tools.loaded_toolsets.dedup();
                    snapshot.thread.updated_at = Utc::now();
                }
                Ok(!already_loaded)
            })
            .await?;
        info!(
            thread_id = %self.locator.thread_id,
            toolset_name = %toolset_name,
            inserted,
            "updated persisted thread loaded toolsets"
        );
        Ok(inserted)
    }

    /// Mark one toolset as unloaded for the current thread and persist the change atomically.
    pub async fn unload_toolset(&mut self, toolset_name: &str) -> Result<bool> {
        let toolset_name = toolset_name.trim().to_string();
        if toolset_name.is_empty() {
            return Ok(false);
        }

        let removed = self
            .apply_persisted_mutation("unload_toolset", |snapshot| {
                let original_len = snapshot.state.tools.loaded_toolsets.len();
                snapshot
                    .state
                    .tools
                    .loaded_toolsets
                    .retain(|candidate| candidate != &toolset_name);
                let removed = original_len != snapshot.state.tools.loaded_toolsets.len();
                if removed {
                    snapshot.thread.updated_at = Utc::now();
                }
                Ok(removed)
            })
            .await?;
        info!(
            thread_id = %self.locator.thread_id,
            toolset_name = %toolset_name,
            removed,
            "updated persisted thread loaded toolsets"
        );
        Ok(removed)
    }

    pub(crate) async fn toolset_catalog_prompt_with_registry(
        &self,
        tool_registry: &ToolRegistry,
    ) -> Option<String> {
        tool_registry
            .render_toolset_catalog_prompt(&self.load_toolsets())
            .await
    }

    pub(crate) async fn visible_tools_with_registry(
        &self,
        tool_registry: &ToolRegistry,
        compact_visible: bool,
    ) -> Result<Vec<ToolDefinition>> {
        let mut definitions = tool_registry
            .always_visible_definitions()
            .await
            .into_iter()
            .filter(|definition| self.tool_allowed_by_feature_state(&definition.name))
            .collect::<Vec<_>>();
        definitions.push(crate::agent::tool::load_toolset_definition());
        definitions.push(crate::agent::tool::unload_toolset_definition());

        for toolset_name in self.load_toolsets() {
            definitions.extend(tool_registry.toolset_definitions(&toolset_name).await?);
        }

        if compact_visible {
            definitions.push(crate::agent::tool::compact_tool_definition());
        }

        definitions.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(definitions)
    }

    async fn load_toolset_via_registry(
        &mut self,
        tool_registry: &ToolRegistry,
        toolset_name: &str,
    ) -> Result<bool> {
        let toolset_name = toolset_name.trim();
        if toolset_name.is_empty() {
            bail!("load_toolset requires a non-empty tool name");
        }

        tool_registry.toolset_definitions(toolset_name).await?;
        self.load_toolset(toolset_name).await
    }

    async fn unload_toolset_via_registry(
        &mut self,
        tool_registry: &ToolRegistry,
        toolset_name: &str,
    ) -> Result<bool> {
        let toolset_name = toolset_name.trim();
        if toolset_name.is_empty() {
            bail!("unload_toolset requires a non-empty tool name");
        }

        let thread_id = self.locator.thread_id.clone();
        let is_loaded = self
            .load_toolsets()
            .into_iter()
            .any(|loaded_name| loaded_name == toolset_name);
        if is_loaded && let Some(runtime) = tool_registry.toolset_runtime(toolset_name).await? {
            runtime.on_unload(&thread_id).await?;
        }

        self.unload_toolset(toolset_name).await
    }

    pub(crate) async fn call_tool_with_registry(
        &mut self,
        tool_registry: &ToolRegistry,
        request: ToolCallRequest,
    ) -> Result<ToolCallResult> {
        let thread_id = self.locator.thread_id.clone();
        let tool_name = request.name.clone();
        let loaded_toolset_count = self.load_toolsets().len();
        let argument_field_count = request
            .arguments
            .as_object()
            .map(|arguments| arguments.len())
            .unwrap_or_default();
        let started_at = std::time::Instant::now();
        tracing::debug!(
            thread_id = %thread_id,
            tool_name = %tool_name,
            loaded_toolset_count,
            argument_field_count,
            "starting thread-owned tool call"
        );

        let result = match request.name.as_str() {
            "compact" => bail!("tool `compact` must be handled by the agent loop compact runtime"),
            "load_toolset" => {
                let toolset_name = request
                    .arguments
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .ok_or_else(|| {
                        anyhow::anyhow!("load_toolset requires a non-empty tool name")
                    })?;
                let inserted = self
                    .load_toolset_via_registry(tool_registry, toolset_name)
                    .await?;
                let loaded_toolsets = self.load_toolsets();
                Ok(ToolCallResult {
                    content: if inserted {
                        format!("Toolset `{toolset_name}` loaded for the current thread.")
                    } else {
                        format!(
                            "Toolset `{toolset_name}` was already loaded for the current thread."
                        )
                    },
                    metadata: json!({
                        "event_kind": "load_toolset",
                        "toolset": toolset_name,
                        "loaded_toolsets": loaded_toolsets,
                        "already_loaded": !inserted,
                        "approval_required": false,
                        "policy_extension_point": true,
                    }),
                    is_error: false,
                })
            }
            "unload_toolset" => {
                let toolset_name = request
                    .arguments
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .ok_or_else(|| {
                        anyhow::anyhow!("unload_toolset requires a non-empty tool name")
                    })?;
                let removed = self
                    .unload_toolset_via_registry(tool_registry, toolset_name)
                    .await?;
                let loaded_toolsets = self.load_toolsets();
                Ok(ToolCallResult {
                    content: if removed {
                        format!("Toolset `{toolset_name}` unloaded for the current thread.")
                    } else {
                        format!("Toolset `{toolset_name}` was not loaded for the current thread.")
                    },
                    metadata: json!({
                        "event_kind": "unload_toolset",
                        "toolset": toolset_name,
                        "loaded_toolsets": loaded_toolsets,
                        "was_loaded": removed,
                        "approval_required": false,
                        "policy_extension_point": true,
                    }),
                    is_error: false,
                })
            }
            _ => {
                let mut context = crate::agent::ToolCallContext::for_thread(thread_id.clone())
                    .with_locator(self.locator.clone());
                if let Some(request_runtime) = self.request_runtime.as_ref() {
                    context = context.with_sessions(request_runtime.sessions.clone());
                }
                if !self.tool_allowed_by_feature_state(&request.name) {
                    bail!(
                        "tool `{}` is not enabled for thread `{}`",
                        request.name,
                        thread_id
                    );
                }
                if let Some(handler) = tool_registry.always_visible_handler(&request.name).await {
                    handler.call_with_context(context, request).await
                } else {
                    let mut resolved_handler = None;
                    for toolset_name in self.load_toolsets() {
                        if let Some(handler) = tool_registry
                            .toolset_handler(&toolset_name, &request.name)
                            .await?
                        {
                            resolved_handler = Some(handler);
                            break;
                        }
                    }
                    let Some(handler) = resolved_handler else {
                        bail!(
                            "tool `{}` is not registered for thread `{}`",
                            request.name,
                            thread_id
                        );
                    };
                    handler.call_with_context(context, request).await
                }
            }
        };

        match &result {
            Ok(tool_result) => tracing::debug!(
                thread_id = %thread_id,
                tool_name = %tool_name,
                loaded_toolset_count,
                argument_field_count,
                elapsed_ms = started_at.elapsed().as_millis() as u64,
                is_error = tool_result.is_error,
                event_kind = ?tool_result
                    .metadata
                    .get("event_kind")
                    .and_then(|value| value.as_str()),
                "completed thread-owned tool call"
            ),
            Err(error) => tracing::debug!(
                thread_id = %thread_id,
                tool_name = %tool_name,
                loaded_toolset_count,
                argument_field_count,
                elapsed_ms = started_at.elapsed().as_millis() as u64,
                error = %error,
                "thread-owned tool call failed"
            ),
        }
        result
    }

    /// Clear the current thread back to one empty initial state in memory only.
    pub fn clear_to_initial_state(&mut self, now: DateTime<Utc>) {
        info!(
            thread_id = %self.locator.thread_id,
            external_thread_id = %self.locator.external_thread_id,
            "clearing thread back to initial state"
        );
        self.thread = ThreadContext::new(now);
        self.state = ThreadState::default();
        self.active_request = None;
        self.request_runtime = None;
    }

    /// Persist one reset that clears the thread back to one empty initial state.
    pub async fn reset_to_initial_state(&mut self, now: DateTime<Utc>) -> Result<()> {
        self.apply_persisted_mutation("reset_to_initial_state", |snapshot| {
            snapshot.thread = ThreadContext::new(now);
            snapshot.state = ThreadState::default();
            Ok(())
        })
        .await?;
        self.active_request = None;
        self.request_runtime = None;
        Ok(())
    }

    /// Persist one reset that clears runtime history while preserving child-thread identity.
    pub async fn reset_to_initial_state_preserving_child_thread(
        &mut self,
        now: DateTime<Utc>,
    ) -> Result<()> {
        self.apply_persisted_mutation(
            "reset_to_initial_state_preserving_child_thread",
            |snapshot| {
                let child_thread = snapshot.state.child_thread.clone();
                snapshot.thread = ThreadContext::new(now);
                snapshot.state = ThreadState::default();
                snapshot.state.child_thread = child_thread;
                Ok(())
            },
        )
        .await?;
        self.active_request = None;
        self.request_runtime = None;
        Ok(())
    }

    /// Replace the active thread snapshot while keeping the current locator.
    #[doc(hidden)]
    pub fn overwrite_active_history(&mut self, replacement: &Thread) {
        self.thread = replacement.thread.clone();
        self.state = replacement.state.clone();
    }

    /// Return the effective auto-compact state for this thread.
    pub fn auto_compact_enabled(&self, default_enabled: bool) -> bool {
        let _ = default_enabled;
        self.is_enabled(Feature::AutoCompact)
    }

    /// Enable one thread feature in memory only.
    pub fn enable_feature(&mut self, feature: Feature) {
        self.state.features.enabled_features.insert(feature);
    }

    /// Disable one thread feature in memory only.
    pub fn disable_feature(&mut self, feature: Feature) {
        self.state.features.enabled_features.remove(feature);
    }

    fn tool_allowed_by_feature_state(&self, tool_name: &str) -> bool {
        match tool_name {
            _ if feature::init::subagent::owns_always_visible_tool(tool_name) => {
                self.child_thread_identity().is_none()
                    && self.thread_agent_kind() == ThreadAgentKind::Main
                    && self.is_enabled(Feature::Subagent)
            }
            _ if feature::init::skill::owns_always_visible_tool(tool_name) => {
                self.is_enabled(Feature::Skill)
            }
            _ => true,
        }
    }
}

impl Deref for Thread {
    type Target = ThreadContext;

    fn deref(&self) -> &Self::Target {
        &self.thread
    }
}

impl DerefMut for Thread {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.thread
    }
}

impl fmt::Debug for Thread {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Thread")
            .field("locator", &self.locator)
            .field("thread", &self.thread)
            .field("state", &self.state)
            .field("revision", &self.revision)
            .field("active_request", &self.active_request)
            .field("request_runtime", &self.request_runtime.is_some())
            .finish()
    }
}

impl PartialEq for Thread {
    fn eq(&self, other: &Self) -> bool {
        self.locator == other.locator
            && self.thread == other.thread
            && self.state == other.state
            && self.revision == other.revision
            && self.active_request == other.active_request
    }
}
