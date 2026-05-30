//! Phase 2 #06 (`std::fmt`) — surface tests.
//!
//! Locks in that `Display`, `Debug`, `Formatter`, and `FmtError` are
//! resolvable types/traits. Semantics (Display::fmt routing,
//! `"#{x:?}"` debug interpolation, Formatter buffering) are wired in
//! later phases; these tests guard the type-surface plumbing only.
//!
//! Ruxen's `class` is the Ruby-style entity that allows inline
//! `impl`/`def` blocks (see `parse_class_body` in `parser/mod.rs`);
//! `struct` is intentionally fields-only and rejects inline impl
//! blocks with a structured diagnostic (regression tests in
//! `parser/tests.rs::struct_with_impl_inside_*`). Fixtures here use
//! `class` accordingly.

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

fn type_errors(src: &str) -> Vec<Diagnostic> {
    let mut lexer = Lexer::new(src);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    result
        .diagnostics
        .into_iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect()
}

/// Phase A1: `Display` resolves as a built-in mixin, `Formatter` and
/// `FmtError` as built-in classes. A user-defined `class T ... include
/// Display ... end` parses and typechecks (the mixin method
/// `fmt(&self, &var Formatter) -> Result[(), FmtError]` is the Phase
/// A contract — Phase D wires the canonical interpolation dispatch).
#[test]
fn display_trait_and_formatter_are_resolvable() {
    let src = rx("display_trait_and_formatter_are_resolvable");
    let errs = type_errors(&src);
    assert!(
        errs.is_empty(),
        "expected no type errors, got: {:#?}",
        errs.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

/// Phase A1: `Debug` resolves as a built-in mixin with the same
/// `fmt(&var Formatter) -> Result[(), FmtError]` contract as
/// `Display`. A user-defined `include Debug` typechecks identically.
#[test]
fn debug_trait_is_resolvable_with_fmt_method() {
    let src = rx("debug_trait_is_resolvable_with_fmt_method");
    let errs = type_errors(&src);
    assert!(
        errs.is_empty(),
        "expected no type errors, got: {:#?}",
        errs.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

/// Phase 2 #06.B: format spec `"#{x:?}"` is captured at lex time.
/// This is a typecheck-level test — the spec must not break parse
/// or typecheck, even when applied to a struct with `derive Debug`.
/// MIR routing (Phase C) preserves existing behaviour: structs with
/// `derive Debug` already lower through `{Name}_to_debug`, so the
/// spec is currently a no-op semantically. Phase D will refactor
/// the canonical interp path through `Display::fmt`.
#[test]
fn debug_interpolation_spec_typechecks() {
    let src = rx("debug_interpolation_spec_typechecks");
    let errs = type_errors(&src);
    assert!(
        errs.is_empty(),
        "expected no type errors for `:?` on derive Debug, got: {:#?}",
        errs.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

/// Phase 2 #06.B: width/precision/align specs typecheck on numeric
/// types. Phase D will consume them via `Formatter`; for now the
/// spec is captured but not yet applied.
#[test]
fn width_and_precision_specs_typecheck() {
    let src = rx("width_and_precision_specs_typecheck");
    let errs = type_errors(&src);
    assert!(
        errs.is_empty(),
        "expected no type errors for width/precision specs, got: {:#?}",
        errs.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

/// Phase A3: `Formatter.write_str(&str) -> Result[(), FmtError]` is
/// callable from inside an `include Display`. Locks in the method
/// signature; Phase D wires the runtime semantics.
#[test]
fn formatter_write_str_returns_result_unit_fmt_error() {
    let src = rx("formatter_write_str_returns_result_unit_fmt_error");
    let errs = type_errors(&src);
    assert!(
        errs.is_empty(),
        "expected no type errors, got: {:#?}",
        errs.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}
