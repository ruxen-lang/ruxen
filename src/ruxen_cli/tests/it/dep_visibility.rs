//! Q16 acceptance: dependency symbols must be visible to LIBRARY builds,
//! `ruxen check`, and `ruxen test` — not just to binary builds.
//!
//! Until Q16, only `compile_project` (the binary path) flat-merged a
//! dependency package's sources into the consuming compilation unit. A
//! library, its `ruxen check`, and its `ruxen test` saw none of the
//! dependency's symbols, so a library could not `use` a path-dependency
//! type in `src/lib.rx` or in a `tests/**.rx` file.
//!
//! These tests stage an isolated install of the FRESHLY-BUILT
//! `CARGO_BIN_EXE_ruxen` (target/, never the installed `~/.ruxen` one),
//! scaffold a two-package layout — a `dep-color` library exposing
//! `struct Color`, and a `consumer` library that `use`s it — and exercise
//! `build`, `check`, and `test` in the consumer. This is a real
//! codegen + link path, so a duplicate-symbol or double-link regression
//! would fail the link, not just the type check.
//!
//! Uses the staged install from `common::shared_install` (which see for
//! the OnceLock / ETXTBUSY rationale).

use crate::common;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

/// Lay out a workspace with:
///   dep-color/  (library)  — exports `struct Color` + a free fn `red()`
///   consumer/   (library)  — path-depends on dep-color, `use`s Color in
///                            src/lib.rx AND in tests/color_test.rx
/// Returns the consumer project dir.
fn stage_two_package_workspace(root: &Path) -> PathBuf {
    // --- dependency library ---
    write(
        &root.join("dep-color/Ruxen.toml"),
        "[package]\nname = \"dep-color\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n\
         [build]\ntype = \"library\"\n",
    );
    write(
        &root.join("dep-color/src/lib.rx"),
        "struct Color\n  r: Int\n  g: Int\n  b: Int\nend\n\n\
         def red -> Color\n  Color.new(255, 0, 0)\nend\n",
    );

    // --- consumer library that depends on it ---
    write(
        &root.join("consumer/Ruxen.toml"),
        "[package]\nname = \"consumer\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n\
         [build]\ntype = \"library\"\n\n\
         [dependencies]\ndep-color = { path = \"../dep-color\" }\n",
    );
    // src/lib.rx references the dependency's symbol — this is the line that
    // does not type-check before Q16 in a LIBRARY build.
    write(
        &root.join("consumer/src/lib.rx"),
        "def brightest -> Int\n  let c = red\n  c.r\nend\n",
    );
    // A test file that uses the dependency symbol directly and asserts on it.
    write(
        &root.join("consumer/tests/color_test.rx"),
        "Tester.describe(\"color\") do |t: &var Tester|\n  \
           t.it(\"sees the dependency symbol\") do\n    \
             t.expect(red().r).to_eq(255)\n    \
             t.expect(brightest()).to_eq(255)\n  \
           end\nend\n",
    );

    root.join("consumer")
}

fn run_ruxen(ruxen: &Path, project: &Path, args: &[&str]) -> std::process::Output {
    Command::new(ruxen)
        .args(args)
        .current_dir(project)
        .output()
        .expect("spawn ruxen")
}

#[test]
fn library_build_sees_dependency_symbol() {
    let temp = tempfile::tempdir().unwrap();
    let ruxen = common::shared_install();
    let consumer = stage_two_package_workspace(temp.path());

    let out = run_ruxen(ruxen, &consumer, &["build"]);
    assert!(
        out.status.success(),
        "library `ruxen build` should see the dependency symbol.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

#[test]
fn library_check_sees_dependency_symbol() {
    let temp = tempfile::tempdir().unwrap();
    let ruxen = common::shared_install();
    let consumer = stage_two_package_workspace(temp.path());

    let out = run_ruxen(ruxen, &consumer, &["check"]);
    assert!(
        out.status.success(),
        "`ruxen check` should see the dependency symbol.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

#[test]
fn library_test_sees_dependency_symbol_and_runs() {
    let temp = tempfile::tempdir().unwrap();
    let ruxen = common::shared_install();
    let consumer = stage_two_package_workspace(temp.path());

    let out = run_ruxen(ruxen, &consumer, &["test"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "`ruxen test` should compile + run a test that uses the dependency symbol.\n\
         stdout:\n{}\nstderr:\n{}",
        stdout,
        stderr,
    );
    // The test asserts on real dependency values; make sure a case ran and
    // none failed (the summary line is "N passed; M failed; K pending").
    assert!(
        stdout.contains("1 passed") || stdout.contains("passed"),
        "expected a passing test summary; got stdout:\n{}\nstderr:\n{}",
        stdout,
        stderr,
    );
    assert!(
        !stdout.contains("FAILED") && !stdout.contains("1 failed"),
        "no test should fail; got stdout:\n{}\nstderr:\n{}",
        stdout,
        stderr,
    );
}
