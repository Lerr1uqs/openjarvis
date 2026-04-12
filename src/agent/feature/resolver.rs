//! Resolve thread features from `channel + user` config and development defaults.

use crate::{
    config::AppConfig,
    thread::{Features, ThreadContextLocator},
};
use std::collections::BTreeMap;
use tracing::{info, warn};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct FeatureScope {
    channel: String,
    user_id: String,
}

/// Resolve thread features from `channel + user` with one explicit development fallback.
#[derive(Debug, Clone)]
pub struct FeatureResolver {
    available_features: Features,
    scoped_overrides: BTreeMap<FeatureScope, Features>,
}

impl FeatureResolver {
    /// Create a resolver that defaults missing config to all available features.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::FeatureResolver;
    /// use openjarvis::thread::{Feature, Features};
    ///
    /// let resolver = FeatureResolver::development_default(Features::from_iter([
    ///     Feature::Memory,
    ///     Feature::Skill,
    /// ]));
    /// let resolved = resolver.resolve("feishu", "ou_demo");
    /// assert!(resolved.contains(Feature::Memory));
    /// assert!(resolved.contains(Feature::Skill));
    /// ```
    pub fn development_default(available_features: Features) -> Self {
        Self {
            available_features,
            scoped_overrides: BTreeMap::new(),
        }
    }

    /// Build a resolver from the current app config snapshot.
    pub fn from_app_config(config: &AppConfig, available_features: Features) -> Self {
        let mut scoped_overrides = BTreeMap::new();
        for (user_id, user_config) in config.channel_config().feishu_config().users() {
            let Some(features) = user_config.features() else {
                continue;
            };
            let normalized = features.intersect(&available_features);
            if normalized != *features {
                warn!(
                    channel = "feishu",
                    user_id,
                    configured_features = ?features.names(),
                    available_features = ?available_features.names(),
                    resolved_features = ?normalized.names(),
                    "filtered unavailable configured thread features"
                );
            }
            scoped_overrides.insert(
                FeatureScope {
                    channel: "feishu".to_string(),
                    user_id: user_id.clone(),
                },
                normalized,
            );
        }

        Self {
            available_features,
            scoped_overrides,
        }
    }

    /// Resolve the effective feature set for one `channel + user` pair.
    pub fn resolve(&self, channel: &str, user_id: &str) -> Features {
        let scope = FeatureScope {
            channel: channel.trim().to_string(),
            user_id: user_id.trim().to_string(),
        };
        if let Some(explicit) = self.scoped_overrides.get(&scope) {
            info!(
                channel = %scope.channel,
                user_id = %scope.user_id,
                enabled_features = ?explicit.names(),
                "resolved thread features from explicit channel-user config"
            );
            return explicit.clone();
        }

        info!(
            channel = %scope.channel,
            user_id = %scope.user_id,
            enabled_features = ?self.available_features.names(),
            "resolved thread features from development default fallback"
        );
        self.available_features.clone()
    }

    pub fn resolve_for_locator(&self, locator: &ThreadContextLocator) -> Features {
        self.resolve(&locator.channel, &locator.user_id)
    }

    pub fn available_features(&self) -> &Features {
        &self.available_features
    }
}
