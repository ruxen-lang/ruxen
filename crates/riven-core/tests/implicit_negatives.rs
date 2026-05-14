//! Negative tests for the P1.05 derive-synth refactor (sub-item B2+).
//!
//! These tests pin the diagnostics that *must* fire when a derive
//! request can't be satisfied. They sit alongside the positive
//! release-e2e fixtures (`tests/release-e2e/cases/20[2-5]_*.rvn` and
//! `210_*.rvn`) which exercise the green path.
//!
//! Each test compiles a small Riven snippet through the same lex →
//! parse → typecheck pipeline used by `implicit_diagnostics.rs`, but
//! takes the source as a string literal so a single test file holds
//! all related negatives without scattering tiny `.rvn` files across
//! `tests/fixtures/`.

use riven_core::diagnostics::{Diagnostic, DiagnosticLevel};
use riven_core::lexer::Lexer;
use riven_core::parser::Parser;
use riven_core::typeck;

/// Lex + parse + typecheck a Riven source string and return the full
/// diagnostic list. Panics with a clear message on lex/parse failure
/// so the caller never confuses a harness break with a missing-error
/// regression.
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

/// Collect just the `code` strings from a diagnostic list.
fn codes(diags: &[Diagnostic]) -> Vec<String> {
    diags.iter().filter_map(|d| d.code.clone()).collect()
}

/// Deriving `Clone` on a struct whose field type is itself not Clone
/// must emit either:
///
///   * E0610 (`derive_field_does_not_implement` — generic field-shape
///     diagnostic), or
///   * E0611 (`derive_clone_unsupported_shape` — Clone-specific).
///
/// The architect's binding ruling allows either code; coder picks the
/// final mapping in B4. Today the Clone synthesiser doesn't validate
/// fields at all (no synth exists for Clone), so neither code fires
/// — that's the intended red.
#[test]
fn derive_clone_on_struct_with_non_clone_field_emits_e0610_or_e0611() {
    // `NoClone` has no derive list, so it is not Clone.
    // `Outer` derives Clone but holds a `NoClone` field — the
    // synthesised Clone impl can't be generated cleanly.
    let source = r#"
struct NoClone
  x: Int
end

@[derive(Clone)]
struct Outer
  inner: NoClone
end

def main
  let _x = 0
end
"#;

    let diags = typecheck_diagnostics_from_source(source);
    let all_codes = codes(&diags);
    let errors: Vec<&Diagnostic> = diags
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();

    let saw_e0610_or_e0611 = all_codes.iter().any(|c| c == "E0610" || c == "E0611");
    assert!(
        saw_e0610_or_e0611,
        "expected E0610 or E0611 from deriving Clone on a struct \
         whose `inner: NoClone` field is not Clone — got codes {:?} \
         and {} error-level diagnostic(s).",
        all_codes,
        errors.len()
    );
}

/// Deriving `Clone` on an enum whose variant payload is not Clone
/// must emit E0610 or E0611. Mirrors the struct case above for the
/// other major aggregate shape; this surfaces a separate code-path in
/// the upcoming synthesiser (the enum variant walk vs the struct
/// field walk), so we want both pinned.
#[test]
fn derive_clone_on_enum_with_non_clone_payload_emits_e0610_or_e0611() {
    let source = r#"
struct NoClone
  x: Int
end

@[derive(Clone)]
enum Wrapper
  Loaded(inner: NoClone)
  Empty
end

def main
  let _x = 0
end
"#;

    let diags = typecheck_diagnostics_from_source(source);
    let all_codes = codes(&diags);
    let saw_e0610_or_e0611 = all_codes.iter().any(|c| c == "E0610" || c == "E0611");
    assert!(
        saw_e0610_or_e0611,
        "expected E0610 or E0611 from deriving Clone on an enum \
         whose `Loaded` variant carries a non-Clone field — got \
         codes {:?}.",
        all_codes
    );
}

/// Deriving `PartialEq` on a struct whose field type is not PartialEq
/// must emit `E0613` (architect's binding code from B1, registered in
/// `diagnostics/codes.rs`).
#[test]
fn derive_partial_eq_on_struct_with_non_eq_field_emits_e0613() {
    let source = r#"
class Opaque
  x: Int
end

@[derive(PartialEq)]
struct Outer
  inner: Opaque
end

def main
  let _x = 0
end
"#;

    let diags = typecheck_diagnostics_from_source(source);
    let all_codes = codes(&diags);
    assert!(
        all_codes.iter().any(|c| c == "E0613"),
        "expected E0613 when deriving PartialEq on a struct whose \
         field type does not satisfy PartialEq — got codes {:?}.",
        all_codes
    );
}

/// Deriving `Hash` on a struct with a non-hashable field must emit
/// `E0615`. We use a class without `derive Hash` as the offending
/// field type so the validator reports it as not hashable.
#[test]
fn derive_hash_on_struct_with_non_hash_field_emits_e0615() {
    let source = r#"
class NotHashable
  x: Int
end

@[derive(Hash)]
struct Outer
  inner: NotHashable
end

def main
  let _x = 0
end
"#;

    let diags = typecheck_diagnostics_from_source(source);
    let all_codes = codes(&diags);
    assert!(
        all_codes.iter().any(|c| c == "E0615"),
        "expected E0615 when deriving Hash on a struct whose field \
         type is not hashable — got codes {:?}.",
        all_codes
    );
}

/// Deriving `Default` on an *empty* enum must emit `E0616` (the B1
/// "first variant has fields without Default" code; the empty-enum
/// case is the degenerate version where there is no first variant
/// at all).
#[test]
fn derive_default_on_empty_enum_emits_e0616() {
    let source = r#"
@[derive(Default)]
enum Empty
end

def main
  let _x = 0
end
"#;

    let diags = typecheck_diagnostics_from_source(source);
    let all_codes = codes(&diags);
    assert!(
        all_codes.iter().any(|c| c == "E0616"),
        "expected E0616 when deriving Default on an empty enum — got \
         codes {:?}.",
        all_codes
    );
}

/// Deriving `Ord` on a struct with a non-Ord field must emit `E0617`.
/// Same field-type trick as the PartialEq case: a class without any
/// derives never satisfies Ord.
#[test]
fn derive_ord_on_struct_with_non_ord_field_emits_e0617() {
    let source = r#"
class NotOrd
  x: Int
end

@[derive(Eq, Ord, PartialEq, PartialOrd)]
struct Outer
  inner: NotOrd
end

def main
  let _x = 0
end
"#;

    let diags = typecheck_diagnostics_from_source(source);
    let all_codes = codes(&diags);
    assert!(
        all_codes.iter().any(|c| c == "E0617"),
        "expected E0617 when deriving Ord on a struct whose field \
         type does not implement Ord — got codes {:?}.",
        all_codes
    );
}

/// Deriving `PartialOrd` on a struct with a non-PartialOrd field must
/// emit `E0618`.
#[test]
fn derive_partial_ord_on_struct_with_non_partial_ord_field_emits_e0618() {
    let source = r#"
class NotPartialOrd
  x: Int
end

@[derive(PartialOrd, PartialEq)]
struct Outer
  inner: NotPartialOrd
end

def main
  let _x = 0
end
"#;

    let diags = typecheck_diagnostics_from_source(source);
    let all_codes = codes(&diags);
    assert!(
        all_codes.iter().any(|c| c == "E0618"),
        "expected E0618 when deriving PartialOrd on a struct whose \
         field type does not implement PartialOrd — got codes {:?}.",
        all_codes
    );
}
