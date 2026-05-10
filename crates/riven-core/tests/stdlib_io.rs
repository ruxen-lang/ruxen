//! Integration tests for Phase 2 stdlib (#06.1) `std::io` print
//! convenience methods on Stdout / Stderr.
//!
//! `print` / `println` (Stdout) and `eprint` / `eprintln` (Stderr) are
//! the no-Result variants of `write_str`. Failures are silently
//! swallowed (a v1 simplification of Rust's panic-on-broken-pipe
//! behaviour). The trailing-newline variants emit a literal `\n`.
//!
//! We compile a tiny Riven program, run it, and assert on stdout /
//! stderr separately so we can pin the per-stream output.

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

/// `Stdout.println(s)` writes `s` followed by a newline to stdout.
/// Two consecutive `println` calls produce two lines.
#[test]
fn stdout_println_emits_text_plus_newline() {
    let source = r##"
def main
  let out = stdout()
  out.println("first")
  out.println("second")
end
"##;
    let (stdout, stderr, ok) = compile_and_run(source, "stdlib_io_println");
    assert!(ok, "binary failed. stdout=[{stdout}] stderr=[{stderr}]");
    assert_eq!(
        stdout, "first\nsecond\n",
        "expected two newline-terminated lines on stdout, got: {:?}",
        stdout
    );
    assert!(
        stderr.is_empty(),
        "stderr should be empty, got: {:?}",
        stderr
    );
}

/// `Stdout.print(s)` writes `s` with NO trailing newline. Two
/// `print` calls concatenate.
#[test]
fn stdout_print_emits_text_without_newline() {
    let source = r##"
def main
  let out = stdout()
  out.print("alpha")
  out.print("beta")
  out.println("")
end
"##;
    let (stdout, _stderr, ok) = compile_and_run(source, "stdlib_io_print");
    assert!(ok);
    // The closing `println("")` flushes a newline so the test does not
    // depend on stdout buffering across stream close.
    assert_eq!(stdout, "alphabeta\n", "got: {:?}", stdout);
}

/// `Stderr.eprintln(s)` writes to stderr, leaving stdout empty.
/// Pins the stream-routing contract.
#[test]
fn stderr_eprintln_routes_to_stderr_only() {
    let source = r##"
def main
  let err = stderr()
  err.eprintln("warning")
end
"##;
    let (stdout, stderr, ok) = compile_and_run(source, "stdlib_io_eprintln");
    assert!(ok);
    assert!(
        stdout.is_empty(),
        "stdout should be empty, got: {:?}",
        stdout
    );
    assert_eq!(stderr, "warning\n", "got: {:?}", stderr);
}

/// `Stderr.eprint(s)` writes to stderr without a newline.
#[test]
fn stderr_eprint_no_newline() {
    let source = r##"
def main
  let err = stderr()
  err.eprint("a")
  err.eprint("b")
  err.eprintln("c")
end
"##;
    let (stdout, stderr, ok) = compile_and_run(source, "stdlib_io_eprint");
    assert!(ok);
    assert!(stdout.is_empty(), "got stdout: {:?}", stdout);
    assert_eq!(stderr, "abc\n", "got stderr: {:?}", stderr);
}
