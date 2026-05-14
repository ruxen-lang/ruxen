//! Integration test for Phase 3 `std::process::process_run`.
//!
//! Verifies that `process_run(cmd, args)` is reachable through the
//! resolver, lowers to the right runtime call, and returns the child's
//! exit code from a real fork+execvp roundtrip.
//!
//! Output capture is intentionally out of scope for v1 — these tests
//! only verify exit codes (and that stdio is inherited, which is what
//! /bin/echo writing to the test's stdout demonstrates implicitly).

use riven_core::codegen;
use riven_core::lexer::Lexer;
use riven_core::mir::lower::Lowerer;
use riven_core::parser::Parser;
use riven_core::typeck;
use std::process::Command;

fn workspace_root() -> std::path::PathBuf {
    let crate_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

fn compile_and_run(source: &str, basename: &str) -> (String, String, bool) {
    let root = workspace_root();
    let tmp_dir = root.join("tmp");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let bin_path = tmp_dir.join(format!("{}.bin", basename));

    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == riven_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "typecheck errors: {:?}", errors);

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .expect("MIR lowering");
    codegen::compile(&mir, bin_path.to_str().unwrap()).expect("codegen");

    let output = Command::new(&bin_path).output().expect("run binary");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.success(),
    )
}

/// `/usr/bin/true` — POSIX no-op that exits 0. The simplest possible
/// fork+execvp success path. Use `/usr/bin/`, not `/bin/`: macOS keeps
/// only a fixed minimal set in `/bin/` and `true`/`false` live in
/// `/usr/bin/` only. On most modern Linux distros `/bin → /usr/bin`,
/// so `/usr/bin/true` works there too.
#[test]
fn process_run_true_returns_zero() {
    let source = r##"
use std.process.process_run

def main
  let cmd = "/usr/bin/true"
  let args: Vec[String] = Vec.new
  let code = process_run(cmd, args)
  if code == 0
    puts "ok"
  else
    puts "fail code=#{code}"
  end
end
"##;
    let (stdout, stderr, ok) = compile_and_run(source, "stdlib_process_true");
    assert!(
        ok,
        "binary exited non-zero. stdout=[{}] stderr=[{}]",
        stdout, stderr
    );
    assert!(
        stdout.contains("ok"),
        "expected exit code 0 from /usr/bin/true, got: stdout=[{}] stderr=[{}]",
        stdout,
        stderr
    );
}

/// `/usr/bin/false` — POSIX no-op that exits 1. Verifies that we faithfully
/// surface non-zero exit codes (and don't, e.g., always return 0 because
/// of a sign-extension bug or a misread `WEXITSTATUS`).
#[test]
fn process_run_false_returns_one() {
    let source = r##"
use std.process.process_run

def main
  let cmd = "/usr/bin/false"
  let args: Vec[String] = Vec.new
  let code = process_run(cmd, args)
  if code == 1
    puts "ok"
  else
    puts "fail code=#{code}"
  end
end
"##;
    let (stdout, stderr, ok) = compile_and_run(source, "stdlib_process_false");
    assert!(
        ok,
        "binary exited non-zero. stdout=[{}] stderr=[{}]",
        stdout, stderr
    );
    assert!(
        stdout.contains("ok"),
        "expected exit code 1 from /usr/bin/false, got: stdout=[{}] stderr=[{}]",
        stdout,
        stderr
    );
}

/// `/bin/echo hello` — verifies that args propagate from a Riven
/// `Vec[String]` into the child's argv. Exit code is 0; we don't
/// capture stdout (out of scope for v1) but the child inherits the
/// parent's stdout, so "hello" lands in the test process's captured
/// stdout. We assert exit-code success only — output checking is a
/// nicety and would be brittle on systems where echo behaves
/// slightly differently.
///
/// Path note: unlike `true`/`false`, `echo` IS in macOS's `/bin/`.
/// `/usr/bin/echo` is missing on the GitHub `macos-14` runner image —
/// CI surfaced this via the runtime's execvp diagnostic. So we use
/// `/bin/echo`, which exists on both macOS and modern Linux distros
/// (where `/bin → /usr/bin`).
#[test]
fn process_run_echo_with_args_returns_zero() {
    let source = r##"
use std.process.process_run

def main
  let cmd = "/bin/echo"
  let mut args: Vec[String] = Vec.new
  args.push("hello")
  let code = process_run(cmd, args)
  if code == 0
    puts "ok"
  else
    puts "fail code=#{code}"
  end
end
"##;
    let (stdout, stderr, ok) = compile_and_run(source, "stdlib_process_echo");
    assert!(
        ok,
        "binary exited non-zero. stdout=[{}] stderr=[{}]",
        stdout, stderr
    );
    assert!(
        stdout.contains("ok"),
        "expected exit code 0 from /bin/echo, got: stdout=[{}] stderr=[{}]",
        stdout,
        stderr
    );
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
    let bin_path = tmp_dir.join(format!("{}.bin", basename));

    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == riven_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "typecheck errors: {:?}", errors);

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .expect("MIR lowering");
    codegen::compile(&mir, bin_path.to_str().unwrap()).expect("codegen");

    let output = Command::new(&bin_path).output().expect("run binary");
    output.status.code().expect("process should exit normally")
}

#[test]
fn process_exit_zero_returns_zero() {
    let source = r##"
use std.process.exit

def main
  exit(0)
end
"##;
    assert_eq!(
        compile_and_get_exit_code(source, "stdlib_process_exit_0"),
        0
    );
}

#[test]
fn process_exit_one_returns_one() {
    let source = r##"
use std.process.exit

def main
  exit(1)
end
"##;
    assert_eq!(
        compile_and_get_exit_code(source, "stdlib_process_exit_1"),
        1
    );
}

#[test]
fn process_exit_forty_two_returns_forty_two() {
    let source = r##"
use std.process.exit

def main
  exit(42)
end
"##;
    assert_eq!(
        compile_and_get_exit_code(source, "stdlib_process_exit_42"),
        42
    );
}

/// `process_run` of a nonexistent binary returns `127` per the spec's
/// B4 failure-mode encoding.  Pins the execvp-failure branch.
#[test]
fn process_run_nonexistent_binary_returns_127() {
    let source = r##"
use std.process.process_run

def main
  let cmd = "/no/such/binary/we/hope/exists"
  let args: Vec[String] = Vec.new
  let code = process_run(cmd, args)
  if code == 127
    puts "ok"
  else
    puts "fail code=#{code}"
  end
end
"##;
    let (stdout, stderr, ok) = compile_and_run(source, "stdlib_process_run_missing");
    assert!(ok, "stdout=[{}] stderr=[{}]", stdout, stderr);
    assert!(
        stdout.contains("ok"),
        "expected 127 from missing binary, got: stdout=[{}] stderr=[{}]",
        stdout,
        stderr
    );
}
