//! Q1 regression — a same-name method overloaded on `&str` vs
//! `any Fn[...]` must dispatch to the overload whose parameter type
//! matches the call-site argument type, independent of declaration
//! order.
//!
//! The MIR symbol selector (`select_method_symbol_name` ->
//! `method_signature_accepts_args`) was missing the by-value-arg /
//! `&T`-param coercion arm that the typeck selector already had. A
//! `Str` literal argument therefore matched NEITHER overload's strict
//! check and fell through to the arity-only fallback, which binds the
//! FIRST-declared overload. With the closure overload declared first,
//! `add("static")` was mis-dispatched to the closure overload and
//! `f.()` ran on a string pointer (crash / heap corruption).
//!
//! The fixture declares the closure overload FIRST on purpose, since
//! that is the ordering that exposed the bug.

use ruxen_core::codegen;
use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

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

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn compile_and_run(source: &str, basename: &str) -> (String, String, bool) {
    let root = workspace_root();
    let tmp_dir = root.join("tmp");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let bin_path = tmp_dir.join(format!(
        "{}-{}-{}.bin",
        basename,
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
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
    let _ = std::fs::remove_file(&bin_path);
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.success(),
    )
}

#[test]
fn q1_overload_str_vs_closure() {
    let source = rx("q1_overload_str_vs_closure");
    let (stdout, stderr, ok) = compile_and_run(&source, "q1_overload_str_vs_closure");
    assert!(ok, "non-zero exit; stderr: {}", stderr);
    assert_eq!(stdout, "static dynamic", "stdout was {:?}", stdout);
}
