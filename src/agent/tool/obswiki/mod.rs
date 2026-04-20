//! Obsidian-backed `obswiki` toolset runtime, vault helpers, and CLI integration.

pub mod runtime;
pub mod tool;

pub use runtime::{
    OBSWIKI_AGENTS_FILE_NAME, OBSWIKI_INDEX_FILE_NAME, OBSWIKI_RAW_DIR_NAME,
    OBSWIKI_SCHEMA_DIR_NAME, OBSWIKI_SCHEMA_README_RELATIVE_PATH, OBSWIKI_WIKI_DIR_NAME,
    ObswikiDocument, ObswikiDocumentMetadata, ObswikiPreflightStatus, ObswikiRuntime,
    ObswikiRuntimeConfig, ObswikiSearchCandidate, ObswikiSearchResponse, ObswikiUpdateInstruction,
    ObswikiVaultContext, ObswikiVaultLayout, is_mutable_obswiki_path, is_raw_obswiki_path,
    parse_obswiki_update_instruction, validate_obswiki_markdown_path,
};
pub use tool::{
    ObswikiToolsetRuntime, register_obswiki_toolset_with_config, run_internal_obswiki_command,
};
