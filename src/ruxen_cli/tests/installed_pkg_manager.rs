//! End-to-end tests for the `ruxen` package-manager binary.
//!
//! Stages an isolated install layout (`bin/ruxen` + `lib/runtime.c`),
//! scaffolds a project with `ruxen new`, then builds and runs it. This
//! catches regressions in the package-manager → compiler handoff — notably
//! that the compiler locates `runtime.c` at runtime rather than at build
//! time.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use tempfile::TempDir;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn stdlib_root_src() -> PathBuf {
    workspace_root().join("library/std")
}

fn ruxen_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ruxen"))
}

/// Shared, pre-staged install layout for all tests in this binary.
///
/// On Linux, parallel `fs::copy + Command::new().spawn()` races into
/// ETXTBUSY because a sibling thread's fork inherits the still-open write
/// fd. Staging exactly once via `OnceLock` eliminates the race: every test
/// spawns from a binary that was fully written and closed before any test
/// started.
fn shared_install() -> &'static Path {
    static INSTALL: OnceLock<(TempDir, PathBuf)> = OnceLock::new();
    &INSTALL
        .get_or_init(|| {
            let temp = tempfile::tempdir().expect("mktemp shared install");
            let bin_dir = temp.path().join("bin");
            let lib_dir = temp.path().join("lib");
            fs::create_dir_all(&bin_dir).unwrap();
            fs::create_dir_all(&lib_dir).unwrap();

            let staged = bin_dir.join("ruxen");
            fs::copy(ruxen_exe(), &staged).expect("copy ruxen");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(&staged).unwrap().permissions();
                perms.set_mode(0o755);
                fs::set_permissions(&staged, perms).unwrap();
            }

            // Mirror the full `library/std/` tree into `<install>/lib/std/`.
            // After #06.95 Phase B, each stdlib package owns its own
            // `runtime/` subdirectory; runtime.c (still a unity build for
            // the install layout) `#include`s sibling packages via
            // `../../<pkg>/runtime/<file>.c`, so the whole `std/` tree
            // must be present at the destination to preserve the relative
            // include paths.
            let lib_std_dir = lib_dir.join("std");
            copy_runtime_tree(&stdlib_root_src(), &lib_std_dir);
            (temp, staged)
        })
        .1
}

fn copy_runtime_tree(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let dest = dst.join(entry.file_name());
        if path.is_dir() {
            copy_runtime_tree(&path, &dest);
        } else {
            fs::copy(&path, &dest).expect("copy runtime file");
        }
    }
}

/// Return a fresh per-test tempdir for project files and the path to the
/// process-wide shared staged `ruxen` binary.
fn stage_install() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().expect("mktemp");
    (temp, shared_install().to_path_buf())
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
    let example = workspace_root().join("examples/01-cli-utility");

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
