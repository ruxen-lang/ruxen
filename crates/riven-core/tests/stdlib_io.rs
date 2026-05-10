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

/// Phase 2 stdlib (#06.2): `Stdout.write_str(&str) -> Result[(), IoError]`
/// emits exactly the supplied bytes — no trailing newline. The Result
/// is Ok when the write succeeds.
///
/// Riven match arms are single-expression and use `->` (not `=>`) — see
/// the `or_empty` helper-fn workaround in stdlib_fs.rs for the v1
/// idiom. Here we wrap `write_str` in a `did_ok` reducer that returns
/// a String tag the caller can print.
#[test]
fn stdout_write_str_emits_exact_bytes() {
    let source = r##"
def did_ok(r: Result[Unit, IoError]) -> String
  match r
    Ok(_)  -> String.from("ok")
    Err(_) -> String.from("err")
  end
end

def main
  let out = stdout()
  let tag = did_ok(out.write_str("hello"))
  out.print(tag)
end
"##;
    let (stdout, _stderr, ok) = compile_and_run(source, "stdlib_io_write_str");
    assert!(ok);
    // First token is the literal write, second is the match-arm marker.
    assert_eq!(stdout, "hellook", "got: {:?}", stdout);
}

/// `Stdout.flush() -> Result[(), IoError]` returns Ok on a healthy stream.
#[test]
fn stdout_flush_returns_ok() {
    let source = r##"
def did_ok(r: Result[Unit, IoError]) -> String
  match r
    Ok(_)  -> String.from(" flushed")
    Err(_) -> String.from(" failed")
  end
end

def main
  let out = stdout()
  out.print("before")
  let tag = did_ok(out.flush())
  out.println(tag)
end
"##;
    let (stdout, _stderr, ok) = compile_and_run(source, "stdlib_io_flush");
    assert!(ok);
    assert_eq!(stdout, "before flushed\n", "got: {:?}", stdout);
}

/// `Stderr.write_str(&str)` routes to stderr and leaves stdout empty.
#[test]
fn stderr_write_str_routes_to_stderr() {
    let source = r##"
def did_ok(r: Result[Unit, IoError]) -> String
  match r
    Ok(_)  -> String.from("!")
    Err(_) -> String.from("?")
  end
end

def main
  let err = stderr()
  let tag = did_ok(err.write_str("oops"))
  err.eprint(tag)
end
"##;
    let (stdout, stderr, ok) = compile_and_run(source, "stdlib_io_stderr_write");
    assert!(ok);
    assert!(stdout.is_empty(), "stdout should be empty, got: {:?}", stdout);
    assert_eq!(stderr, "oops!", "got stderr: {:?}", stderr);
}

/// Phase 2 stdlib (#06.2): `Stdin.lines() -> Vec[Result[String, IoError]]`.
/// v1 simplification of Rust's `BufRead::lines` — reads to EOF up
/// front, splits on '\n', materialises a Vec. Newlines stripped from
/// each line; trailing '\n' does NOT produce a final empty element.
///
/// Helper `unwrap_line` reduces each match arm to a single expression.
#[test]
fn stdin_lines_yields_each_line() {
    let source = r##"
def unwrap_line(r: Result[String, IoError]) -> String
  match r
    Ok(line) -> line
    Err(_)   -> String.from("ERR")
  end
end

def main
  let stream = stdin()
  let lines = stream.lines()
  let out = stdout()
  for line_result in lines do
    out.println(unwrap_line(line_result))
  end
end
"##;
    let (stdout, _stderr, ok) =
        compile_and_run_with_stdin(source, "stdlib_io_lines_basic", b"alpha\nbeta\ngamma\n");
    assert!(ok);
    assert_eq!(stdout, "alpha\nbeta\ngamma\n", "got: {:?}", stdout);
}

/// `Stdin.lines()` does NOT emit an empty trailing element when the
/// input ends with '\n' (matches Rust's BufRead::lines). A final
/// partial line with no trailing newline IS emitted. We assert the
/// full transcript plus the count via `.length` on the Vec.
#[test]
fn stdin_lines_no_trailing_empty_and_partial_final_line() {
    let source = r##"
def unwrap_line(r: Result[String, IoError]) -> String
  match r
    Ok(line) -> line
    Err(_)   -> String.from("ERR")
  end
end

def main
  let stream = stdin()
  let lines = stream.lines()
  let out = stdout()
  for line_result in lines do
    out.print("[")
    out.print(unwrap_line(line_result))
    out.println("]")
  end
  out.println(lines.len.to_string())
end
"##;
    // Input has no trailing newline — final partial line "z" should
    // still be emitted, giving 3 elements total.
    let (stdout, _stderr, ok) =
        compile_and_run_with_stdin(source, "stdlib_io_lines_partial", b"x\ny\nz");
    assert!(ok);
    assert_eq!(stdout, "[x]\n[y]\n[z]\n3\n", "got: {:?}", stdout);
}

/// Empty stdin → empty Vec from `Stdin.lines()`.
#[test]
fn stdin_lines_empty_input_yields_empty_vec() {
    let source = r##"
def main
  let stream = stdin()
  let lines = stream.lines()
  let out = stdout()
  out.println(lines.len.to_string())
end
"##;
    let (stdout, _stderr, ok) =
        compile_and_run_with_stdin(source, "stdlib_io_lines_empty", b"");
    assert!(ok);
    assert_eq!(stdout, "0\n", "got: {:?}", stdout);
}
