//! Typecheck-level guards for the Phase 2 stdlib `HashSet[T]` surface
//! (#04). Pairs with `tests/release-e2e/cases/52[1-9]_hashset_*.rx`
//! and `drop_fixtures.rs::hashset_*_releases_every_element`.

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

fn errors(diags: &[Diagnostic]) -> Vec<&Diagnostic> {
    diags
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect()
}

/// `HashSet.insert(T) -> Bool` per the v1 surface (returns true if
/// newly inserted) — but the v1 runtime currently signals dedup via
/// the inner hash's len delta, which the typecheck doesn't know.
/// Today `insert` typechecks as Unit (matching `Set.insert`); the
/// per-call true/false signal is observed via `len` change. Pinned
/// here so a future tightening flips the test rather than silently
/// changing surface.
#[test]
fn hashset_insert_typechecks_as_unit_today() {
    let source = rx("hashset_insert_typechecks_as_unit_today");
    let diags = typecheck_diagnostics(&source);
    let errs = errors(&diags);
    assert!(
        errs.is_empty(),
        "HashSet.insert(_) must typecheck cleanly today; got: {:#?}",
        errs
    );
}

/// `HashSet.remove(&T) -> Bool` returns true iff the element was
/// present. The MIR/runtime collapse the underlying Option[V] from
/// `ruxen_hash_remove` into a Bool via the tag word.
#[test]
fn hashset_remove_returns_bool() {
    let source = rx("hashset_remove_returns_bool");
    let diags = typecheck_diagnostics(&source);
    let errs = errors(&diags);
    assert!(
        errs.is_empty(),
        "HashSet.remove(_) must typecheck as Bool; got: {:#?}",
        errs
    );
}

/// Set-ops (`union` / `intersection` / `difference`) take a borrow
/// of another set and return a freshly-allocated `HashSet[T]`. The
/// new set is registered in `FRESH_ALLOC_CALLEES` so its lifetime is
/// the caller's drop frame.
#[test]
fn hashset_set_ops_return_fresh_set() {
    let source = rx("hashset_set_ops_return_fresh_set");
    let diags = typecheck_diagnostics(&source);
    let errs = errors(&diags);
    assert!(
        errs.is_empty(),
        "HashSet set-ops must typecheck and yield HashSet[T]; got: {:#?}",
        errs
    );
}

/// `HashSet.iter -> Vec[&T]` — same shape as `HashMap.iter` (eager
/// iterator that's actually a Vec at the runtime layer). Lazy iter
/// lands in #05.
#[test]
fn hashset_iter_returns_vec() {
    let source = rx("hashset_iter_returns_vec");
    let diags = typecheck_diagnostics(&source);
    let errs = errors(&diags);
    assert!(
        errs.is_empty(),
        "HashSet.iter must typecheck as Vec[&T]; got: {:#?}",
        errs
    );
}

/// `HashSet == HashSet` — wired in mir/lower.rs alongside the Vec /
/// HashMap binop wiring. Returns Bool.
#[test]
fn hashset_equality_yields_bool() {
    let source = rx("hashset_equality_yields_bool");
    let diags = typecheck_diagnostics(&source);
    let errs = errors(&diags);
    assert!(
        errs.is_empty(),
        "HashSet == HashSet must typecheck as Bool; got: {:#?}",
        errs
    );
}
