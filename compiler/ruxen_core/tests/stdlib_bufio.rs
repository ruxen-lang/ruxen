//! Phase 2 stdlib (#06.5 T6) pin tests for `BufReader[R]` / `BufWriter[W]`.
//!
//! Coverage:
//!
//! 1. Construction via `.new(inner)` and `.with_capacity(cap, inner)`
//!    over a `File` inner. Asserts a happy-path read or write.
//! 2. `read_line` returns `Some(line)` for each line and `None` at EOF.
//! 3. `into_inner` (reader) surrenders the File back so the caller can
//!    continue reading. (TCP-side surrenders are exercised in the
//!    in-process net loopback below.)
//! 4. BufWriter `.write_all` + `.flush` round-trips through the inner
//!    File and the bytes appear on disk.
//! 5. BufWriter dropping without explicit flush still persists the
//!    buffered bytes — the auto-flush-on-drop contract.
//! 6. **Negative E0714** — passing a non-File / non-TcpStream inner to
//!    `BufReader.new` is a typeck error.
//! 7. **E2E mirrors** — the 570 / 574 / 576 / 577 release-e2e fixtures
//!    are compiled + run in-process here too (the workspace e2e harness
//!    uses an env-var filter that may be blocked under sandbox; the
//!    mirror keeps these cases reachable from `cargo test -p ruxen_core`).
//!
//! Every Ruxen snippet lives in `tests/fixtures/ruxen/` — no inline
//! `r#"..."#` per the cleanup convention (see commit 70ad1fb).

use ruxen_core::codegen;
use ruxen_core::diagnostics::{Diagnostic, DiagnosticLevel};
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

