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
use std::io::Write;
use std::process::{Command, Stdio};

fn rvn(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/riven")
        .join(format!("{name}.rvn"));
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

/// Phase 2 stdlib (#06.2): compile + run a Riven program with the
/// supplied bytes piped to its stdin. Used by `Stdin.lines()` /
/// `Stdin.read_*` integration tests where the binary needs a known
/// payload on fd 0.
fn compile_and_run_with_stdin(
    source: &str,
    basename: &str,
    stdin_bytes: &[u8],
) -> (String, String, bool) {
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
    let source = rvn("stdout_println_emits_text_plus_newline");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_io_println");
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
    let source = rvn("stdout_print_emits_text_without_newline");
    let (stdout, _stderr, ok) = compile_and_run(&source, "stdlib_io_print");
    assert!(ok);
    // The closing `println("")` flushes a newline so the test does not
    // depend on stdout buffering across stream close.
    assert_eq!(stdout, "alphabeta\n", "got: {:?}", stdout);
}

/// `Stderr.eprintln(s)` writes to stderr, leaving stdout empty.
/// Pins the stream-routing contract.
#[test]
fn stderr_eprintln_routes_to_stderr_only() {
    let source = rvn("stderr_eprintln_routes_to_stderr_only");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_io_eprintln");
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
    let source = rvn("stderr_eprint_no_newline");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_io_eprint");
    assert!(ok);
    assert!(stdout.is_empty(), "got stdout: {:?}", stdout);
    assert_eq!(stderr, "abc\n", "got stderr: {:?}", stderr);
}

/// Phase 2 stdlib (#06.2): `Stdout.write_str(&str) -> Result[(), IoError]`
/// emits exactly the supplied bytes — no trailing newline. The Result
/// is Ok when the write succeeds.
///
/// Riven match arms are single-expression and use `->` (not `=>`).
/// Match-as-expression (test 121) means we can bind the tag inline
/// without a helper fn — `Result[Unit, IoError]` isn't expressible at
/// the source-syntax level (`Unit` is a typeck-internal type, not a
/// reserved type name).
#[test]
fn stdout_write_str_emits_exact_bytes() {
    let source = rvn("stdout_write_str_emits_exact_bytes");
    let (stdout, _stderr, ok) = compile_and_run(&source, "stdlib_io_write_str");
    assert!(ok);
    // First token is the literal write, second is the match-arm marker.
    assert_eq!(stdout, "hellook", "got: {:?}", stdout);
}

/// `Stdout.flush() -> Result[(), IoError]` returns Ok on a healthy stream.
#[test]
fn stdout_flush_returns_ok() {
    let source = rvn("stdout_flush_returns_ok");
    let (stdout, _stderr, ok) = compile_and_run(&source, "stdlib_io_flush");
    assert!(ok);
    assert_eq!(stdout, "before flushed\n", "got: {:?}", stdout);
}

/// `Stderr.write_str(&str)` routes to stderr and leaves stdout empty.
#[test]
fn stderr_write_str_routes_to_stderr() {
    let source = rvn("stderr_write_str_routes_to_stderr");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_io_stderr_write");
    assert!(ok);
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
/// Riven `for` syntax has no `do` keyword (see fixture `26_array_basic.rvn`).
#[test]
fn stdin_lines_yields_each_line() {
    let source = rvn("stdin_lines_yields_each_line");
    let (stdout, _stderr, ok) =
        compile_and_run_with_stdin(&source, "stdlib_io_lines_basic", b"alpha\nbeta\ngamma\n");
    assert!(ok);
    assert_eq!(stdout, "alpha\nbeta\ngamma\n", "got: {:?}", stdout);
}

/// `Stdin.lines()` does NOT emit an empty trailing element when the
/// input ends with '\n' (matches Rust's BufRead::lines). A final
/// partial line with no trailing newline IS emitted. We assert the
/// full transcript plus the count via interpolation on `.len`
/// (`.to_string()` on USize is typeck-only — no runtime symbol yet).
#[test]
fn stdin_lines_no_trailing_empty_and_partial_final_line() {
    let source = rvn("stdin_lines_no_trailing_empty_and_partial_final_line");
    // Input has no trailing newline — final partial line "z" should
    // still be emitted, giving 3 elements total.
    let (stdout, _stderr, ok) =
        compile_and_run_with_stdin(&source, "stdlib_io_lines_partial", b"x\ny\nz");
    assert!(ok);
    assert_eq!(stdout, "[x]\n[y]\n[z]\n3\n", "got: {:?}", stdout);
}

/// Empty stdin → empty Vec from `Stdin.lines()`.
#[test]
fn stdin_lines_empty_input_yields_empty_vec() {
    let source = rvn("stdin_lines_empty_input_yields_empty_vec");
    let (stdout, _stderr, ok) = compile_and_run_with_stdin(&source, "stdlib_io_lines_empty", b"");
    assert!(ok);
    assert_eq!(stdout, "0\n", "got: {:?}", stdout);
}

/// Phase 2 #06.5: with `IoError` promoted to a tagged enum, user code
/// can now construct variants directly. `.message()` dispatches to
/// the runtime helper that knows how to render each tag.
#[test]
fn io_error_variants_are_constructible_and_message_dispatches() {
    let source = rvn("io_error_variants_are_constructible_and_message_dispatches");
    let (stdout, _stderr, ok) = compile_and_run(&source, "stdlib_io_err_construct");
    assert!(ok);
    assert_eq!(
        stdout, "entity not found\npermission denied\nboom\n",
        "got: {:?}",
        stdout
    );
}

/// Phase 2 #06.5: `IoError` is now a tagged enum. Reading from an
/// empty stdin yields `Err(IoError.UnexpectedEof)`; calling
/// `.message()` on the captured error dispatches through the new
/// runtime helper (`riven_io_error_get_message`) and returns the
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
    let source = rvn("io_error_message_dispatches_per_variant_on_empty_stdin");
    let (stdout, _stderr, ok) = compile_and_run_with_stdin(&source, "stdlib_io_err_message", b"");
    assert!(ok);
    assert_eq!(
        stdout, "unexpected end of file\n",
        "expected UnexpectedEof variant message, got: {:?}",
        stdout
    );
}

/// Phase 2 #06.5 T1: the 11 message-carrying variants added on top of
/// the original 9 are constructible from user code with named-field
/// syntax — `IoError.ConnectionRefused(message: "...")` — and
/// `.message()` returns the captured payload (not the default static
/// description). One Riven program exercises all 11 in a single
/// binary to keep cargo-test wall time low.
#[test]
fn io_error_new_variants_construct_and_message_passes_through() {
    let source = rvn("io_error_new_variants_construct_and_message");
    let (stdout, _stderr, ok) =
        compile_and_run(&source, "stdlib_io_err_new_variants");
    assert!(ok, "binary failed; stdout=[{stdout}]");
    let expected = "no listener\nrst\nabort\nno sock\nin use\nunavail\n\
                    bad bytes\ntimeout\nwrote 0\nnope\nno mem\n";
    assert_eq!(stdout, expected, "got: {:?}", stdout);
}

/// Phase 2 #06.5 T1: `IoError.kind() -> IoErrorKind` returns the
/// discriminant as a sibling enum whose tag values match `IoError`
/// 1:1. Constructing several variants and matching on their
/// `.kind()` exercises both the runtime dispatcher
/// (`riven_io_error_kind`) and the typeck mapping
/// `Ty::Enum("IoError") :: "kind"` → `Ty::Enum("IoErrorKind")`.
#[test]
fn io_error_kind_round_trip_returns_matching_io_error_kind_variant() {
    let source = rvn("io_error_kind_round_trip");
    let (stdout, _stderr, ok) =
        compile_and_run(&source, "stdlib_io_err_kind_round_trip");
    assert!(ok, "binary failed; stdout=[{stdout}]");
    let expected = "kind:NotFound\nkind:PermissionDenied\n\
                    kind:ConnectionRefused\nkind:TimedOut\nkind:OutOfMemory\n";
    assert_eq!(stdout, expected, "got: {:?}", stdout);
}

/// Phase 2 #06.5 T1: the compiler must accept a 20-arm match on
/// `IoError` as exhaustive (no wildcard, no catch-all). This pins
/// the resolve-side variant list and the typeck exhaustiveness
/// checker against the runtime constant table.
#[test]
fn io_error_exhaustive_20_variant_match_compiles_and_runs() {
    let source = rvn("io_error_exhaustive_match_all_20_variants");
    let (stdout, _stderr, ok) =
        compile_and_run(&source, "stdlib_io_err_exhaustive");
    assert!(ok, "binary failed; stdout=[{stdout}]");
    assert_eq!(stdout, "nf\not\ncr\nom\n", "got: {:?}", stdout);
}
