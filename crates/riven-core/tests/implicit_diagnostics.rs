//! Implicit-include diagnostics (E0601-E0609).
//!
//! These tests pin the diagnostics emitted by the *auto-synthesis*
//! pipeline (§3.6) when an `@[derive(...)]` annotation is rejected.
//! They are the negative counterpart to the green-path release-e2e
//! fixtures under `tests/release-e2e/cases/20[2-9]_implicit_*.rvn`.
//!
//! The companion file `implicit_negatives.rs` covers the newer
//! E0610-E0618 range (per-field validators introduced in P1.05/B1).
//! This file deliberately stays focused on the E0601-E0609 range,
//! whose codes pre-date that refactor and have been re-purposed as
//! "auto-include of X failed" diagnostics rather than the old
//! `derive` keyword diagnostics they originally described.
//!
//! Every fixture is inlined as a raw string literal so the test file
//! is self-contained — the previous file-based fixtures under
//! `tests/fixtures/derive/` were removed when the `derive` keyword was
//! retired in favour of `@[derive(...)]` annotations and implicit
//! synthesis.

use riven_core::diagnostics::{Diagnostic, DiagnosticLevel};
use riven_core::lexer::Lexer;
use riven_core::parser::Parser;
use riven_core::typeck;

/// Lex + parse a Riven source string and return the parser-level
/// diagnostics. Panics on lex failure (those are harness breaks, not
/// the regressions this file is pinning).
fn parse_errors_from_source(source: &str) -> Vec<Diagnostic> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer
        .tokenize()
        .unwrap_or_else(|e| panic!("lexer failed on negative-test source: {:?}", e));
    let mut parser = Parser::new(tokens);
    parser
        .parse()
        .expect_err("fixture should fail during parse")
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
/// Copy + Clone with no diagnostics, even with an explicit
/// `@[derive(Copy, Clone)]` annotation. This pins the "no false
/// positives" half of the validator.
///
/// Note: this source is wrapped in `r##"..."##` rather than `r#"..."#`
/// so the `"#` sequence inside Riven string interpolation
/// (`"#{a.x}"`) does not accidentally close the Rust raw-string
/// literal.
#[test]
fn derive_copy_clone_on_pod_struct_typechecks_cleanly() {
    let source = r##"
@[derive(Copy, Clone)]
struct Point
  x: Int
  y: Int
end

def main
  let a = Point.new(1, 2)
  let b = a
  puts "#{a.x} #{a.y}"
  puts "#{b.x} #{b.y}"
end
"##;

    let diags = typecheck_diagnostics_from_source(source);
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
    let source = r#"
@[derive(Copy, Clone)]
struct Person
  name: String
  age: Int
end

def main
  let _x = 0
end
"#;

    let diags = typecheck_diagnostics_from_source(source);
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
    let source = r#"
@[derive(Copy)]
struct Pair
  x: Int
  y: Int
end

def main
  let _x = 0
end
"#;

    let diags = typecheck_diagnostics_from_source(source);
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
    let source = r#"
@[derive(Copy, Clone)]
class Counter
  value: Int
end

def main
  let _x = 0
end
"#;

    let diags = typecheck_diagnostics_from_source(source);
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
    let source = r#"
@[derive(Eq)]
struct Id
  value: Int
end

def main
  let _x = 0
end
"#;

    let diags = typecheck_diagnostics_from_source(source);
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
    let source = r#"
@[derive(Default)]
enum Mode
  Read
  Write
  Append
end

def main
  let _x = 0
end
"#;

    let diags = typecheck_diagnostics_from_source(source);
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
    let source = r#"
@[derive(Ord, PartialEq)]
struct Version
  major: Int
  minor: Int
end

def main
  let _x = 0
end
"#;

    let diags = typecheck_diagnostics_from_source(source);
    assert!(
        has_code(&diags, "E0606"),
        "expected E0606 when deriving Ord without Eq + PartialOrd — \
         got diagnostics {:?}.",
        diags
    );
}

/// E0607: `@[derive(...)]` cannot be applied to a function. The
/// parser rejects the misplaced attribute, so the diagnostic surfaces
/// out of `Parser::parse` rather than out of typeck.
#[test]
fn derive_on_function_reports_e0607() {
    let source = r#"
@[derive(Debug)]
def helper
  1
end
"#;

    let diags = parse_errors_from_source(source);
    assert!(
        has_code(&diags, "E0607"),
        "expected E0607 when `@[derive(...)]` is applied to a `def` — \
         got diagnostics {:?}.",
        diags
    );
}

/// E0608: only the auto-synthesisable mixins (Debug, Clone, Copy,
/// PartialEq, Eq, Hash/Hashable, Default, Ord, PartialOrd) may appear
/// in `@[derive(...)]`. Anything else must be flagged.
#[test]
fn unknown_derive_trait_reports_e0608() {
    let source = r#"
@[derive(Serializable)]
struct Config
  name: String
end

def main
  let _x = 0
end
"#;

    let diags = typecheck_diagnostics_from_source(source);
    assert!(
        has_code(&diags, "E0608"),
        "expected E0608 when `@[derive(...)]` names a non-synthesisable \
         mixin — got diagnostics {:?}.",
        diags
    );
}

/// E0609: naming the same auto-synthesisable mixin twice in one
/// `@[derive(...)]` list is almost always a copy-paste mistake and
/// must be rejected.
#[test]
fn duplicate_derive_trait_reports_e0609() {
    let source = r#"
@[derive(Clone, Clone)]
struct Point
  x: Int
  y: Int
end

def main
  let _x = 0
end
"#;

    let diags = typecheck_diagnostics_from_source(source);
    assert!(
        has_code(&diags, "E0609"),
        "expected E0609 when an `@[derive(...)]` list repeats a trait \
         name — got diagnostics {:?}.",
        diags
    );
}
