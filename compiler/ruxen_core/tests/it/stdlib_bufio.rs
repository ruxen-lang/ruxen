//! Negative typeck pin for `BufReader` / `BufWriter`: both are modules, so
//! `BufReader.new(..)` is E0714 pointing to `BufReader.File` /
//! `BufReader.Tcp`. The runtime behaviour tests live in
//! `library/std/bufio/tests`.
//!
//! Every Ruxen snippet lives in `tests/fixtures/ruxen/` — no inline
//! `r#"..."#` per the cleanup convention (see commit 70ad1fb).

use ruxen_core::diagnostics::{Diagnostic, DiagnosticLevel};
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

fn rx(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ruxen")
        .join(format!("{name}.rx"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn typecheck_diagnostics(source: &str) -> Vec<Diagnostic> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    typeck::type_check(&program).diagnostics
}

/// **Negative E0714**: `BufReader.new(file)` is a typeck error naming the
/// module's classes, not a silently dropped call.
#[test]
fn bufreader_module_new_emits_e0714() {
    let source = rx("bufreader_module_new_emits_e0714");
    let diags = typecheck_diagnostics(&source);
    let errs: Vec<&Diagnostic> = diags
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    let e0714 = errs
        .iter()
        .find(|d| d.code.as_deref() == Some("E0714"))
        .unwrap_or_else(|| panic!("expected E0714; got: {errs:#?}"));
    assert!(
        e0714.message.contains("`BufReader.File.new`")
            && e0714.message.contains("`BufReader.Tcp.new`"),
        "E0714 must point to the module's classes; got: {}",
        e0714.message
    );
}
