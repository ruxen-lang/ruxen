//! Negative typeck pin for `BufReader` / `BufWriter`: an inner that is not
//! a `File` or `TcpStream` is E0714. The runtime behaviour tests live in
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
