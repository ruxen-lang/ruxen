//! End-to-end tests for the `ruxen repl` subcommand.
//!
//! These drive the unified `ruxen` binary via `ruxen repl` over stdin and
//! check that its command protocol (`:help`, `:type`, `:quit`) and basic
//! expression evaluation continue to work. Actual JIT execution needs W+X
//! memory permissions which aren't always available in sandboxes, so
//! evaluation tests are gated behind a permissiveness probe.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Path to the unified `ruxen` driver. The REPL runs as `ruxen repl`.
fn ruxen_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ruxen"))
}

/// Run `ruxen repl` with the given stdin input, return (stdout, stderr).
fn run_repl(stdin_input: &str) -> (String, String) {
    let mut child = Command::new(ruxen_exe())
        .arg("repl")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn ruxen repl");

    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(stdin_input.as_bytes())
        .unwrap();

    let out = child.wait_with_output().expect("wait repl");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn version_flag() {
    // `ruxen repl --version` is intercepted by clap on the parent `ruxen`
    // CLI before the REPL ever starts; the version string therefore
    // comes from the unified `ruxen` package.
    let out = Command::new(ruxen_exe())
        .args(["repl", "--version"])
        .output()
        .expect("spawn ruxen repl --version");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.starts_with("ruxen "),
        "unexpected: {:?}",
        stdout,
    );
}

#[test]
fn help_flag() {
    let out = Command::new(ruxen_exe())
        .args(["repl", "--help"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Clap auto-generates the subcommand help; it mentions the parent
    // binary plus the subcommand path.
    assert!(stdout.contains("repl"));
}

#[test]
fn banner_and_quit() {
    let (stdout, _) = run_repl(":quit\n");
    assert!(
        stdout.contains("Ruxen") && stdout.contains("REPL"),
        "banner missing: {:?}",
        stdout,
    );
    assert!(
        stdout.contains("Goodbye"),
        "quit message missing: {:?}",
        stdout
    );
}

#[test]
fn help_command() {
    let (stdout, _) = run_repl(":help\n:quit\n");
    // :help should list the core commands.
    assert!(stdout.contains(":help"));
    assert!(stdout.contains(":quit"));
    assert!(stdout.contains(":reset"));
    assert!(stdout.contains(":type"));
}

#[test]
fn type_command_on_int() {
    let (stdout, _) = run_repl(":type 1 + 2\n:quit\n");
    assert!(
        stdout.contains("Int"),
        ":type should report Int. got:\n{}",
        stdout,
    );
}

#[test]
fn type_command_on_string_literal() {
    let (stdout, _) = run_repl(":type \"hello\"\n:quit\n");
    assert!(
        stdout.contains("&str") || stdout.contains("String"),
        ":type on string failed. got:\n{}",
        stdout,
    );
}

#[test]
fn type_command_on_float() {
    let (stdout, _) = run_repl(":type 1.0 + 2.0\n:quit\n");
    assert!(
        stdout.contains("Float"),
        ":type on float failed. got:\n{}",
        stdout,
    );
}

#[test]
fn unknown_command_is_handled_gracefully() {
    let (stdout, _) = run_repl(":nonsense\n:quit\n");
    // Should not crash — an error line + normal exit.
    assert!(stdout.contains("Goodbye"), "REPL didn't quit cleanly");
}
