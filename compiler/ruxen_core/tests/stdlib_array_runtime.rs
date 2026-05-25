//! Pin tests for `docs/specs/stdlib/array.spec.md` gaps:
//! `Vec.first / last / contains / clone / reverse` — wired in the
//! runtime + codegen but not directly pinned.  These tests compile a
//! tiny Ruxen program for each, run it, and assert on the stdout so
//! the runtime contract is enforced.

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
    let bin_path = tmp_dir.join(format!("{}.bin", basename));

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
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.success(),
    )
}

/// `Vec.first()` returns `Option::Some(first)` on a non-empty vec,
/// `Option::None` on an empty vec.
#[test]
fn vec_first_returns_first_element() {
    let source = rx("vec_first_returns_first_element");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_vec_first");
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("first=10"), "first branch: {}", stdout);
    assert!(stdout.contains("empty_ok"), "empty branch: {}", stdout);
}

/// `Vec.last()` returns `Option::Some(last)` / `Option::None`.
#[test]
fn vec_last_returns_last_element() {
    let source = rx("vec_last_returns_last_element");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_vec_last");
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("last=30"), "got: {}", stdout);
}

/// `Vec.contains(&x)` returns Bool.
#[test]
fn vec_contains_finds_element() {
    let source = rx("vec_contains_finds_element");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_vec_contains");
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("has_2"), "present: {}", stdout);
    assert!(stdout.contains("missing_99"), "absent: {}", stdout);
}

/// `Vec.clone()` returns a deep copy — modifying the original does
/// not affect the clone.
#[test]
fn vec_clone_returns_independent_copy() {
    let source = rx("vec_clone_returns_independent_copy");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_vec_clone");
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("orig_len=4"), "orig grew: {}", stdout);
    assert!(stdout.contains("copy_len=3"), "copy unchanged: {}", stdout);
}

/// `Vec.reverse()` reverses in place.
#[test]
fn vec_reverse_inverts_order() {
    let source = rx("vec_reverse_inverts_order");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_vec_reverse");
    assert!(ok, "stderr: {}", stderr);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.first().copied(), Some("n=3"), "first after reverse");
    assert_eq!(lines.get(2).copied(), Some("n=1"), "last after reverse");
}
