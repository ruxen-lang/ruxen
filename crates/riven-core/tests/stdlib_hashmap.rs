//! Typecheck-level guards for the Phase 2 stdlib `HashMap[K,V]`
//! surface (#04). Pairs with the release-e2e fixtures
//! `tests/release-e2e/cases/50[1-9]_hashmap_*.rvn` and the leak guard
//! in `drop_fixtures.rs::hashmap_*_releases_every_value`.
//!
//! Mirrors the `stdlib_vec_negatives.rs` harness shape: drive the real
//! lex → parse → typecheck pipeline from a source string, then assert
//! on the diagnostic shape.

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

/// `HashMap.new` and `HashMap.with_capacity(n)` are both static
/// constructors registered in `is_builtin_static_method`. Both must
/// typecheck cleanly and yield a `HashMap[K,V]`.
#[test]
fn hashmap_constructors_typecheck() {
    let source = r##"
def main
  let _a: HashMap[Int, Int] = HashMap.new
  let _b: HashMap[Int, Int] = HashMap.with_capacity(8)
end
"##;
    let diags = typecheck_diagnostics(source);
    let errs = errors(&diags);
    assert!(
        errs.is_empty(),
        "HashMap.new / HashMap.with_capacity should typecheck; got: {:#?}",
        errs
    );
}

/// `HashMap.remove(&K) -> Option[V]` (#04). Must typecheck as
/// `Option[V]` so the pattern-match on `Some(_) / None` arms work.
#[test]
fn hashmap_remove_returns_option() {
    let source = r##"
def main
  let mut h: HashMap[Int, Int] = HashMap.new
  h.insert(1, 10)
  match h.remove(1)
    Some(v) -> puts "got=#{v}"
    None    -> puts "miss"
  end
end
"##;
    let diags = typecheck_diagnostics(source);
    let errs = errors(&diags);
    assert!(
        errs.is_empty(),
        "HashMap.remove(_) must typecheck as Option[V]; got: {:#?}",
        errs
    );
}

/// `HashMap.keys -> Vec[&K]` and `HashMap.values -> Vec[&V]`. The v1
/// runtime treats `&K` and `K` identically at the slot level (both 8
/// bytes), so iteration via `for` over the result Vec works directly.
#[test]
fn hashmap_keys_values_iter_typecheck() {
    let source = r##"
def main
  let mut h: HashMap[Int, Int] = HashMap.new
  h.insert(1, 10)
  let _ks: Vec[&Int] = h.keys
  let _vs: Vec[&Int] = h.values
  let _it: Vec[&Int] = h.iter
end
"##;
    let diags = typecheck_diagnostics(source);
    let errs = errors(&diags);
    assert!(
        errs.is_empty(),
        "HashMap.keys / values / iter must typecheck; got: {:#?}",
        errs
    );
}

/// `HashMap.clear` mutates the receiver in place and returns Unit.
#[test]
fn hashmap_clear_typechecks_as_unit() {
    let source = r##"
def main
  let mut h: HashMap[Int, Int] = HashMap.new
  h.insert(1, 10)
  h.clear
end
"##;
    let diags = typecheck_diagnostics(source);
    let errs = errors(&diags);
    assert!(
        errs.is_empty(),
        "HashMap.clear must typecheck cleanly; got: {:#?}",
        errs
    );
}

/// `HashMap == HashMap` routes through the `riven_hash_eq` runtime
/// helper at MIR lowering. Pins the typeck contract: returns Bool.
#[test]
fn hashmap_equality_yields_bool() {
    let source = r##"
def main
  let mut a: HashMap[Int, Int] = HashMap.new
  let mut b: HashMap[Int, Int] = HashMap.new
  a.insert(1, 10)
  b.insert(1, 10)
  let c: Bool = a == b
  puts "#{c}"
end
"##;
    let diags = typecheck_diagnostics(source);
    let errs = errors(&diags);
    assert!(
        errs.is_empty(),
        "HashMap == HashMap must typecheck as Bool; got: {:#?}",
        errs
    );
}

// ── #04 Entry API (entry(K).or_insert(V) / .or_insert_with(closure)) ──

/// `m.entry(K).or_insert(V)` is the v1 entry-API surface. Per prompt 04
/// (deferred sub-task), the chain is recognized as a single syntactic
/// unit; the value is inserted only when the key is absent. Returns
/// Unit (no `&mut V` like Rust — the v1 simplification documented in
/// the prompt). Must typecheck cleanly with concrete K=Int, V=Int.
#[test]
fn hashmap_entry_or_insert_typechecks() {
    let source = r##"
def main
  let mut m: HashMap[Int, Int] = HashMap.new
  m.entry(1).or_insert(10)
  m.entry(1).or_insert(99)
  puts "#{m.len}"
end
"##;
    let diags = typecheck_diagnostics(source);
    let errs = errors(&diags);
    assert!(
        errs.is_empty(),
        "m.entry(k).or_insert(v) must typecheck cleanly; got: {:#?}",
        errs
    );
}

/// `m.entry(K).or_insert_with { || V }` accepts a zero-arg closure
/// whose body type matches V. The closure body is only evaluated
/// when the key is absent (the lazy-default contract). Riven uses a
/// trailing-block form for closure args (parser rejects `(|| 42)` as
/// an inline expression).
#[test]
fn hashmap_entry_or_insert_with_typechecks() {
    let source = r##"
def main
  let mut m: HashMap[String, Int] = HashMap.new
  m.entry("a").or_insert_with { || 42 }
  puts "#{m.len}"
end
"##;
    let diags = typecheck_diagnostics(source);
    let errs = errors(&diags);
    assert!(
        errs.is_empty(),
        "m.entry(k).or_insert_with {{ || v }} must typecheck cleanly; got: {:#?}",
        errs
    );
}
