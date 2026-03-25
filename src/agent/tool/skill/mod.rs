//! Local skill discovery, progressive loading, and `load_skill` tool integration.

mod registry;
mod tool;

pub use registry::{LoadedSkill, LoadedSkillFile, SkillManifest, SkillRegistry};
pub use tool::LoadSkillTool;
