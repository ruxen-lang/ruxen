//! Integration tests for Phase 2 stdlib (#06.1) `std::io` print
//! convenience methods on Stdout / Stderr.
//!
//! `print` / `println` (Stdout) and `eprint` / `eprintln` (Stderr) are
//! the no-Result variants of `write_str`. Failures are silently
//! swallowed (a v1 simplification of Rust's panic-on-broken-pipe
//! behaviour). The trailing-newline variants emit a literal `\n`.
//!
//! We compile a tiny Ruxen program, run it, and assert on stdout /
//! stderr separately so we can pin the per-stream output.

use crate::common;
use std::io::Write;
use std::process::{Command, Stdio};

fn rx(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ruxen")
        .join(format!("{name}.rx"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

/// Phase 2 stdlib (#06.2): compile + run a Ruxen program with the
/// supplied bytes piped to its stdin. Used by `Stdin.lines()` /
/// `Stdin.read_*` integration tests where the binary needs a known
/// payload on fd 0.
fn compile_and_run_with_stdin(
    source: &str,
    basename: &str,
    stdin_bytes: &[u8],
) -> (String, String, Option<i32>) {
    let bin_path = common::compile_program(source, basename);
    let mut child = Command::new(&bin_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn binary");

    {
        let stdin = child.stdin.as_mut().expect("child stdin");
        stdin.write_all(stdin_bytes).expect("write stdin");
    }

    let output = child.wait_with_output().expect("wait child");
    let _ = std::fs::remove_file(&bin_path);
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.code(),
    )
}

/// `Stdout.println(s)` writes `s` followed by a newline to stdout.
/// Two consecutive `println` calls produce two lines.
#[test]
fn stdout_println_emits_text_plus_newline() {
    let source = rx("stdout_println_emits_text_plus_newline");
    let (stdout, stderr, code) = common::compile_and_run(&source, "stdlib_io_println");
    assert_eq!(
        code,
        Some(0),
        "binary failed. stdout=[{stdout}] stderr=[{stderr}]"
    );
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
    let source = rx("stdout_print_emits_text_without_newline");
    let (stdout, _stderr, code) = common::compile_and_run(&source, "stdlib_io_print");
    assert_eq!(code, Some(0));
    // The closing `println("")` flushes a newline so the test does not
    // depend on stdout buffering across stream close.
    assert_eq!(stdout, "alphabeta\n", "got: {:?}", stdout);
}

/// `Stderr.eprintln(s)` writes to stderr, leaving stdout empty.
/// Pins the stream-routing contract.
#[test]
fn stderr_eprintln_routes_to_stderr_only() {
    let source = rx("stderr_eprintln_routes_to_stderr_only");
    let (stdout, stderr, code) = common::compile_and_run(&source, "stdlib_io_eprintln");
    assert_eq!(code, Some(0));
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
    let source = rx("stderr_eprint_no_newline");
    let (stdout, stderr, code) = common::compile_and_run(&source, "stdlib_io_eprint");
    assert_eq!(code, Some(0));
    assert!(stdout.is_empty(), "got stdout: {:?}", stdout);
    assert_eq!(stderr, "abc\n", "got stderr: {:?}", stderr);
}

/// Phase 2 stdlib (#06.2): `Stdout.write_str(&str) -> Result[(), IoError]`
/// emits exactly the supplied bytes — no trailing newline. The Result
/// is Ok when the write succeeds.
///
/// Ruxen match arms are single-expression and use `->` (not `=>`).
/// Match-as-expression (test 121) means we can bind the tag inline
/// without a helper fn — `Result[Unit, IoError]` isn't expressible at
/// the source-syntax level (`Unit` is a typeck-internal type, not a
/// reserved type name).
#[test]
fn stdout_write_str_emits_exact_bytes() {
    let source = rx("stdout_write_str_emits_exact_bytes");
    let (stdout, _stderr, code) = common::compile_and_run(&source, "stdlib_io_write_str");
    assert_eq!(code, Some(0));
    // First token is the literal write, second is the match-arm marker.
    assert_eq!(stdout, "hellook", "got: {:?}", stdout);
}

/// `Stdout.flush() -> Result[(), IoError]` returns Ok on a healthy stream.
#[test]
fn stdout_flush_returns_ok() {
    let source = rx("stdout_flush_returns_ok");
    let (stdout, _stderr, code) = common::compile_and_run(&source, "stdlib_io_flush");
    assert_eq!(code, Some(0));
    assert_eq!(stdout, "before flushed\n", "got: {:?}", stdout);
}

/// `Stderr.write_str(&str)` routes to stderr and leaves stdout empty.
#[test]
fn stderr_write_str_routes_to_stderr() {
    let source = rx("stderr_write_str_routes_to_stderr");
    let (stdout, stderr, code) = common::compile_and_run(&source, "stdlib_io_stderr_write");
    assert_eq!(code, Some(0));
    assert!(
        stdout.is_empty(),
        "stdout should be empty, got: {:?}",
        stdout
    );
    assert_eq!(stderr, "oops!", "got stderr: {:?}", stderr);
}

/// Phase 2 stdlib (#06.2): `Stdin.lines() -> Vec[Result[String, IoError]]`.
/// v1 simplification of Rust's `BufRead::lines` — reads to EOF up
/// front, splits on '\n', materialises a Vec. Newlines stripped from
/// each line; trailing '\n' does NOT produce a final empty element.
///
/// Helper `unwrap_line` reduces each match arm to a single expression.
/// Ruxen `for` syntax has no `do` keyword (see fixture `26_array_basic.rx`).
#[test]
fn stdin_lines_yields_each_line() {
    let source = rx("stdin_lines_yields_each_line");
    let (stdout, _stderr, code) =
        compile_and_run_with_stdin(&source, "stdlib_io_lines_basic", b"alpha\nbeta\ngamma\n");
    assert_eq!(code, Some(0));
    assert_eq!(stdout, "alpha\nbeta\ngamma\n", "got: {:?}", stdout);
}

/// `Stdin.lines()` does NOT emit an empty trailing element when the
/// input ends with '\n' (matches Rust's BufRead::lines). A final
/// partial line with no trailing newline IS emitted. We assert the
/// full transcript plus the count via interpolation on `.len`
/// (`.to_string()` on USize is typeck-only — no runtime symbol yet).
#[test]
fn stdin_lines_no_trailing_empty_and_partial_final_line() {
    let source = rx("stdin_lines_no_trailing_empty_and_partial_final_line");
    // Input has no trailing newline — final partial line "z" should
    // still be emitted, giving 3 elements total.
    let (stdout, _stderr, code) =
        compile_and_run_with_stdin(&source, "stdlib_io_lines_partial", b"x\ny\nz");
    assert_eq!(code, Some(0));
    assert_eq!(stdout, "[x]\n[y]\n[z]\n3\n", "got: {:?}", stdout);
}

/// Empty stdin → empty Vec from `Stdin.lines()`.
#[test]
fn stdin_lines_empty_input_yields_empty_vec() {
    let source = rx("stdin_lines_empty_input_yields_empty_vec");
    let (stdout, _stderr, code) = compile_and_run_with_stdin(&source, "stdlib_io_lines_empty", b"");
    assert_eq!(code, Some(0));
    assert_eq!(stdout, "0\n", "got: {:?}", stdout);
}

/// Phase 2 #06.5: `IoError` is now a tagged enum. Reading from an
/// empty stdin yields `Err(IoError.UnexpectedEof)`; calling
/// `.message()` on the captured error dispatches through the new
/// runtime helper (`ruxen_io_error_get_message`) and returns the
/// per-variant description string. The explicit `e: IoError` type
/// annotation pins inference so the method-call callee is mangled
/// as `IoError_message` (which `codegen/runtime.rs` maps to the
/// real dispatcher). Without the annotation, the bound pattern
/// variable's type stays an unresolved inference variable and the
/// callee gets mangled with the type-var name instead of `IoError`
/// — a pre-existing inference gap unrelated to the C2/IoError
/// refactor.
#[test]
fn io_error_message_dispatches_per_variant_on_empty_stdin() {
    let source = rx("io_error_message_dispatches_per_variant_on_empty_stdin");
    let (stdout, _stderr, code) = compile_and_run_with_stdin(&source, "stdlib_io_err_message", b"");
    assert_eq!(code, Some(0));
    assert_eq!(
        stdout, "unexpected end of file\n",
        "expected UnexpectedEof variant message, got: {:?}",
        stdout
    );
}
