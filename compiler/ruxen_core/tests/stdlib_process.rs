//! Integration test for `std::process`.
//!
//! Covers `process.exit(code)` and the full `Command` builder pipeline
//! (`Command.new(cmd).arg(_).args(_).env(_,_).current_dir(_).{status,
//! output}`). The flat `process_run(cmd, args) -> Int` free-fn that
//! shipped in earlier previews was removed in #06.5 T5.5 once
//! `Command.{status, output}` covered every use case — its pin tests
//! moved to the `command_*` family below.

use ruxen_core::codegen;
use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;
use std::process::Command;

fn rx(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ruxen")
        .join(format!("{name}.rx"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn workspace_root() -> std::path::PathBuf {
    let crate_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

fn compile_and_run(source: &str, basename: &str) -> (String, String, bool) {
    let root = workspace_root();
    let tmp_dir = root.join("tmp");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let bin_path = tmp_dir.join(format!(
        "{}-{}-{}.bin",
        basename,
        std::process::id(),
        ruxen_unique_id()
    ));

    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "typecheck errors: {:?}", errors);

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .expect("MIR lowering");
    codegen::compile(&mir, bin_path.to_str().unwrap()).expect("codegen");

    let output = Command::new(&bin_path).output().expect("run binary");
    let _ = std::fs::remove_file(&bin_path);
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.success(),
    )
}

// ─── process spec B1 direct exit-code pins (gap fill 2026-05) ──────────
//
// `std_use_resolution.rs::std_process_exit_round_trip` already pins
// `exit(23)`.  Below we cover the spec-named common codes (0, 1, 2, 42)
// in a single test that records every exit and asserts on each.

fn compile_and_get_exit_code(source: &str, basename: &str) -> i32 {
    let root = workspace_root();
    let tmp_dir = root.join("tmp");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let bin_path = tmp_dir.join(format!(
        "{}-{}-{}.bin",
        basename,
        std::process::id(),
        ruxen_unique_id()
    ));

    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "typecheck errors: {:?}", errors);

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .expect("MIR lowering");
    codegen::compile(&mir, bin_path.to_str().unwrap()).expect("codegen");

    let output = Command::new(&bin_path).output().expect("run binary");
    let _ = std::fs::remove_file(&bin_path);
    output.status.code().expect("process should exit normally")
}

#[test]
fn process_exit_zero_returns_zero() {
    let source = rx("process_exit_zero_returns_zero");
    assert_eq!(
        compile_and_get_exit_code(&source, "stdlib_process_exit_0"),
        0
    );
}

#[test]
fn process_exit_one_returns_one() {
    let source = rx("process_exit_one_returns_one");
    assert_eq!(
        compile_and_get_exit_code(&source, "stdlib_process_exit_1"),
        1
    );
}

#[test]
fn process_exit_forty_two_returns_forty_two() {
    let source = rx("process_exit_forty_two_returns_forty_two");
    assert_eq!(
        compile_and_get_exit_code(&source, "stdlib_process_exit_42"),
        42
    );
}

// ─── std::process::Command builder API (Phase 2 #06 final gap) ─────────
//
// Mirrors the `fs.metadata` "flat heap struct + accessors" pattern
// (commit 0c62e97). Each test exercises the full builder pipeline
// from `Command.new` through one of the terminals (`.status` or
// `.output`) and asserts on the captured stdout. The fixtures all
// hand-roll the `match Result` branches because Result.unwrap is
// typeck-only — same pattern as `stdlib_fs.rs::fs_metadata_*`.

/// `/usr/bin/true.status -> Ok(ExitStatus(0))`.  Pins the happy-path
/// fork+waitpid path through the Command builder.
#[test]
fn command_status_true_returns_zero() {
    let source = rx("command_status_true_returns_zero");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_command_status_true");
    assert!(ok, "stdout=[{}] stderr=[{}]", stdout, stderr);
    assert!(
        stdout.contains("ok"),
        "expected exit code 0 from /usr/bin/true via Command, got: stdout=[{}] stderr=[{}]",
        stdout,
        stderr
    );
}

/// `/usr/bin/false.status -> Ok(ExitStatus(1))`.  Pins non-zero exit
/// propagation (would catch a sign-extension or WEXITSTATUS bug).
#[test]
fn command_status_false_returns_one() {
    let source = rx("command_status_false_returns_one");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_command_status_false");
    assert!(ok, "stdout=[{}] stderr=[{}]", stdout, stderr);
    assert!(
        stdout.contains("ok"),
        "expected exit code 1 from /usr/bin/false via Command, got: stdout=[{}] stderr=[{}]",
        stdout,
        stderr
    );
}

/// `.arg(...)` propagates a single argv slot.  `/bin/echo hello` exits 0
/// regardless of what we pass, so this just pins the typecheck +
/// codegen of the chained builder shape.
#[test]
fn command_arg_passes_through() {
    let source = rx("command_arg_passes_through");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_command_arg");
    assert!(ok, "stdout=[{}] stderr=[{}]", stdout, stderr);
    assert!(
        stdout.contains("ok"),
        "expected exit 0 from /bin/echo with arg, got: stdout=[{}] stderr=[{}]",
        stdout,
        stderr
    );
}

/// `.args(Array[String])` bulk-appends.  Three positional args to
/// /bin/echo — exit 0 confirms the Vec[String] -> argv plumbing
/// (each slot read via `args->data[i]` cast back to `char*`).
#[test]
fn command_args_bulk() {
    let source = rx("command_args_bulk");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_command_args_bulk");
    assert!(ok, "stdout=[{}] stderr=[{}]", stdout, stderr);
    assert!(
        stdout.contains("ok"),
        "expected exit 0 from /bin/echo with .args bulk, got: stdout=[{}] stderr=[{}]",
        stdout,
        stderr
    );
}

/// `.env(K, V)` adds an env var that the child can read.  Use
/// `/usr/bin/env` (prints `KEY=VAL` lines for every env entry) and
/// capture stdout — the `RUXEN_TEST=1` line must appear.  Pins envp
/// construction in `ruxen_command_build_envp`.
#[test]
fn command_env_visible_to_child() {
    let source = rx("command_env_visible_to_child");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_command_env");
    assert!(ok, "stdout=[{}] stderr=[{}]", stdout, stderr);
    assert!(
        stdout.contains("env_ok"),
        "expected RUXEN_TEST=1 in captured stdout, got: stdout=[{}] stderr=[{}]",
        stdout,
        stderr
    );
}

/// `.current_dir(path)` changes the child's cwd.  Use `/bin/pwd`,
/// capture stdout, assert it contains "/tmp" (macOS may resolve to
/// "/private/tmp" — contains catches both).  Pins the chdir branch
/// in the child.
#[test]
fn command_current_dir_changes_cwd() {
    let source = rx("command_current_dir_changes_cwd");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_command_cwd");
    assert!(ok, "stdout=[{}] stderr=[{}]", stdout, stderr);
    assert!(
        stdout.contains("cwd_ok"),
        "expected pwd output to contain /tmp, got: stdout=[{}] stderr=[{}]",
        stdout,
        stderr
    );
}

/// `.output()` captures the child's stdout via a pipe.  `/bin/echo xyz`
/// → captured stdout contains "xyz".  Pins pipe + dup2 + drain in
/// `ruxen_command_output`.
#[test]
fn command_output_captures_stdout() {
    let source = rx("command_output_captures_stdout");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_command_output_stdout");
    assert!(ok, "stdout=[{}] stderr=[{}]", stdout, stderr);
    assert!(
        stdout.contains("stdout_ok"),
        "expected captured stdout to contain xyz, got: stdout=[{}] stderr=[{}]",
        stdout,
        stderr
    );
}

/// `.output()` captures stderr separately.  `/bin/sh -c "echo err 1>&2"`
/// writes to stderr only — captured stderr must contain "err".
#[test]
fn command_output_captures_stderr() {
    let source = rx("command_output_captures_stderr");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_command_output_stderr");
    assert!(ok, "stdout=[{}] stderr=[{}]", stdout, stderr);
    assert!(
        stdout.contains("stderr_ok"),
        "expected captured stderr to contain err, got: stdout=[{}] stderr=[{}]",
        stdout,
        stderr
    );
}

/// `Command.new("/no/such/path").status` returns Result::Err.  Pins
/// the pre-flight `access(F_OK)` check that turns a typo'd binary
/// into a structured error instead of an exec-failure exit code of
/// 127 indistinguishable from a child that legitimately exited 127.
#[test]
fn command_nonexistent_binary_returns_err() {
    let source = rx("command_nonexistent_binary_returns_err");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_command_missing");
    assert!(ok, "stdout=[{}] stderr=[{}]", stdout, stderr);
    assert!(
        stdout.contains("err_ok"),
        "expected Err on missing binary, got: stdout=[{}] stderr=[{}]",
        stdout,
        stderr
    );
}

fn ruxen_unique_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}
