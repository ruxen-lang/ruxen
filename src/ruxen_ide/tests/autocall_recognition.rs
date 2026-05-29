//! IDE/LSP recognition of type-directed auto-call of function references.
//! See `docs/superpowers/specs/2026-05-29-auto-call-fn-references-design.md`.
//!
//! The IDE analysis pipeline (`analyze`) shares `typeck::type_check`, so the
//! auto-call transform must surface in editor features: a bare nullary fn
//! reference is treated as its RESULT type (so the inferred-type inlay hint
//! shows the result, and no false "no field on Fn" diagnostic appears), and a
//! bare reference to a fn that needs arguments surfaces E0726.
//!
//! Fixtures live under `src/ruxen_ide/tests/fixtures/autocall/` (team rule
//! against inline `r#"..."#` Ruxen source in `.rs` pin tests).

use lsp_types::{InlayHint, InlayHintKind, InlayHintLabel, Position, Range};
use ruxen_core::diagnostics::DiagnosticLevel;
use ruxen_ide::analysis::analyze;
use ruxen_ide::inlay_hints::{inlay_hints, InlayHintConfig};

fn load(stem: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/autocall")
        .join(format!("{stem}.rx"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn full_range() -> Range {
    Range {
        start: Position {
            line: 0,
            character: 0,
        },
        end: Position {
            line: u32::MAX,
            character: u32::MAX,
        },
    }
}

fn label(h: &InlayHint) -> &str {
    match &h.label {
        InlayHintLabel::String(s) => s.as_str(),
        InlayHintLabel::LabelParts(_) => "",
    }
}

/// A clean auto-call source (no dangling members) produces NO error
/// diagnostics in the editor — the false "no field on Fn" squiggle is gone
/// because `xs` binds the auto-called `Array[Int]` result.
#[test]
fn ide_no_false_diagnostic_on_autocall() {
    let result = analyze(&load("autocall_clean"));
    let errs: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(
        errs.is_empty(),
        "expected no diagnostics for clean auto-call source, got: {:?}",
        errs.iter()
            .map(|d| (&d.code, &d.message))
            .collect::<Vec<_>>()
    );
}

/// The inferred-type inlay hint for `let xs = get_items` (a nullary
/// `Fn() -> Array[Int]`) shows the auto-called RESULT type `Array[Int]`,
/// NOT `Fn() -> Array[Int]` — proving the editor recognises the auto-call.
#[test]
fn ide_inlay_hint_shows_autocalled_result_type() {
    let result = analyze(&load("autocall_clean"));
    let hints = inlay_hints(&result, full_range(), &InlayHintConfig::default());
    let type_hints: Vec<String> = hints
        .iter()
        .filter(|h| h.kind == Some(InlayHintKind::TYPE))
        .map(|h| label(h).to_string())
        .collect();
    let xs_hint = type_hints
        .iter()
        .find(|l| l.contains("Array"))
        .unwrap_or_else(|| panic!("expected an `Array` type hint for `xs`; got {type_hints:?}"));
    assert!(
        !xs_hint.contains("Fn"),
        "xs should infer the auto-called `Array[Int]`, not a `Fn` type; got {xs_hint:?}"
    );
}

/// A bare reference to a fn that needs arguments surfaces E0726 in the editor.
#[test]
fn ide_surfaces_e0726_for_arity_reference() {
    let result = analyze(&load("autocall_arity"));
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.code.as_deref() == Some("E0726")),
        "expected E0726 diagnostic in IDE analysis, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|d| (&d.code, &d.message))
            .collect::<Vec<_>>()
    );
}
