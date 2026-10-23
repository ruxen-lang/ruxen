//! Typeck-level pin tests for std.regex (Phase 5).
//!
//! These exercise the two diagnostics that fire at type-check time:
//!
//!   * E1702 — `~=` operand type mismatch (LHS must be `String`-like,
//!     RHS must be `Regex`).
//!   * E1704 — invalid regex pattern (the `/pat/flags` body failed
//!     `regex-syntax`'s compile-time validation).
//!
//! Both run the full lexer + parser + (resolve + typeck) pipeline so
//! the diagnostic spans land at the source position the user wrote.

use ruxen_core::diagnostics::{Diagnostic, DiagnosticLevel};
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

fn typeck_errors(source: &str) -> Vec<Diagnostic> {
    let mut lx = Lexer::new(source);
    let toks = lx.tokenize().expect("lex");
    let mut p = Parser::new(toks);
    let prog = p.parse().expect("parse");
    let result = typeck::type_check(&prog);
    result
        .diagnostics
        .into_iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect()
}

fn has_code(diags: &[Diagnostic], code: &str) -> bool {
    diags.iter().any(|d| d.code.as_deref() == Some(code))
}

/// E1702 — `5 ~= /foo/` rejects the Int LHS.
#[test]
fn tilde_eq_rejects_non_string_lhs_e1702() {
    let src = "def main\n  5 ~= /foo/\nend";
    let errs = typeck_errors(src);
    assert!(
        has_code(&errs, "E1702"),
        "expected E1702 for `5 ~= /foo/`, got {:?}",
        errs.iter().map(|d| d.to_string()).collect::<Vec<_>>()
    );
}

/// E1702 — RHS that isn't a Regex (e.g. a String literal) is rejected.
#[test]
fn tilde_eq_rejects_non_regex_rhs_e1702() {
    let src = "def main\n  \"hi\" ~= \"there\"\nend";
    let errs = typeck_errors(src);
    assert!(
        has_code(&errs, "E1702"),
        "expected E1702 for `\"hi\" ~= \"there\"`, got {:?}",
        errs.iter().map(|d| d.to_string()).collect::<Vec<_>>()
    );
}

/// E1704 — a dangling quantifier (`*` with nothing to quantify)
/// produces a well-lexed but compile-rejected pattern.
#[test]
fn invalid_regex_pattern_errors_e1704() {
    // `*foo` — `*` with nothing to repeat on its left. The lexer
    // happily emits `RegexLiteral { pattern: "*foo", flags: "" }`
    // (bracket depth never advances); the typeck-time regex-syntax
    // parse rejects it.
    let src = "def main\n  let r = /*foo/\nend";
    let errs = typeck_errors(src);
    assert!(
        has_code(&errs, "E1704"),
        "expected E1704 for `/*foo/`, got {:?}",
        errs.iter().map(|d| d.to_string()).collect::<Vec<_>>()
    );
}

/// E1704 — an unbalanced group `(unbalanced` produces a compile-
/// time-rejected pattern.
#[test]
fn invalid_regex_unbalanced_group_errors_e1704() {
    let src = "def main\n  let r = /(unbalanced/\nend";
    let errs = typeck_errors(src);
    assert!(
        has_code(&errs, "E1704"),
        "expected E1704 for `/(unbalanced/`, got {:?}",
        errs.iter().map(|d| d.to_string()).collect::<Vec<_>>()
    );
}

/// A well-formed pattern (`/foo/i`) does NOT fire E1704.
#[test]
fn well_formed_regex_no_e1704() {
    let src = "def main\n  let r = /foo/i\nend";
    let errs = typeck_errors(src);
    assert!(
        !has_code(&errs, "E1704"),
        "did not expect E1704 for `/foo/i`, got {:?}",
        errs.iter().map(|d| d.to_string()).collect::<Vec<_>>()
    );
}
