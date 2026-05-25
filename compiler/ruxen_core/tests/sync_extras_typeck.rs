//! Pin tests for `docs/specs/stdlib/sync.spec.md` typeck surface.
//!
//! The `std_sync_concurrency_surface_typechecks_cleanly` test covers
//! the umbrella; this file adds **named** pins for individual methods
//! that previously typechecked transitively (`try_lock`,
//! `into_inner`, `deref_mut`, `Arc.strong_count`, `Arc.weak_count`).
//! All assertions are typeck-only — sync.spec.md B4/B5 docs the
//! runtime gap.

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
    let result = typeck::type_check(&prog);
    result
        .diagnostics
        .into_iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect()
}

fn assert_clean(source: &str) {
    let errs = typeck_errors(source);
    assert!(
        errs.is_empty(),
        "expected no typeck errors, got: {:?}",
        errs.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

/// `Mutex.try_lock() -> Option[MutexGuard[T]]` typechecks.
#[test]
fn mutex_try_lock_returns_option() {
    let source = rx("mutex_try_lock_returns_option");
    assert_clean(&source);
}

/// `Mutex.into_inner() -> Result[T, PoisonError]` typechecks.
#[test]
fn mutex_into_inner_returns_result() {
    let source = rx("mutex_into_inner_returns_result");
    assert_clean(&source);
}

/// `MutexGuard.deref_mut() -> &mut T` typechecks.
#[test]
fn mutex_guard_deref_mut_returns_mut_ref() {
    let source = rx("mutex_guard_deref_mut_returns_mut_ref");
    assert_clean(&source);
}

/// `Arc.strong_count()` and `weak_count()` return `USize`.
#[test]
fn arc_count_methods_return_usize() {
    let source = rx("arc_count_methods_return_usize");
    assert_clean(&source);
}

/// `Arc.deref()` returns `&T`.
#[test]
fn arc_deref_returns_ref() {
    let source = rx("arc_deref_returns_ref");
    assert_clean(&source);
}
