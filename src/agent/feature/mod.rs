//! Thread feature helpers split into initialization helpers and resolver logic.
//!
//! `ThreadRuntime` owns the actual `init_thread(...)` orchestration. This module only provides
//! feature-scoped helpers such as resolver logic plus grouped initialization helpers.

pub mod init;
pub mod resolver;

pub use init::auto_compact::AutoCompactor;
pub use init::shell_env::ShellEnv;
pub use resolver::FeatureResolver;
