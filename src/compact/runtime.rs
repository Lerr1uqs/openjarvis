//! Thread-scoped runtime switches for compact behavior.
//!
//! Static YAML config defines the default compact policy, while this module stores live overrides
//! that can be toggled at runtime for one channel/user/external-thread scope.

use crate::{model::IncomingMessage, session::ThreadLocator, thread::ThreadContext};
use std::collections::HashMap;
use tokio::sync::RwLock;
use tracing::info;

/// Stable thread-scoped key shared between runtime commands and the agent loop.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct CompactScopeKey {
    pub channel: String,
    pub user_id: String,
    pub external_thread_id: String,
}

impl CompactScopeKey {
    /// Build one explicit compact scope key.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::compact::CompactScopeKey;
    ///
    /// let key = CompactScopeKey::new("feishu", "ou_xxx", "thread_1");
    /// assert_eq!(key.external_thread_id, "thread_1");
    /// ```
    pub fn new(
        channel: impl Into<String>,
        user_id: impl Into<String>,
        external_thread_id: impl Into<String>,
    ) -> Self {
        Self {
            channel: channel.into(),
            user_id: user_id.into(),
            external_thread_id: external_thread_id.into(),
        }
    }

    /// Build a compact scope key from one external incoming message.
    ///
    /// This is used by slash commands before the message enters the session layer.
    pub fn from_incoming(incoming: &IncomingMessage) -> Self {
        Self::new(
            incoming.channel.clone(),
            incoming.user_id.clone(),
            incoming.resolved_external_thread_id(),
        )
    }

    /// Build a compact scope key from one resolved thread locator.
    ///
    /// This keeps the agent loop and the command layer aligned on the same thread scope.
    pub fn from_locator(locator: &ThreadLocator) -> Self {
        Self::new(
            locator.channel.clone(),
            locator.user_id.clone(),
            locator.external_thread_id.clone(),
        )
    }
}

/// Thread-scoped runtime overrides applied on top of static compact config.
#[derive(Default)]
pub struct CompactRuntimeManager {
    compact_enabled_overrides: RwLock<HashMap<CompactScopeKey, bool>>,
    auto_compact_overrides: RwLock<HashMap<CompactScopeKey, bool>>,
}

impl CompactRuntimeManager {
    /// Create an empty runtime override manager.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::compact::CompactRuntimeManager;
    ///
    /// let manager = CompactRuntimeManager::new();
    /// let _ = manager;
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Merge deprecated scope-keyed overrides into one live `ThreadContext`.
    #[allow(deprecated)]
    pub async fn merge_legacy_scope_overrides(
        &self,
        scope: &CompactScopeKey,
        thread_context: &mut ThreadContext,
    ) {
        if let Some(enabled) = self.compact_enabled_override(scope).await {
            thread_context.set_compact_enabled_override(Some(enabled));
        }
        if let Some(enabled) = self.auto_compact_override(scope).await {
            thread_context.set_auto_compact_override(Some(enabled));
        }
    }

    /// Rebuild deprecated scope-keyed override caches from the persisted `ThreadContext`.
    pub async fn sync_legacy_scope_overrides(
        &self,
        scope: &CompactScopeKey,
        thread_context: &ThreadContext,
    ) {
        let compact_enabled_override = thread_context.state.features.compact_enabled_override;
        let auto_compact_override = thread_context.state.features.auto_compact_override;

        let mut compact_enabled_overrides = self.compact_enabled_overrides.write().await;
        match compact_enabled_override {
            Some(enabled) => {
                compact_enabled_overrides.insert(scope.clone(), enabled);
            }
            None => {
                compact_enabled_overrides.remove(scope);
            }
        }
        drop(compact_enabled_overrides);

        let mut auto_compact_overrides = self.auto_compact_overrides.write().await;
        match auto_compact_override {
            Some(enabled) => {
                auto_compact_overrides.insert(scope.clone(), enabled);
            }
            None => {
                auto_compact_overrides.remove(scope);
            }
        }
    }

    /// Set the thread-scoped compact-enabled override for one scope.
    #[deprecated(note = "use ThreadContext::set_compact_enabled_override instead")]
    pub async fn set_compact_enabled(&self, scope: CompactScopeKey, enabled: bool) {
        info!(
            channel = scope.channel,
            user_id = scope.user_id,
            external_thread_id = scope.external_thread_id,
            enabled,
            "updated runtime compact-enabled override"
        );
        self.compact_enabled_overrides
            .write()
            .await
            .insert(scope, enabled);
    }

    /// Return the explicit thread-scoped compact-enabled override when present.
    #[deprecated(note = "use ThreadContext::compact_enabled instead")]
    pub async fn compact_enabled_override(&self, scope: &CompactScopeKey) -> Option<bool> {
        self.compact_enabled_overrides
            .read()
            .await
            .get(scope)
            .copied()
    }

    /// Return the effective compact-enabled state for one scope.
    ///
    /// `default_enabled` is the static config value loaded from YAML.
    #[deprecated(note = "use ThreadContext::compact_enabled instead")]
    pub async fn compact_enabled(&self, scope: &CompactScopeKey, default_enabled: bool) -> bool {
        self.compact_enabled_override(scope)
            .await
            .unwrap_or(default_enabled)
    }

    /// Set the thread-scoped auto-compact override for one scope.
    #[deprecated(note = "use ThreadContext::set_auto_compact_override instead")]
    pub async fn set_auto_compact(&self, scope: CompactScopeKey, enabled: bool) {
        info!(
            channel = scope.channel,
            user_id = scope.user_id,
            external_thread_id = scope.external_thread_id,
            enabled,
            "updated runtime auto-compact override"
        );
        self.auto_compact_overrides
            .write()
            .await
            .insert(scope, enabled);
    }

    /// Return the explicit thread-scoped auto-compact override when present.
    #[deprecated(note = "use ThreadContext::auto_compact_enabled instead")]
    pub async fn auto_compact_override(&self, scope: &CompactScopeKey) -> Option<bool> {
        self.auto_compact_overrides.read().await.get(scope).copied()
    }

    /// Return the effective auto-compact state for one scope.
    ///
    /// `default_enabled` is the static config value loaded from YAML.
    #[deprecated(note = "use ThreadContext::auto_compact_enabled instead")]
    pub async fn auto_compact_enabled(
        &self,
        scope: &CompactScopeKey,
        default_enabled: bool,
    ) -> bool {
        self.auto_compact_override(scope)
            .await
            .unwrap_or(default_enabled)
    }
}
