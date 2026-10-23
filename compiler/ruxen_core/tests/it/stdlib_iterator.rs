//! Typecheck negative for the stdlib `Enumerable#sum` bound.
//!
//! The runtime behaviour of the iterator methods is tested in
//! `library/std/iter/tests/*.rx` (run with `scripts/test-stdlib.sh iter`).

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
    let tokens = lexer
        .tokenize()
        .unwrap_or_else(|e| panic!("lexer failed: {:?}", e));
    let mut parser = Parser::new(tokens);
    let program = parser
        .parse()
        .unwrap_or_else(|e| panic!("parser failed: {:?}", e));
    typeck::type_check(&program).diagnostics
}

/// `Vec[String].iter.sum` — String does not implement Add, so a
/// numeric `sum` makes no sense. Typeck rejects the call with `E0700`
/// ("`sum` requires an iterator whose Item implements `Add`").
#[test]
fn sum_on_string_iter_typeck_rejects() {
    let source = rx("sum_on_string_iter_typeck_rejects");
    let diags = typecheck_diagnostics(&source);
    let errors: Vec<&Diagnostic> = diags
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(
        !errors.is_empty(),
        "expected `*Iter[String].sum` to be rejected at typeck"
    );
    let saw_e0700 = errors.iter().any(|d| d.code.as_deref() == Some("E0700"));
    assert!(
        saw_e0700,
        "expected diagnostic code E0700 on the rejection; got: {:#?}",
        errors
    );
    let saw_message = errors
        .iter()
        .any(|d| d.message.contains("Add") || d.message.contains("numeric"));
    assert!(
        saw_message,
        "expected diagnostic message to mention `Add`/numeric; got: {:#?}",
        errors
    );
}
