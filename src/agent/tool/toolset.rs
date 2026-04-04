//! Program-defined toolset catalog types.

use serde::{Deserialize, Serialize};

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
