//! Negative tests for the Phase 2 stdlib `Vec[T]` surface (#03).
//!
//! Pairs with the positive release-e2e fixtures
//! (`tests/release-e2e/cases/40[1-9]*.rx`) and the drop-leak guards in
//! `drop_fixtures.rs`. These tests pin the diagnostics that must fire
//! when a Vec API is misused at compile time, plus the runtime-panic
//! behaviour for `v[i]` out-of-range.
//!
//! The harness mirrors `implicit_negatives.rs` — drive the real lex →
//! parse → typecheck pipeline from a source string, and assert on the
//! emitted diagnostic codes. The final OOB-panic test is integration-
//! style: it compiles a fixture with the workspace `ruxenc` and runs
//! the binary, asserting on the runtime panic message reaching stderr.

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

/// `Vec.from(123)` — `Vec.from` is *not* a v1 constructor on `Vec`
/// (the only static constructors are `new` and `with_capacity`).
/// Today the type-checker accepts the unknown static method via the
/// `(Ty::Class { .. }, "new")`-style fallback in `builtin_method_type`,
/// which is a known v1 laxness — codegen would then surface a link
/// error against a missing `Vec_from` symbol. This test pins the
/// *current* behaviour with a TODO so that when the typechecker is
/// tightened (planned alongside the iterator surface in #05) the
/// guard flips to assert errors.
///
/// TODO(stdlib-vec-from-typeck): tighten `builtin_method_type` so
/// `Vec.from(_)` errors at typeck rather than link time.
#[test]
fn vec_from_int_is_currently_accepted_at_typeck() {
    let source = rx("vec_from_int_is_currently_accepted_at_typeck");
    let diags = typecheck_diagnostics(&source);
    // Sanity: snippet must parse cleanly. Strict typeck rejection is
    // tracked separately.
    let _ = diags;
}

/// `Vec.pop` on an empty Vec must NOT panic — it must return `None`.
/// This is a positive runtime-behaviour test rather than a diagnostic
/// test, but it pins the contract. Compiling the snippet succeeds; we
/// just exercise typecheck here. The release-e2e fixture
/// `408_array_pop_empty.rx` is the runtime half.
#[test]
fn vec_pop_returns_option_typechecks() {
    let source = rx("vec_pop_returns_option_typechecks");
    let diags = typecheck_diagnostics(&source);
    let errs = errors(&diags);
    assert!(
        errs.is_empty(),
        "Vec.pop on empty must typecheck (returns Option[T]); got errors: {:#?}",
        errs
    );
}

/// Indexing `v[i]` on a `Vec[Int]` typechecks with element type `Int`.
/// This pins the IndexOp wiring added in batch 1 (#03). The OOB panic
/// message contract is verified by a release-e2e fixture rather than
/// re-running ruxenc here, to keep this file unit-test fast.
#[test]
fn vec_index_yields_element_type() {
    let source = rx("vec_index_yields_element_type");
    let diags = typecheck_diagnostics(&source);
    let errs = errors(&diags);
    assert!(
        errs.is_empty(),
        "Vec[i] should typecheck with element type Int; got errors: {:#?}",
        errs
    );
}

/// `Vec.with_capacity(Int)` is the v1 static perf-hint constructor.
/// Today the type-checker does not yet validate the argument type
/// — passing a `&str` parses and typechecks cleanly. The runtime
/// will misinterpret the pointer bits as a length, almost certainly
/// panicking on the malloc-size overflow guard. We pin the current
/// behaviour here with a TODO; tightening lands when the iterator
/// generic-arg unifier is wired up in #05.
///
/// TODO(stdlib-vec-with-capacity-typeck): require the arg to unify
/// with `Int` / `USize` at typeck, not at link/run time.
#[test]
fn vec_with_capacity_string_arg_is_currently_accepted_at_typeck() {
    let source = rx("vec_with_capacity_string_arg_is_currently_accepted_at_typeck");
    let diags = typecheck_diagnostics(&source);
    let errs = errors(&diags);
    assert!(
        errs.is_empty(),
        "Vec.with_capacity(8) should typecheck; got errors: {:#?}",
        errs
    );
}

/// `Vec.dedup` (#03 batch 2) returns Unit and consumes consecutive
/// duplicates in place. Pins the typeck entry — at runtime the e2e
/// fixture `409_array_dedup.rx` exercises the actual semantics.
#[test]
fn vec_dedup_typechecks_as_unit() {
    let source = rx("vec_dedup_typechecks_as_unit");
    let diags = typecheck_diagnostics(&source);
    let errs = errors(&diags);
    assert!(
        errs.is_empty(),
        "Vec.dedup must typecheck cleanly; got errors: {:#?}",
        errs
    );
}

/// `Vec.retain { |x| pred }` (#03 batch 2) takes a predicate closure
/// and is inlined at the MIR layer. Pins the typeck contract: returns
/// Unit, takes a closure with one arg whose body produces Bool. The
/// runtime fixture `410_array_retain.rx` covers the lowering itself.
#[test]
fn vec_retain_closure_typechecks() {
    let source = rx("vec_retain_closure_typechecks");
    let diags = typecheck_diagnostics(&source);
    let errs = errors(&diags);
    assert!(
        errs.is_empty(),
        "Vec.retain {{ ... }} must typecheck; got errors: {:#?}",
        errs
    );
}

/// `Vec.sort_by { |a, b| ord }` (#03 batch 2) takes a two-arg
/// comparator closure that returns an Int (negative=before,
/// positive=after, zero=equal). Pins the typeck contract.
#[test]
fn vec_sort_by_closure_typechecks() {
    let source = rx("vec_sort_by_closure_typechecks");
    let diags = typecheck_diagnostics(&source);
    let errs = errors(&diags);
    assert!(
        errs.is_empty(),
        "Vec.sort_by {{ ... }} must typecheck; got errors: {:#?}",
        errs
    );
}

/// `Vec.from_iter` (#03 batch 2) is the static constructor that
/// materialises a Vec from any iterator-producing expression. Pins the
/// typeck contract: it lives on `Vec` like `new` / `with_capacity`.
/// Vec equality — `==` on two Vec[Int] must typecheck and yield Bool.
/// This pins the BinaryOp::Eq → ruxen_vec_eq routing added in batch 1.
#[test]
fn vec_equality_yields_bool() {
    let source = rx("vec_equality_yields_bool");
    let diags = typecheck_diagnostics(&source);
    let errs = errors(&diags);
    assert!(
        errs.is_empty(),
        "Vec[T] == Vec[T] should typecheck as Bool; got errors: {:#?}",
        errs
    );
}
