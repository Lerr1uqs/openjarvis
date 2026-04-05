use std::{
    env::temp_dir,
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use uuid::Uuid;

struct SkillFrontmatterDumpFixture {
    root: PathBuf,
}

impl SkillFrontmatterDumpFixture {
    fn new(prefix: &str) -> Self {
        let root = temp_dir().join(format!("{prefix}-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join(".openjarvis/skills"))
            .expect("skill dump fixture root should be created");
        Self { root }
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn write_skill(&self, dir_name: &str, skill_md: &str) -> PathBuf {
        let skill_dir = self.root.join(".openjarvis/skills").join(dir_name);
        fs::create_dir_all(&skill_dir).expect("fixture skill dir should be created");
        let skill_file = skill_dir.join("SKILL.md");
        fs::write(&skill_file, skill_md).expect("fixture skill file should be written");
        skill_file
    }
}

impl Drop for SkillFrontmatterDumpFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn skill_frontmatter_dump_bin_renders_readable_table_by_default() {
    // 验证场景: 默认输出应由第三方终端表格库渲染，包含 UTF-8 边框和结构化列。
    let fixture = SkillFrontmatterDumpFixture::new("openjarvis-skill-frontmatter-dump");
    fixture.write_skill(
        "zeta",
        r#"---
name: zeta
description: zeta description
---
zeta body
"#,
    );
    fixture.write_skill(
        "alpha",
        r#"---
name: alpha
description: alpha description
---
alpha body
"#,
    );
    fixture.write_skill(
        "broken",
        r#"---
name: broken
---
broken body
"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_skill_frontmatter_dump"))
        .arg("--workspace")
        .arg(fixture.root())
        .arg("--width")
        .arg("80")
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")))
        .output()
        .expect("skill_frontmatter_dump binary should run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("╭"));
    assert!(stdout.contains("╰"));
    assert!(stdout.lines().any(|line| line.contains("│ # ")
        && line.contains("name")
        && line.contains("description")));
    assert!(stdout.lines().any(|line| line.contains("│ 1 ")
        && line.contains("alpha")
        && line.contains("alpha description")));
    assert!(stdout.lines().any(|line| line.contains("│ 2 ")
        && line.contains("zeta")
        && line.contains("zeta description")));
    assert!(stdout.contains("2 skill(s) from"));
}

#[test]
fn skill_frontmatter_dump_bin_supports_plain_machine_readable_output() {
    // 验证场景: 需要脚本链路消费时，`--plain` 应保留稳定的 `name<TAB>description` 输出。
    let fixture = SkillFrontmatterDumpFixture::new("openjarvis-skill-frontmatter-dump-plain");
    fixture.write_skill(
        "alpha",
        r#"---
name: alpha
description: alpha description
---
alpha body
"#,
    );
    fixture.write_skill(
        "zeta",
        r#"---
name: zeta
description: zeta description
---
zeta body
"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_skill_frontmatter_dump"))
        .arg("--workspace")
        .arg(fixture.root())
        .arg("--plain")
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")))
        .output()
        .expect("skill_frontmatter_dump binary should run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        vec!["alpha\talpha description", "zeta\tzeta description"]
    );
}

#[test]
fn skill_frontmatter_dump_bin_reports_empty_workspace_in_table_mode() {
    // 验证场景: 当工作区下没有任何 skill 时，默认表格模式应给出明确提示，而不是静默空输出。
    let fixture = SkillFrontmatterDumpFixture::new("openjarvis-skill-frontmatter-dump-empty");

    let output = Command::new(env!("CARGO_BIN_EXE_skill_frontmatter_dump"))
        .arg("--workspace")
        .arg(fixture.root())
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")))
        .output()
        .expect("skill_frontmatter_dump binary should run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("No local skills found under"));
}
