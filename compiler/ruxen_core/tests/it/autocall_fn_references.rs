//! Pin tests for type-directed auto-call of function references.
//! See `docs/superpowers/specs/2026-05-29-auto-call-fn-references-design.md`.
//!
//! Rule: a bare reference to a named function/method in a value position
//! whose expected type is NOT a function type is auto-called (zero args).
//! A `Fn`-typed context (annotation / `Fn`-typed param) references it
//! without calling. A bare reference to a function that requires arguments
//! in a non-`Fn` context is E0726.
//!
//! Fixtures: `compiler/ruxen_core/tests/fixtures/ruxen/autocall_*.rx`
//! (team rule against inline `r#"..."#` Ruxen source in `.rs` pin tests).

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

fn typeck_errors(source: &str) -> Vec<Diagnostic> {
    let mut lx = Lexer::new(source);
    let toks = lx.tokenize().expect("lex");
    let mut p = Parser::new(toks);
    let prog = p.parse().expect("parse");
    typeck::type_check(&prog)
        .diagnostics
        .into_iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect()
}

fn count_with_code(errs: &[Diagnostic], code: &str) -> usize {
    errs.iter()
        .filter(|d| d.code.as_deref() == Some(code))
        .count()
}

/// A bare nullary fn reference in a let binding (non-`Fn` context) auto-calls,
/// so the binding has the fn's RETURN type and `.len` resolves.
#[test]
fn bare_nullary_fn_in_let_auto_calls() {
    let errs = typeck_errors(&rx("autocall_let_binding"));
    assert!(
        errs.is_empty(),
        "expected clean typecheck (auto-call), got: {:?}",
        errs.iter()
            .map(|d| (&d.code, &d.message))
            .collect::<Vec<_>>()
    );
}

/// A `Fn`-typed annotation suppresses auto-call: the function is referenced,
/// not called. Must remain valid.
#[test]
fn fn_typed_annotation_suppresses_auto_call() {
    let errs = typeck_errors(&rx("autocall_annotation_suppresses"));
    assert!(
        errs.is_empty(),
        "expected clean typecheck (reference, no call), got: {:?}",
        errs.iter()
            .map(|d| (&d.code, &d.message))
            .collect::<Vec<_>>()
    );
}

/// A bare reference to a fn that needs arguments, in a non-`Fn` context,
/// is E0726 — and ONLY E0726 (no cascading type-mismatch diagnostic).
#[test]
fn bare_multi_arg_fn_reference_is_e0726() {
    let errs = typeck_errors(&rx("autocall_arity_requires_args"));
    assert_eq!(
        count_with_code(&errs, "E0726"),
        1,
        "expected exactly one E0726 for `let f = add`, got: {:?}",
        errs.iter()
            .map(|d| (&d.code, &d.message))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        errs.len(),
        1,
        "E0726 should be the only diagnostic (no cascading mismatch), got: {:?}",
        errs.iter()
            .map(|d| (&d.code, &d.message))
            .collect::<Vec<_>>()
    );
}

/// A bare multi-arg fn reference with a concrete non-`Fn` annotation reports
/// ONLY E0726 — no cascading type-mismatch from the failed annotation unify.
#[test]
fn annotated_multi_arg_fn_reference_is_only_e0726() {
    let errs = typeck_errors(&rx("autocall_arity_annotated"));
    assert_eq!(
        errs.len(),
        1,
        "expected exactly one diagnostic (E0726, no cascade), got: {:?}",
        errs.iter()
            .map(|d| (&d.code, &d.message))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        count_with_code(&errs, "E0726"),
        1,
        "the one diagnostic must be E0726"
    );
}

/// A bare nullary fn reference as a function's tail expression auto-calls
/// against the declared return type.
#[test]
fn bare_nullary_fn_in_return_position_auto_calls() {
    let errs = typeck_errors(&rx("autocall_return_position"));
    assert!(
        errs.is_empty(),
        "expected clean typecheck (return-position auto-call), got: {:?}",
        errs.iter()
            .map(|d| (&d.code, &d.message))
            .collect::<Vec<_>>()
    );
}

/// A bare nullary fn reference passed to a non-`Fn` parameter auto-calls;
/// passed to a `Fn`-typed parameter, it is referenced.
#[test]
fn bare_nullary_fn_in_call_arg_auto_calls_or_references() {
    let errs = typeck_errors(&rx("autocall_call_arg"));
    assert!(
        errs.is_empty(),
        "expected clean typecheck (call-arg auto-call + Fn-param reference), got: {:?}",
        errs.iter()
            .map(|d| (&d.code, &d.message))
            .collect::<Vec<_>>()
    );
}
