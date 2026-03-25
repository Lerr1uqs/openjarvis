use std::{
    env::temp_dir,
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

mod registry;
mod tool;

pub(crate) struct SkillFixture {
    root: PathBuf,
    skills_root: PathBuf,
}

impl SkillFixture {
    pub(crate) fn new(prefix: &str) -> Self {
        let root = temp_dir().join(format!("{prefix}-{}", Uuid::new_v4()));
        let skills_root = root.join(".skills");
        fs::create_dir_all(&skills_root).expect("skills root should be created");
        Self { root, skills_root }
    }

    pub(crate) fn skills_root(&self) -> &Path {
        &self.skills_root
    }

    pub(crate) fn write_skill(&self, dir_name: &str, skill_md: &str) -> PathBuf {
        let skill_dir = self.skills_root.join(dir_name);
        fs::create_dir_all(&skill_dir).expect("skill directory should be created");
        let skill_file = skill_dir.join("SKILL.md");
        fs::write(&skill_file, skill_md).expect("skill file should be written");
        skill_file
    }

    pub(crate) fn write_skill_file(
        &self,
        dir_name: &str,
        relative_path: &str,
        content: &str,
    ) -> PathBuf {
        let path = self.skills_root.join(dir_name).join(relative_path);
        fs::create_dir_all(path.parent().expect("skill file parent should exist"))
            .expect("skill file parent directory should be created");
        fs::write(&path, content).expect("skill file content should be written");
        path
    }
}

impl Drop for SkillFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
