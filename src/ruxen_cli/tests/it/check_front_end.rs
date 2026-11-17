//! `ruxen check` runs the front end `ruxen build` runs (type, borrow and
//! loan checks, MIR lowering), so the two accept and reject the same
//! programs and print the same errors.

use crate::common;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

const CLEAN: &str = "class U\n  name: String\n  def init(@name: String)\n  end\nend\n\n\
                     def reset(u: &var U) -> Int\n  u.name = \"new\"\n  0\nend\n\n\
                     u = U.new(\"old\")\nputs(reset(u))\nputs(u.name)\n";

/// The array literal borrows `u.name` while `reset(u)` mutates `u`: E1002,
/// which only the MIR loan check reports.
const BORROW_ERROR: &str = "class U\n  name: String\n  def init(@name: String)\n  end\nend\n\n\
                            def reset(u: &var U) -> Int\n  u.name = \"new\"\n  0\nend\n\n\
                            u = U.new(\"old\")\nputs([u.name][reset(u)])\n";

fn stage_project(root: &Path, main: &str) {
    fs::write(
        root.join("Ruxen.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2026\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/main.rx"), main).unwrap();
}

fn run_ruxen(project: &Path, command: &str) -> Output {
    Command::new(common::shared_install())
        .arg(command)
        .current_dir(project)
        .output()
        .expect("spawn ruxen")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn check_accepts_a_clean_program_and_rejects_a_borrow_error() {
    let temp = tempfile::tempdir().unwrap();
    stage_project(temp.path(), CLEAN);
    let clean = run_ruxen(temp.path(), "check");
    assert!(clean.status.success(), "{}", stderr(&clean));

    stage_project(temp.path(), BORROW_ERROR);
    let rejected = run_ruxen(temp.path(), "check");
    assert!(!rejected.status.success());
    assert!(stderr(&rejected).contains("E1002"), "{}", stderr(&rejected));
}

#[test]
fn check_and_build_report_the_same_error() {
    let temp = tempfile::tempdir().unwrap();
    stage_project(temp.path(), BORROW_ERROR);

    let check = run_ruxen(temp.path(), "check");
    let build = run_ruxen(temp.path(), "build");
    assert!(!check.status.success() && !build.status.success());
    assert!(
        stderr(&check).contains("src/main.rx: "),
        "{}",
        stderr(&check)
    );
    assert_eq!(stderr(&check), stderr(&build));
}
