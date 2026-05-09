//! Negative tests for Phase 2 stdlib (#04 batch 3): HashMap / HashSet
//! key-type constraints.
//!
//! These tests pin the diagnostics that must fire when a HashMap or
//! HashSet is constructed with a key/element type that is not Hash.
//! Compound containers (`Vec`, `Set`/`HashSet`, `HashMap`) are
//! deliberately *not* Hash in v1 even when their element types are —
//! see `crates/riven-core/src/resolve/mod.rs::ty_is_valid_hash_key`.
//!
//! The diagnostic carries error code `E0615` (registered in
//! `diagnostics/codes.rs` as "derive Hash: field type is not hashable",
//! reused here for the parallel "type-construction" site).
//!
//! Sits alongside the positive release-e2e fixtures
//! (`tests/release-e2e/cases/50[1-9]_hashmap_*.rvn`,
//! `52[1-7]_hashset_*.rvn`).

use riven_core::diagnostics::{Diagnostic, DiagnosticLevel};
use riven_core::lexer::Lexer;
use riven_core::parser::Parser;
use riven_core::typeck;

/// Lex + parse + typecheck a Riven source string and return the full
/// diagnostic list. Panics on lex/parse failure so the caller never
/// confuses a harness break with a missing-error regression.
fn typecheck_diagnostics_from_source(source: &str) -> Vec<Diagnostic> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer
        .tokenize()
        .unwrap_or_else(|e| panic!("lexer failed on negative-test source: {:?}", e));
    let mut parser = Parser::new(tokens);
    let program = parser
        .parse()
        .unwrap_or_else(|e| panic!("parser failed on negative-test source: {:?}", e));
    typeck::type_check(&program).diagnostics
}

fn codes(diags: &[Diagnostic]) -> Vec<String> {
    diags.iter().filter_map(|d| d.code.clone()).collect()
}

/// `HashMap[Vec[Int], V]` must reject the key type because `Vec` is not
/// Hash in v1 (its heap pointer's identity is unstable across reallocs).
/// The resolver emits E0615 at the type-construction site.
#[test]
fn hashmap_with_non_hash_key_emits_e0615() {
    let source = r#"
def main
  let mut h: HashMap[Vec[Int], Int] = HashMap.new
end
"#;
    let diags = typecheck_diagnostics_from_source(source);
    let errs: Vec<_> = diags
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(
        !errs.is_empty(),
        "expected at least one error for HashMap[Vec[Int], Int], got none. all diags: {:#?}",
        diags
    );
    let cs = codes(&diags);
    assert!(
        cs.iter().any(|c| c == "E0615"),
        "expected E0615 in {:?}",
        cs
    );
}

/// `HashSet[Vec[Int]]` mirrors the HashMap case. The element type T
/// must be Hash + Eq; `Vec` is not.
#[test]
fn hashset_with_non_hash_element_emits_e0615() {
    let source = r#"
def main
  let mut s: HashSet[Vec[Int]] = HashSet.new
end
"#;
    let diags = typecheck_diagnostics_from_source(source);
    let errs: Vec<_> = diags
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(
        !errs.is_empty(),
        "expected at least one error for HashSet[Vec[Int]], got none. all diags: {:#?}",
        diags
    );
    let cs = codes(&diags);
    assert!(
        cs.iter().any(|c| c == "E0615"),
        "expected E0615 in {:?}",
        cs
    );
}

/// Nested compound: `HashMap[HashSet[Int], Int]` — inner HashSet is
/// itself a compound container and not Hash.
#[test]
fn hashmap_with_nested_compound_key_emits_e0615() {
    let source = r#"
def main
  let mut h: HashMap[HashSet[Int], Int] = HashMap.new
end
"#;
    let diags = typecheck_diagnostics_from_source(source);
    let cs = codes(&diags);
    assert!(
        cs.iter().any(|c| c == "E0615"),
        "expected E0615 for HashMap[HashSet[Int], Int] in {:?}",
        cs
    );
}

/// HashSet[HashMap[Int,Int]] — same shape, set of maps.
#[test]
fn hashset_of_hashmap_emits_e0615() {
    let source = r#"
def main
  let mut s: HashSet[HashMap[Int, Int]] = HashSet.new
end
"#;
    let diags = typecheck_diagnostics_from_source(source);
    let cs = codes(&diags);
    assert!(
        cs.iter().any(|c| c == "E0615"),
        "expected E0615 for HashSet[HashMap[Int,Int]] in {:?}",
        cs
    );
}

/// Sanity check: `HashMap[Int, Vec[Int]]` is fine — only the *key* is
/// constrained. Values may be anything.
#[test]
fn hashmap_with_vec_value_is_accepted() {
    let source = r#"
def main
  let mut h: HashMap[Int, Vec[Int]] = HashMap.new
end
"#;
    let diags = typecheck_diagnostics_from_source(source);
    let errs: Vec<_> = diags
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(
        errs.is_empty(),
        "HashMap[Int, Vec[Int]] should typecheck, but got errors: {:#?}",
        errs
    );
}

/// Sanity check: primitive keys are accepted.
#[test]
fn hashmap_with_string_key_is_accepted() {
    let source = r#"
def main
  let mut h: HashMap[String, Int] = HashMap.new
end
"#;
    let diags = typecheck_diagnostics_from_source(source);
    let errs: Vec<_> = diags
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(
        errs.is_empty(),
        "HashMap[String, Int] should typecheck, but got errors: {:#?}",
        errs
    );
}
