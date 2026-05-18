//! Implicit-include diagnostics (E0601-E0609).
//!
//! These tests pin the diagnostics emitted by the *auto-synthesis*
//! pipeline (§3.6) when an in-body `include ...` directive names a
//! mixin that cannot be auto-derived against the host type. They are
//! the negative counterpart to the green-path release-e2e fixtures
//! under `tests/release-e2e/cases/20[2-9]_implicit_*.rvn`.
//!
//! The companion file `implicit_negatives.rs` covers the newer
//! E0610-E0618 range (per-field validators introduced in P1.05/B1).
//! This file deliberately stays focused on the E0601-E0609 range,
//! whose codes pre-date that refactor and have been re-purposed as
//! "auto-include of X failed" diagnostics rather than the old
//! `derive` keyword diagnostics they originally described.
//!
//! Every fixture lives in `tests/fixtures/riven/<test-fn-name>.rvn`
//! and is loaded at runtime via the `rvn(name)` helper below.

use riven_core::diagnostics::{Diagnostic, DiagnosticLevel};
use riven_core::lexer::Lexer;
use riven_core::parser::Parser;
use riven_core::typeck;

/// Read a Riven fixture file from `tests/fixtures/riven/<name>.rvn`.
fn rvn(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/riven")
        .join(format!("{name}.rvn"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

/// Lex + parse + typecheck a Riven source string and return every
/// diagnostic the pipeline produced. Panics on lex/parse failure so a
/// harness break is never confused with a missing diagnostic.
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

fn has_code(diags: &[Diagnostic], code: &str) -> bool {
    diags.iter().any(|diag| diag.code.as_deref() == Some(code))
}

/// Green path: a struct whose every field is `Copy` should auto-derive
/// Copy + Clone with no diagnostics, even with explicit in-body
/// `include Copy` / `include Clone` directives. This pins the
/// "no false positives" half of the validator.
#[test]
fn derive_copy_clone_on_pod_struct_typechecks_cleanly() {
    let source = rvn("derive_copy_clone_on_pod_struct_typechecks_cleanly");

    let diags = typecheck_diagnostics_from_source(&source);
    let errors: Vec<&Diagnostic> = diags
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "unexpected error-level diagnostics on POD Copy+Clone struct: {:?}",
        errors
    );
}

/// E0601: deriving Copy on a struct whose field type is not Copy
/// (here: `String`) must be rejected at the validator.
#[test]
fn copy_with_non_copy_field_reports_e0601() {
    let source = rvn("copy_with_non_copy_field_reports_e0601");

    let diags = typecheck_diagnostics_from_source(&source);
    assert!(
        has_code(&diags, "E0601"),
        "expected E0601 when deriving Copy on a struct with a non-Copy \
         `String` field — got diagnostics {:?}.",
        diags
    );
}

/// E0602: `Copy` implies `Clone`. Asking for Copy without Clone must
/// be rejected. (The annotation parser lets the list be malformed in
/// this way; the validator catches it.)
#[test]
fn copy_without_clone_reports_e0602() {
    let source = rvn("copy_without_clone_reports_e0602");

    let diags = typecheck_diagnostics_from_source(&source);
    assert!(
        has_code(&diags, "E0602"),
        "expected E0602 when deriving Copy without Clone — got \
         diagnostics {:?}.",
        diags
    );
}

/// E0603: `Copy` cannot be auto-synthesised on a `class` — classes
/// have reference identity and (potentially) destructors, so silently
/// bit-copying them would alias ownership.
#[test]
fn copy_on_class_reports_e0603() {
    let source = rvn("copy_on_class_reports_e0603");

    let diags = typecheck_diagnostics_from_source(&source);
    assert!(
        has_code(&diags, "E0603"),
        "expected E0603 when deriving Copy on a class — got \
         diagnostics {:?}.",
        diags
    );
}

/// E0604: `Eq` implies `PartialEq`. The validator must reject Eq on a
/// type that does not also opt into PartialEq.
#[test]
fn eq_without_partial_eq_reports_e0604() {
    let source = rvn("eq_without_partial_eq_reports_e0604");

    let diags = typecheck_diagnostics_from_source(&source);
    assert!(
        has_code(&diags, "E0604"),
        "expected E0604 when deriving Eq without PartialEq — got \
         diagnostics {:?}.",
        diags
    );
}

/// E0605: deriving `Default` on a non-empty enum that does not mark a
/// variant with `@[default]` must be rejected — the synthesiser has
/// no canonical zero variant to construct. (The empty-enum case is
/// E0616, covered in `implicit_negatives.rs`.)
#[test]
fn default_on_enum_without_default_variant_reports_e0605() {
    let source = rvn("default_on_enum_without_default_variant_reports_e0605");

    let diags = typecheck_diagnostics_from_source(&source);
    assert!(
        has_code(&diags, "E0605"),
        "expected E0605 when deriving Default on an enum without a \
         `@[default]` variant — got diagnostics {:?}.",
        diags
    );
}

/// E0606: `Ord` requires both `Eq` and `PartialOrd`. The validator
/// must reject a partial derive set that names Ord alone (or Ord +
/// only one of the two prerequisites).
#[test]
fn ord_without_eq_and_partial_ord_reports_e0606() {
    let source = rvn("ord_without_eq_and_partial_ord_reports_e0606");

    let diags = typecheck_diagnostics_from_source(&source);
    assert!(
        has_code(&diags, "E0606"),
        "expected E0606 when deriving Ord without Eq + PartialOrd — \
         got diagnostics {:?}.",
        diags
    );
}

// E0607 was specifically "`@[derive(...)]` cannot be applied to a
// function" — a pure attribute-form diagnostic. With the `@[...]`
// prefix attribute retired (ruby-naming.spec.md §10a), that test
// premise is obsolete. The diagnostic code is preserved in the
// registry for backward-compat; the test that exercised it has been
// removed.

/// E0608: only the auto-synthesisable mixins (Debug, Clone, Copy,
/// PartialEq, Eq, Hash/Hashable, Default, Ord, PartialOrd) may appear
/// in an in-body `include`. Anything else must be flagged.
#[test]
fn unknown_derive_trait_reports_e0608() {
    let source = rvn("unknown_derive_trait_reports_e0608");

    let diags = typecheck_diagnostics_from_source(&source);
    assert!(
        has_code(&diags, "E0608"),
        "expected E0608 when `include` names a non-synthesisable \
         mixin — got diagnostics {:?}.",
        diags
    );
}

/// E0609: naming the same auto-synthesisable mixin twice via in-body
/// `include` directives is almost always a copy-paste mistake and
/// must be rejected.
#[test]
fn duplicate_derive_trait_reports_e0609() {
    let source = rvn("duplicate_derive_trait_reports_e0609");

    let diags = typecheck_diagnostics_from_source(&source);
    assert!(
        has_code(&diags, "E0609"),
        "expected E0609 when in-body `include` directives repeat a trait \
         name — got diagnostics {:?}.",
        diags
    );
}
