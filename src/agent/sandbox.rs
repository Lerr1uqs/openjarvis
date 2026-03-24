//! Placeholder sandbox container types owned by the agent worker.

/// Dummy sandbox container held by `AgentWorker` before a real sandbox runtime exists.
///
/// # 示例
/// ```rust
/// use openjarvis::agent::DummySandboxContainer;
///
/// let sandbox = DummySandboxContainer::new();
/// assert!(sandbox.is_placeholder());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DummySandboxContainer;

impl DummySandboxContainer {
    /// Create the placeholder sandbox container used by the current worker implementation.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::DummySandboxContainer;
    ///
    /// let sandbox = DummySandboxContainer::new();
    /// assert_eq!(sandbox.kind(), "dummy");
    /// ```
    pub fn new() -> Self {
        Self
    }

    /// Return the stable sandbox kind label for diagnostics and tests.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::DummySandboxContainer;
    ///
    /// let sandbox = DummySandboxContainer::new();
    /// assert_eq!(sandbox.kind(), "dummy");
    /// ```
    pub fn kind(&self) -> &'static str {
        "dummy"
    }

    /// Return whether this sandbox container is only a placeholder implementation.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::DummySandboxContainer;
    ///
    /// let sandbox = DummySandboxContainer::new();
    /// assert!(sandbox.is_placeholder());
    /// ```
    pub fn is_placeholder(&self) -> bool {
        true
    }
}

impl Default for DummySandboxContainer {
    fn default() -> Self {
        Self::new()
    }
}
