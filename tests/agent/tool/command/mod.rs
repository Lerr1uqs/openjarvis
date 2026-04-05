mod output;
mod process;
mod session;
mod tool;

use std::{path::PathBuf, process::Command};

pub(crate) fn command_session_fixture() -> Option<String> {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/command_session_sum_tui.py");
    for python in ["python3", "python"] {
        if Command::new(python).arg("--version").output().is_ok() {
            return Some(format!("{python} {}", fixture.display()));
        }
    }
    None
}
