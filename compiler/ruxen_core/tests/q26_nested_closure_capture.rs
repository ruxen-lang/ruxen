//! Q26 — a capturing closure STORED through a `&var *self` reborrow (or, more
//! generally, a closure nested inside another closure's body) must keep its
//! captures. Before the fix the nested closure's free-variable analysis only
//! consulted `def_to_local`, so a variable captured by the OUTER block (living
//! in `capture_map`) was never re-captured: the nested closure got a NULL
//! captures pointer and read the value as slot garbage (0). Symptom in the
//! ledger: `box.call0` printed 1 instead of 43, and SIGSEGV'd for a captured
//! class handle.
//!
//! These pins read the SAME fixtures the release-e2e harness runs
//! (`tests/release-e2e/cases/615_*`, `616_*`) so the cargo pin and the e2e
//! case can never drift apart — the `dyn_fn_e2e_600` convention.

use ruxen_core::codegen;
use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;
use std::path::PathBuf;
use std::process::Command;

fn workspace_root() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

fn compile_and_run(source: &str, basename: &str) -> (String, String, bool) {
    let root = workspace_root();
    let tmp_dir = root.join("tmp");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let bin_path = tmp_dir.join(format!("{}-{}.bin", basename, std::process::id()));

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
    assert!(
        errors.is_empty(),
        "typecheck errors for {basename}: {errors:?}"
    );

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .unwrap_or_else(|e| panic!("MIR lowering for {basename}: {e}"));
    codegen::compile(&mir, bin_path.to_str().unwrap())
        .unwrap_or_else(|e| panic!("codegen for {basename}: {e}"));

    let output = Command::new(&bin_path)
        .output()
        .unwrap_or_else(|e| panic!("run {basename}: {e}"));
    let _ = std::fs::remove_file(&bin_path);
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.success(),
    )
}

fn case(name: &str) -> (String, String) {
    let root = workspace_root();
    let src = std::fs::read_to_string(root.join("tests/release-e2e/cases").join(name))
        .unwrap_or_else(|e| panic!("read case {name}: {e}"));
    let out_name = name.replace(".rx", ".out");
    let expected = std::fs::read_to_string(root.join("tests/release-e2e/expected").join(out_name))
        .unwrap_or_else(|e| panic!("read expected for {name}: {e}"));
    (src, expected)
}

/// The ledger repro: closure storing `{ || v + 1 }` through `b.(&var *self)`.
/// Must print 43 (v captured == 42), not 1.
#[test]
fn capture_survives_self_reborrow() {
    let (src, expected) = case("615_nested_closure_capture_reborrow.rx");
    let (stdout, stderr, ok) = compile_and_run(&src, "q26_reborrow");
    assert!(ok, "non-zero exit; stderr: {stderr}");
    assert_eq!(stdout, expected, "stdout was {stdout:?}");
}

/// The class-handle variant the ledger flagged as a SIGSEGV: the re-captured
/// value is a `Cell` instance and a field is read through it on invocation.
/// Must not segfault and must print 43.
#[test]
fn class_handle_capture_survives_self_reborrow() {
    let (src, expected) = case("616_nested_closure_capture_class_handle.rx");
    let (stdout, stderr, ok) = compile_and_run(&src, "q26_class_handle");
    assert!(ok, "segfault/non-zero exit; stderr: {stderr}");
    assert_eq!(stdout, expected, "stdout was {stdout:?}");
}
