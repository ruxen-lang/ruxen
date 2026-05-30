//! Phase 2 #06.9 — `any Fn(...)` dyn-erased closure dispatch.
//!
//! Pins the four contexts where T9 of #06.5 found that `any Fn(...)`
//! was not dispatchable today:
//!   1. `let h: any Fn(Int) -> Int = closure` — coerces at let-binding.
//!   2. `Array[any Fn(Int) -> Int]` — store closures, iterate, `.(args)`.
//!   3. Class field of type `Array[any Fn(...)]` (Router pattern).
//!   4. Return position — `def make_adder(n: Int) -> any Fn(Int) -> Int`.
//!
//! A fifth pin (`dyn_fn_e2e_600_handler_dispatch`) mirrors the
//! `tests/release-e2e/cases/600_closure_handler_dispatch.rx` fixture
//! so the regression is caught by `cargo test -p ruxen_core` even
//! before the e2e harness runs.
//!
//! Layout invariant exploited: a closure literal already heap-allocates
//! a 16-byte pair `{fn_ptr, captures_ptr}` (see
//! `mir/lower/expr/closure.rs`), and `Ty::AnyMixin` lays out as a
//! 16-byte primitive (see `codegen/layout.rs:445`). The dyn-erased
//! receiver dereferences slot 0 (fn_ptr) and slot 1 (captures_ptr) —
//! the exact same shape the concrete-`Fn` indirect call already uses.
//! No vtable plumbing is needed because the fn_ptr *is* the dispatch.

use ruxen_core::codegen;
use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;
use std::path::PathBuf;
use std::process::Command;

fn rx(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ruxen")
        .join(format!("{name}.rx"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn workspace_root() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

/// Compile `source`, run the produced binary, return (stdout, stderr,
/// exit-ok). Asserts no typecheck errors.
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
    assert!(
        errors.is_empty(),
        "typecheck errors for {}: {:?}",
        basename,
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .unwrap_or_else(|e| panic!("MIR lowering for {}: {}", basename, e));
    codegen::compile(&mir, bin_path.to_str().unwrap())
        .unwrap_or_else(|e| panic!("codegen for {}: {}", basename, e));

    let output = Command::new(&bin_path)
        .output()
        .unwrap_or_else(|e| panic!("run {}: {}", basename, e));
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.success(),
    )
}

/// 1. `let h: any Fn(Int) -> Int = |n| n + 1` typechecks (no
///    `type mismatch`) and `h.(5)` returns 6.
#[test]
fn dyn_fn_let_binding_coerces() {
    let source = rx("dyn_fn_let_binding_coerces");
    let (stdout, stderr, ok) = compile_and_run(&source, "dyn_fn_let_binding_coerces");
    assert!(ok, "non-zero exit; stderr: {}", stderr);
    assert_eq!(stdout, "6\n", "stdout was {:?}", stdout);
}

/// 2. T9's failing case: `Array[any Fn(Int) -> Int]` storage +
///    iteration + dispatch through the dyn-erased element type.
#[test]
fn dyn_fn_array_store_and_dispatch() {
    let source = rx("dyn_fn_array_store_and_dispatch");
    let (stdout, stderr, ok) = compile_and_run(&source, "dyn_fn_array_store_and_dispatch");
    assert!(ok, "non-zero exit; stderr: {}", stderr);
    // (5 + 10) + (5 * 3) = 30
    assert_eq!(stdout, "30\n", "stdout was {:?}", stdout);
}

/// 3. Router pattern: class field of `Array[any Fn(...)]`, `add` +
///    `dispatch_all` methods, surface from the prompt's §"Surface"
///    case 3.
#[test]
fn dyn_fn_router_pattern() {
    let source = rx("dyn_fn_router_pattern");
    let (stdout, stderr, ok) = compile_and_run(&source, "dyn_fn_router_pattern");
    assert!(ok, "non-zero exit; stderr: {}", stderr);
    assert_eq!(stdout, "30\n", "stdout was {:?}", stdout);
}

/// 4. Return-position: a function returning `any Fn(Int) -> Int`
///    coerces its closure literal at the return point.
#[test]
fn dyn_fn_return_from_function() {
    let source = rx("dyn_fn_return_from_function");
    let (stdout, stderr, ok) = compile_and_run(&source, "dyn_fn_return_from_function");
    assert!(ok, "non-zero exit; stderr: {}", stderr);
    assert_eq!(stdout, "7\n", "stdout was {:?}", stdout);
}

/// 5. Mirrors `tests/release-e2e/cases/600_closure_handler_dispatch.rx`
///    so the regression is caught even before the gated e2e sweep.
///    Reads the same fixture used by the e2e harness so the two
///    sources never drift apart.
#[test]
fn dyn_fn_e2e_600_handler_dispatch() {
    let path = workspace_root().join("tests/release-e2e/cases/600_closure_handler_dispatch.rx");
    let source =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e));
    let (stdout, stderr, ok) = compile_and_run(&source, "dyn_fn_e2e_600_handler_dispatch");
    assert!(ok, "non-zero exit; stderr: {}", stderr);

    let expected_path =
        workspace_root().join("tests/release-e2e/expected/600_closure_handler_dispatch.out");
    let expected = std::fs::read_to_string(&expected_path)
        .unwrap_or_else(|e| panic!("read {}: {}", expected_path.display(), e));
    assert_eq!(stdout, expected, "stdout was {:?}", stdout);
}
