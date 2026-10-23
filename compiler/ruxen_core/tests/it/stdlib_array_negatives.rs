//! Typecheck negatives for the stdlib `Array[T]` surface.
//!
//! The runtime behaviour of the Array methods is tested in
//! `library/std/array/tests/*.rx` (run with `scripts/test-stdlib.sh array`).

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

/// Ruby `sort!` sorts in place and returns `nil`, so binding its result to an
/// `Array[Int]` is a type mismatch. Asserted by message: the typeck
/// type-mismatch diagnostic is emitted without a registered code today.
#[test]
fn vec_sort_bang_returns_nil() {
    let source = rx("vec_sort_bang_returns_nil");
    let diags = typecheck_diagnostics(&source);
    let errs: Vec<&Diagnostic> = diags
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(
        errs.iter().any(|d| d.message.contains("type mismatch")),
        "Array#sort! returns nil; binding it to Array[Int] must be a type mismatch. got: {:#?}",
        errs
    );
}

/// `Array[T]` is the Ruby spelling; the Rust `Vec` is gone
/// (ruby-naming.spec.md), so naming it is an undefined type.
#[test]
fn vec_is_an_undefined_type() {
    let diags = typecheck_diagnostics("def main\n  let v: Vec[Int] = []\nend");
    assert!(
        diags.iter().any(
            |d| d.level == DiagnosticLevel::Error && d.message.contains("undefined type `Vec`")
        ),
        "{diags:?}"
    );
}

/// The errors coded `code` in `diags`.
fn coded<'a>(diags: &'a [Diagnostic], code: &str) -> Vec<&'a Diagnostic> {
    diags
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error && d.code.as_deref() == Some(code))
        .collect()
}

/// `Array` declares no `from`, so `Array.from(123)` is E0740 at typeck. It
/// used to type as a fresh variable and fail only at link time.
#[test]
fn array_from_is_no_method() {
    let diags = typecheck_diagnostics("def main\n  let a = Array.from(123)\nend");
    let errs = coded(&diags, "E0740");
    assert!(
        errs.iter()
            .any(|d| d.message == "no method `from` on type `Array`"),
        "{diags:?}"
    );
}

/// `Array.with_capacity(cap: Int)` rejects a non-Int capacity with E0741.
#[test]
fn array_with_capacity_takes_an_int() {
    let diags =
        typecheck_diagnostics("def main\n  let a: Array[Int] = Array.with_capacity(\"x\")\nend");
    let errs = coded(&diags, "E0741");
    assert!(
        errs.iter().any(|d| d
            .message
            .contains("`cap` of `with_capacity` expects `Int`, found `String`")),
        "{diags:?}"
    );
}
