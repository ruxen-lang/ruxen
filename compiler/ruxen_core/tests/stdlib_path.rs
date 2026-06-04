//! Integration test for Phase 3 `std::path` module.
//!
//! Verifies that `path_join`, `path_parent`, `path_file_name`,
//! `path_extension`, and `path_is_absolute` resolve through the
//! resolver, lower to the right runtime calls, and produce correct
//! values at runtime. Linux-style separators only — Windows backslash
//! is a non-goal for v1.

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

#[test]
fn path_module_basic_operations() {
    let source = rx("path_module_basic_operations");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_path_basic");
    assert!(
        ok,
        "binary exited non-zero. stdout=[{}] stderr=[{}]",
        stdout, stderr
    );
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.first().copied(), Some("/usr/local/bin/ruxen.rx"));
    assert_eq!(lines.get(1).copied(), Some("/usr/local/bin"));
    assert_eq!(lines.get(2).copied(), Some("ruxen.rx"));
    assert_eq!(lines.get(3).copied(), Some("rx"));
    assert_eq!(lines.get(4).copied(), Some("abs"));
}

#[test]
fn path_join_handles_absolute_second() {
    let source = rx("path_join_handles_absolute_second");
    let (stdout, _stderr, ok) = compile_and_run(&source, "stdlib_path_abs_override");
    assert!(ok, "stdout=[{}]", stdout);
    assert!(
        stdout.lines().next() == Some("/usr/bin"),
        "absolute second should override; got: [{}]",
        stdout
    );
}

#[test]
fn path_extension_empty_when_missing() {
    let source = rx("path_extension_empty_when_missing");
    let (stdout, _stderr, ok) = compile_and_run(&source, "stdlib_path_no_ext");
    assert!(ok, "stdout=[{}]", stdout);
    assert!(stdout.contains("ok"), "got: [{}]", stdout);
}

#[test]
fn path_is_absolute_detects_root() {
    let source = rx("path_is_absolute_detects_root");
    let (stdout, _stderr, ok) = compile_and_run(&source, "stdlib_path_is_abs");
    assert!(ok, "stdout=[{}]", stdout);
    assert!(stdout.contains("ok"), "got: [{}]", stdout);
}

fn ruxen_unique_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}
