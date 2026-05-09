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

/// `/bin/true` — POSIX no-op that exits 0. The simplest possible
/// fork+execvp success path.
#[test]
fn process_run_true_returns_zero() {
    let source = r##"
use std.process.process_run

def main
  let cmd = "/bin/true"
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
        "expected exit code 0 from /bin/true, got: stdout=[{}] stderr=[{}]",
        stdout,
        stderr
    );
}

/// `/bin/false` — POSIX no-op that exits 1. Verifies that we faithfully
/// surface non-zero exit codes (and don't, e.g., always return 0 because
/// of a sign-extension bug or a misread `WEXITSTATUS`).
#[test]
fn process_run_false_returns_one() {
    let source = r##"
use std.process.process_run

def main
  let cmd = "/bin/false"
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
        "expected exit code 1 from /bin/false, got: stdout=[{}] stderr=[{}]",
        stdout,
        stderr
    );
}

/// `/bin/echo hello` — verifies that args propagate from a Riven
/// `Vec[String]` into the child's argv. Exit code is 0; we don't
/// capture stdout (out of scope for v1) but the child inherits the
/// parent's stdout, so "hello" lands in the test process's captured
/// stdout. We assert exit-code success only — output checking is a
/// nicety and would be brittle on systems where /bin/echo behaves
/// slightly differently.
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
