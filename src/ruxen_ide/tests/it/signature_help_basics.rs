//! Phase 1D LSP signature help — fixture tests for the
//! `signature_help` capability declared in
//! `docs/requirements/tier3_01_lsp.md` §5.1 + §5.4 paragraph 4.
//!
//! Per `feedback_no_inline_rx_in_pin_tests.md`, every Ruxen source
//! lives in a `.rx` fixture under `tests/fixtures/signature_help/`.
//! Each fixture contains a unique anchor identifier that appears
//! exactly once — never in comments — so the cursor offset can be
//! recovered via `source.find(anchor)`. The cursor sits at the START
//! of the anchor (i.e. immediately before the anchor's first byte),
//! which models the user typing into the position just before the
//! placeholder.

use lsp_types::{ParameterLabel, SignatureHelp};
use ruxen_ide::analysis::analyze;
use ruxen_ide::signature_help::signature_help;

/// Read a fixture by stem name under `tests/fixtures/signature_help/`.
fn load(stem: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/signature_help")
        .join(format!("{}.rx", stem));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

/// Find the byte offset of the anchor; cursor lives at the start of it.
fn cursor_at_anchor(source: &str, anchor: &str) -> usize {
    source
        .find(anchor)
        .unwrap_or_else(|| panic!("anchor `{}` not found in fixture", anchor))
}

/// Run analysis + signature_help at the anchor.
fn help_at_anchor(source: &str, anchor: &str) -> Option<SignatureHelp> {
    let result = analyze(source);
    let cursor = cursor_at_anchor(source, anchor);
    let position = result.line_index.position_of(cursor);
    signature_help(&result, position)
}

// ─── Free function — first argument ────────────────────────────────

#[test]
fn fn_call_first_arg_returns_signature_with_active_param_zero() {
    let source = load("fn_first_arg");
    let help = help_at_anchor(&source, "cursor_first_arg_anchor")
        .expect("expected SignatureHelp inside add(...)");
    assert_eq!(help.signatures.len(), 1, "exactly one signature");
    let sig = &help.signatures[0];
    assert!(
        sig.label.contains("add(a: Int, b: Int)"),
        "label should describe `add` — got {:?}",
        sig.label
    );
    assert!(
        sig.label.contains("-> Int"),
        "label should include return type"
    );
    assert_eq!(
        sig.active_parameter,
        Some(0),
        "cursor in the first arg — active_parameter must be 0"
    );
    assert_eq!(help.active_parameter, Some(0));
    let params = sig.parameters.as_ref().expect("parameters present");
    assert_eq!(params.len(), 2, "two declared params");
}

// ─── Free function — second argument increments the index ─────────

#[test]
fn fn_call_second_arg_bumps_active_parameter() {
    let source = load("fn_second_arg");
    let help = help_at_anchor(&source, "cursor_second_arg_anchor")
        .expect("expected SignatureHelp inside add(1, ...)");
    let sig = &help.signatures[0];
    assert_eq!(
        sig.active_parameter,
        Some(1),
        "one comma before cursor — active_parameter must be 1"
    );
}

// ─── Method call ───────────────────────────────────────────────────

#[test]
fn method_call_resolves_method_signature() {
    let source = load("method_call");
    let help = help_at_anchor(&source, "cursor_method_second_anchor")
        .expect("expected SignatureHelp inside c.scale(...)");
    let sig = &help.signatures[0];
    assert!(
        sig.label.contains("scale(factor: Int, offset: Int)"),
        "label should describe the method `scale` — got {:?}",
        sig.label
    );
    assert_eq!(
        sig.active_parameter,
        Some(1),
        "cursor in the second arg (one comma before) — active_parameter must be 1"
    );
}

// ─── Outside any call ──────────────────────────────────────────────

#[test]
fn outside_any_call_returns_none() {
    let source = load("outside_any_call");
    let help = help_at_anchor(&source, "cursor_outside_anchor");
    assert!(
        help.is_none(),
        "cursor not inside a call — must yield None, got {:?}",
        help.map(|h| h.signatures[0].label.clone())
    );
}

// ─── Unresolved callee ─────────────────────────────────────────────

#[test]
fn unresolved_callee_returns_none() {
    let source = load("unresolved_callee");
    let help = help_at_anchor(&source, "cursor_unresolved_anchor");
    assert!(
        help.is_none(),
        "unresolved callee must yield None rather than guess a signature"
    );
}

// ─── Nested call — innermost wins ──────────────────────────────────

#[test]
fn nested_call_returns_innermost_signature() {
    let source = load("nested_call_inner_wins");
    let help = help_at_anchor(&source, "cursor_inner_nested_anchor")
        .expect("expected SignatureHelp from the inner call");
    let sig = &help.signatures[0];
    assert!(
        sig.label.contains("inner(seed: Int, scale: Int)"),
        "innermost call's signature must win — got {:?}",
        sig.label
    );
    assert!(
        !sig.label.contains("outer"),
        "must NOT return outer call's signature: {:?}",
        sig.label
    );
    assert_eq!(sig.active_parameter, Some(0));
}

// ─── Parameter label offsets are valid byte ranges ─────────────────

#[test]
fn parameter_label_offsets_point_inside_signature_label() {
    let source = load("fn_first_arg");
    let help = help_at_anchor(&source, "cursor_first_arg_anchor").expect("expected SignatureHelp");
    let sig = &help.signatures[0];
    let label = &sig.label;
    let params = sig.parameters.as_ref().expect("parameters present");
    for (i, p) in params.iter().enumerate() {
        match &p.label {
            ParameterLabel::LabelOffsets([start, end]) => {
                let s = *start as usize;
                let e = *end as usize;
                assert!(
                    s < e && e <= label.len(),
                    "param {} offsets out of range: [{}, {}] in label of len {}",
                    i,
                    s,
                    e,
                    label.len()
                );
                let slice = &label[s..e];
                assert!(
                    slice.contains(": Int"),
                    "param {} substring `{}` should look like `<name>: <ty>`",
                    i,
                    slice
                );
            }
            ParameterLabel::Simple(text) => {
                panic!(
                    "expected LabelOffsets for param {}, got Simple({:?})",
                    i, text
                );
            }
        }
    }
}
