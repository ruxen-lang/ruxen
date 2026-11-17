//! End-to-end tests for the `ruxen` package-manager binary.
//!
//! Stages an isolated install layout (`bin/ruxen` + `lib/runtime.c`),
//! scaffolds a project with `ruxen new`, then builds and runs it. This
//! catches regressions in the package-manager → compiler handoff — notably
//! that the compiler locates `runtime.c` at runtime rather than at build
//! time.

use crate::common;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Return a fresh per-test tempdir for project files and the path to the
/// process-wide shared staged `ruxen` binary.
fn stage_install() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().expect("mktemp");
    (temp, common::shared_install().to_path_buf())
}

#[test]
fn version_flag() {
    let (_temp, ruxen) = stage_install();
    let out = Command::new(&ruxen).arg("--version").output().unwrap();
    assert!(out.status.success(), "ruxen --version exited nonzero");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("ruxen "), "unexpected: {:?}", stdout);
}

#[test]
fn new_build_run_roundtrip() {
    let (temp, ruxen) = stage_install();

    let project_parent = temp.path().join("work");
    fs::create_dir_all(&project_parent).unwrap();

    // ruxen new hello
    let new_out = Command::new(&ruxen)
        .arg("new")
        .arg("hello")
        .current_dir(&project_parent)
        .output()
        .expect("spawn ruxen new");
    assert!(
        new_out.status.success(),
        "ruxen new failed:\n{}",
        String::from_utf8_lossy(&new_out.stderr),
    );

    let project = project_parent.join("hello");
    assert!(project.join("Ruxen.toml").exists());
    assert!(project.join("src/main.rx").exists());

    // ruxen build
    let build = Command::new(&ruxen)
        .arg("build")
        .current_dir(&project)
        .output()
        .expect("spawn ruxen build");
    assert!(
        build.status.success(),
        "ruxen build failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr),
    );

    // ruxen run
    let run = Command::new(&ruxen)
        .arg("run")
        .current_dir(&project)
        .output()
        .expect("spawn ruxen run");
    assert!(
        run.status.success(),
        "ruxen run failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr),
    );

    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        stdout.contains("Hello, Ruxen!"),
        "scaffolded program didn't produce expected output. got:\n{}",
        stdout,
    );
}

#[test]
fn run_after_editing_main() {
    // Ensures `ruxen build` picks up source edits (no stale cache
    // preventing recompilation for a changed file).
    let (temp, ruxen) = stage_install();
    let project_parent = temp.path().join("work");
    fs::create_dir_all(&project_parent).unwrap();

    let new_out = Command::new(&ruxen)
        .arg("new")
        .arg("edits")
        .current_dir(&project_parent)
        .output()
        .unwrap();
    assert!(new_out.status.success());

    let project = project_parent.join("edits");
    let main = project.join("src/main.rx");
    fs::write(&main, "def main\n  puts \"first\"\nend\n").unwrap();

    let first = Command::new(&ruxen)
        .arg("run")
        .current_dir(&project)
        .output()
        .unwrap();
    assert!(first.status.success());
    assert!(String::from_utf8_lossy(&first.stdout).contains("first"));

    fs::write(&main, "def main\n  puts \"second\"\nend\n").unwrap();

    let second = Command::new(&ruxen)
        .arg("run")
        .current_dir(&project)
        .output()
        .unwrap();
    assert!(second.status.success());
    let stdout = String::from_utf8_lossy(&second.stdout);
    assert!(
        stdout.contains("second"),
        "edits didn't take effect. got:\n{}",
        stdout,
    );
}

#[test]
fn example_cli_utility_runs_from_staged_install() {
    let (_temp, ruxen) = stage_install();
    let example = common::workspace_root().join("examples/01-cli-utility");

    let run = Command::new(&ruxen)
        .arg("run")
        .arg("--")
        .arg("README.md")
        .current_dir(&example)
        .output()
        .expect("spawn ruxen run for example");

    assert!(
        run.status.success(),
        "example run failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr),
    );

    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        stdout.contains("Prints a file to stdout using the current Tier-1 stdlib surface."),
        "example output did not include README content. got:\n{}",
        stdout,
    );
}
