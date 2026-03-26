//! Program-defined toolset catalog types and thread-scoped loaded-toolset state.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use tokio::sync::RwLock;

/// One compact catalog entry describing a program-defined toolset.
///
/// # 示例
/// ```rust
/// use openjarvis::agent::ToolsetCatalogEntry;
///
/// let entry = ToolsetCatalogEntry::new("browser", "Browser automation tools");
/// assert_eq!(entry.name, "browser");
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolsetCatalogEntry {
    pub name: String,
    pub description: String,
}

impl ToolsetCatalogEntry {
    /// Create one catalog entry from a stable name and short description.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
        }
    }
}

/// Persistable snapshot of the loaded toolsets for one internal thread.
///
/// # 示例
/// ```rust
/// use openjarvis::agent::ThreadToolRuntimeSnapshot;
///
/// let snapshot = ThreadToolRuntimeSnapshot::new(vec!["browser".to_string()]);
/// assert_eq!(snapshot.loaded_toolsets, vec!["browser".to_string()]);
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadToolRuntimeSnapshot {
    #[serde(default)]
    pub loaded_toolsets: Vec<String>,
}

impl ThreadToolRuntimeSnapshot {
    /// Create a normalized snapshot with sorted, deduplicated toolset names.
    pub fn new(loaded_toolsets: Vec<String>) -> Self {
        Self {
            loaded_toolsets: normalize_toolset_names(loaded_toolsets),
        }
    }
}

/// Thread-scoped tool runtime manager keyed by internal thread id.
///
/// # 示例
/// ```rust,no_run
/// # async fn demo() {
/// use openjarvis::agent::ThreadToolRuntimeManager;
///
/// let manager = ThreadToolRuntimeManager::new();
/// manager.load_toolset("thread-1", "browser").await;
/// assert_eq!(manager.loaded_toolsets("thread-1").await, vec!["browser".to_string()]);
/// # }
/// ```
#[derive(Debug, Default)]
pub struct ThreadToolRuntimeManager {
    states: RwLock<HashMap<String, ThreadToolRuntimeSnapshot>>,
}

impl ThreadToolRuntimeManager {
    /// Create an empty thread runtime manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace one thread's loaded toolsets from persisted state.
    pub async fn replace_loaded_toolsets(
        &self,
        thread_id: &str,
        loaded_toolsets: &[String],
    ) -> ThreadToolRuntimeSnapshot {
        let snapshot = ThreadToolRuntimeSnapshot::new(loaded_toolsets.to_vec());
        let mut states = self.states.write().await;
        states.insert(thread_id.to_string(), snapshot.clone());
        snapshot
    }

    /// Return a clone of one thread runtime snapshot.
    pub async fn snapshot(&self, thread_id: &str) -> ThreadToolRuntimeSnapshot {
        let states = self.states.read().await;
        states.get(thread_id).cloned().unwrap_or_default()
    }

    /// Return the currently loaded toolset names for one thread.
    pub async fn loaded_toolsets(&self, thread_id: &str) -> Vec<String> {
        self.snapshot(thread_id).await.loaded_toolsets
    }

    /// Mark one toolset as loaded for the target internal thread.
    pub async fn load_toolset(&self, thread_id: &str, toolset_name: &str) -> bool {
        let mut states = self.states.write().await;
        let snapshot = states.entry(thread_id.to_string()).or_default();
        let inserted = snapshot
            .loaded_toolsets
            .binary_search_by(|candidate| candidate.as_str().cmp(toolset_name))
            .is_err();
        if inserted {
            snapshot.loaded_toolsets.push(toolset_name.to_string());
            snapshot.loaded_toolsets.sort();
            snapshot.loaded_toolsets.dedup();
        }
        inserted
    }

    /// Mark one toolset as unloaded for the target internal thread.
    pub async fn unload_toolset(&self, thread_id: &str, toolset_name: &str) -> bool {
        let mut states = self.states.write().await;
        let Some(snapshot) = states.get_mut(thread_id) else {
            return false;
        };
        let original_len = snapshot.loaded_toolsets.len();
        snapshot
            .loaded_toolsets
            .retain(|candidate| candidate != toolset_name);
        original_len != snapshot.loaded_toolsets.len()
    }
}

fn normalize_toolset_names<I>(loaded_toolsets: I) -> Vec<String>
where
    I: IntoIterator<Item = String>,
{
    let mut names = loaded_toolsets
        .into_iter()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    names.sort();
    names
}