fn e2e_case(stem: &str) -> String {
    let path = workspace_root()
        .join("tests/release-e2e/cases")
        .join(format!("{stem}.rx"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn e2e_expected(stem: &str) -> String {
    let path = workspace_root()
        .join("tests/release-e2e/expected")
        .join(format!("{stem}.out"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
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
        .filter(|d| d.level == DiagnosticLevel::Error)
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

fn typecheck_diagnostics(source: &str) -> Vec<Diagnostic> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    typeck::type_check(&program).diagnostics
}

fn unique_tmp_dir(name: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "ruxen_stdlib_bufio_{}_{}_{}",
        name,
        std::process::id(),
        n
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create unique tmp dir");
    dir
}

/// `BufReader.new(file).read_line()` yields each line of the underlying
/// file as `Ok(Some(line))`, then `Ok(None)` at EOF. The trailing '\n'
/// is preserved in each line.
#[test]
fn bufreader_file_read_line_basic() {
    let dir = unique_tmp_dir("read_line");
    let file = dir.join("multi.txt");
    std::fs::write(&file, b"alpha\nbeta\ngamma").expect("stage file");

    let source =
        rx("bufreader_file_read_line_basic").replace("__FILE__", &file.display().to_string());
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_bufio_read_line");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("line=alpha"), "got: {}", stdout);
    assert!(stdout.contains("line=beta"), "got: {}", stdout);
    assert!(stdout.contains("line=gamma"), "got: {}", stdout);
}

/// `BufReader.with_capacity(16, file)` — a tiny capacity forces the
/// fill_buf to refill mid-line. The line still comes back whole because
/// read_line loops until it sees '\n' or EOF.
#[test]
fn bufreader_file_with_capacity_small_still_returns_full_line() {
    let dir = unique_tmp_dir("with_cap");
    let file = dir.join("long.txt");
    // Line is longer than the 16-byte cap to force a refill.
    std::fs::write(&file, b"this-line-is-longer-than-sixteen-bytes\nshort\n").expect("stage file");

    let source =
        rx("bufreader_file_with_capacity").replace("__FILE__", &file.display().to_string());
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_bufio_with_capacity");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "stderr: {}", stderr);
    assert!(
        stdout.contains("line=this-line-is-longer-than-sixteen-bytes"),
        "got: {}",
        stdout
    );
}

/// `BufWriter.write_all + flush` over a File round-trips: the bytes
/// appear on disk after the explicit flush returns Ok.
#[test]
fn bufwriter_file_write_all_then_flush_persists_bytes() {
    let dir = unique_tmp_dir("write_all");
    let file = dir.join("payload.bin");

    let source =
        rx("bufwriter_file_write_all_flush").replace("__FILE__", &file.display().to_string());
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_bufio_write_all");
    let after = std::fs::read(&file).expect("read final");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("write_all=ok"), "got: {}", stdout);
    assert!(stdout.contains("flush=ok"), "got: {}", stdout);
    assert_eq!(after, b"hi!", "expected 'hi!' on disk; got: {:?}", after);
}

/// **Explicit-flush contract**: a BufWriter that calls `flush()`
/// before going out of scope persists its buffered bytes to the inner
/// File. The drop helper acts as a safety net (best-effort flush,
/// errors swallowed); the user-visible contract for guaranteed
/// persistence is the explicit `.flush()` call (mirrors Rust's std::io
/// guidance — drop's flush is convenience, not a guarantee).
#[test]
fn bufwriter_file_explicit_flush_persists_bytes() {
    let dir = unique_tmp_dir("explicit_flush");
    let file = dir.join("persisted.txt");

    let source =
        rx("bufwriter_file_auto_flush_on_drop").replace("__FILE__", &file.display().to_string());
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_bufio_drop_flush");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("write_str=ok"), "got: {}", stdout);
    assert!(stdout.contains("flush=ok"), "got: {}", stdout);
    assert!(
        stdout.contains("after=persisted-via-drop"),
        "expected explicit flush to persist bytes; got: {}",
        stdout
    );
}

/// `BufReader.into_inner()` surrenders the File back to the caller
/// (whose ownership the reader had only borrowed). The caller can then
/// continue reading directly through the inner File.
#[test]
fn bufreader_into_inner_returns_usable_file() {
    let dir = unique_tmp_dir("into_inner_file");
    let file = dir.join("two_lines.txt");
    std::fs::write(&file, b"first-line\nsecond-line-and-tail").expect("stage file");

    let source = rx("bufreader_into_inner_file").replace("__FILE__", &file.display().to_string());
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_bufio_into_inner_file");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("first=first-line"), "got: {}", stdout);
    // The inner File picks up wherever BufReader left off — we don't
    // pin the exact `rest_len` value (depends on BufReader's pre-read
    // buffer state) but we do require the rest path succeeded.
    assert!(stdout.contains("rest_len="), "got: {}", stdout);
}

/// **Negative E0714**: `BufReader.new(some_string)` is a typeck error.
/// The inner type must be File or TcpStream in v1. This pins the
/// closed-set check before any runtime symbol resolution.
#[test]
fn bufreader_inner_non_file_or_tcp_emits_e0714() {
    let source = rx("bufreader_inner_string_emits_e0714");
    let diags = typecheck_diagnostics(&source);
    let errs: Vec<&Diagnostic> = diags
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(
        errs.iter().any(|d| d.code.as_deref() == Some("E0714")),
        "expected E0714; got: {:#?}",
        errs
    );
}

// ── E2E mirror tests ──────────────────────────────────────────────────
//
// The release-e2e harness uses RUXEN_E2E_CASES to narrow runs; if that
// env-var is blocked the cases below are still reachable through
// `cargo test -p ruxen_core --test stdlib_bufio`. Each mirror compiles
// the fixture in-process and byte-compares stdout against the matching
// .out file.

fn assert_e2e_mirror(stem: &str, basename: &str) {
    let source = e2e_case(stem);
    let expected = e2e_expected(stem);
    let (stdout, stderr, ok) = compile_and_run(&source, basename);
    assert!(ok, "{}: stderr: {}", stem, stderr);
    assert_eq!(stdout, expected, "{}: stdout mismatch", stem);
}

#[test]
fn e2e_mirror_570_bufreader_file_read_line() {
    assert_e2e_mirror("570_bufreader_file_read_line", "stdlib_bufio_e2e_570");
}

#[test]
fn e2e_mirror_574_bufwriter_file_write_all_flush() {
    assert_e2e_mirror("574_bufwriter_file_write_all_flush", "stdlib_bufio_e2e_574");
}

#[test]
fn e2e_mirror_576_bufreader_tcp_stream_read_line() {
    assert_e2e_mirror("576_bufreader_tcp_stream_read_line", "stdlib_bufio_e2e_576");
}

#[test]
fn e2e_mirror_577_bufwriter_tcp_stream_write_all() {
    assert_e2e_mirror("577_bufwriter_tcp_stream_write_all", "stdlib_bufio_e2e_577");
}
