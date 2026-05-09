//! Typecheck-level guards for the Phase 2 stdlib `HashSet[T]` surface
//! (#04). Pairs with `tests/release-e2e/cases/52[1-9]_hashset_*.rvn`
//! and `drop_fixtures.rs::hashset_*_releases_every_element`.

use riven_core::diagnostics::{Diagnostic, DiagnosticLevel};
use riven_core::lexer::Lexer;
use riven_core::parser::Parser;
use riven_core::typeck;

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

/// `HashSet[T]` is the v1 alias for `Set[T]`. Both names must work at
/// the type-annotation site, and `HashSet.new` / `HashSet.with_capacity`
/// must reach the runtime via the alias dispatch in
/// `codegen::runtime::runtime_name`.
#[test]
fn hashset_alias_constructs_via_either_name() {
    let source = r##"
def main
  let _a: HashSet[Int] = HashSet.new
  let _b: HashSet[Int] = HashSet.with_capacity(4)
  let _c: Set[Int] = Set.new
end
"##;
    let diags = typecheck_diagnostics(source);
    let errs = errors(&diags);
    assert!(
        errs.is_empty(),
        "HashSet/Set constructors must typecheck cleanly; got: {:#?}",
        errs
    );
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
    let source = r##"
def main
  let mut s: HashSet[Int] = HashSet.new
  s.insert(1)
end
"##;
    let diags = typecheck_diagnostics(source);
    let errs = errors(&diags);
    assert!(
        errs.is_empty(),
        "HashSet.insert(_) must typecheck cleanly today; got: {:#?}",
        errs
    );
}

/// `HashSet.remove(&T) -> Bool` returns true iff the element was
/// present. The MIR/runtime collapse the underlying Option[V] from
/// `riven_hash_remove` into a Bool via the tag word.
#[test]
fn hashset_remove_returns_bool() {
    let source = r##"
def main
  let mut s: HashSet[Int] = HashSet.new
  s.insert(1)
  let r: Bool = s.remove(1)
  puts "#{r}"
end
"##;
    let diags = typecheck_diagnostics(source);
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
    let source = r##"
def main
  let mut a: HashSet[Int] = HashSet.new
  let mut b: HashSet[Int] = HashSet.new
  a.insert(1)
  b.insert(2)
  let _u: HashSet[Int] = a.union(&b)
  let _i: HashSet[Int] = a.intersection(&b)
  let _d: HashSet[Int] = a.difference(&b)
end
"##;
    let diags = typecheck_diagnostics(source);
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
    let source = r##"
def main
  let mut s: HashSet[Int] = HashSet.new
  s.insert(1)
  let _items: Vec[&Int] = s.iter
end
"##;
    let diags = typecheck_diagnostics(source);
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
    let source = r##"
def main
  let mut a: HashSet[Int] = HashSet.new
  let mut b: HashSet[Int] = HashSet.new
  a.insert(1)
  b.insert(1)
  let c: Bool = a == b
  puts "#{c}"
end
"##;
    let diags = typecheck_diagnostics(source);
    let errs = errors(&diags);
    assert!(
        errs.is_empty(),
        "HashSet == HashSet must typecheck as Bool; got: {:#?}",
        errs
    );
}
