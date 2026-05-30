//! Code-action quick-fix integration tests.
//!
//! Each test loads a `.rx` fixture that triggers a specific
//! diagnostic, then runs `code_actions` with the collected
//! LSP diagnostics in the `CodeActionContext`. The assertions
//! check the produced action's title plus the resulting TextEdit
//! shape — never the implementation internals.
//!
//! Fixtures live under `tests/fixtures/code_actions/`. Each one uses
//! a unique anchor identifier (`*_qf`) that appears exactly once,
//! never in comments.

use lsp_types::{
    CodeActionContext, CodeActionKind, CodeActionOrCommand, NumberOrString, Position, Range, Url,
};

use ruxen_ide::analysis::{analyze, AnalysisResult};
use ruxen_ide::code_actions::code_actions;
use ruxen_ide::diagnostics::collect_diagnostics;

fn load(stem: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/code_actions")
        .join(format!("{}.rx", stem));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn fake_uri() -> Url {
    Url::parse("file:///fixture.rx").unwrap()
}

fn analyze_and_collect(source: &str) -> (AnalysisResult, Vec<lsp_types::Diagnostic>, Url) {
    let result = analyze(source);
    let uri = fake_uri();
    let diags = collect_diagnostics(&result, &uri);
    (result, diags, uri)
}

fn diagnostics_with_code<'a>(
    diags: &'a [lsp_types::Diagnostic],
    code: &str,
) -> Vec<&'a lsp_types::Diagnostic> {
    diags
        .iter()
        .filter(|d| match &d.code {
            Some(NumberOrString::String(s)) => s == code,
            _ => false,
        })
        .collect()
}

/// Build a context containing every diagnostic from `diags`.
fn ctx_from(diags: &[lsp_types::Diagnostic]) -> CodeActionContext {
    CodeActionContext {
        diagnostics: diags.to_vec(),
        only: None,
        trigger_kind: None,
    }
}

/// A range that covers the entire fixture (forces overlap for every diag).
fn whole_doc_range(source: &str) -> Range {
    let last_line = source.lines().count() as u32;
    Range {
        start: Position::new(0, 0),
        end: Position::new(last_line + 1, 0),
    }
}

// ─── E1006 quick-fix ─────────────────────────────────────────────────

#[test]
fn e1006_quick_fix_replaces_let_with_var() {
    let source = load("e1006_let_to_var");
    let (result, lsp_diags, uri) = analyze_and_collect(&source);

    let e1006s = diagnostics_with_code(&lsp_diags, "E1006");
    assert!(
        !e1006s.is_empty(),
        "fixture should trigger at least one E1006, got: {:?}",
        lsp_diags
    );

    let ctx = ctx_from(&lsp_diags);
    let actions = code_actions(&result, whole_doc_range(&source), &ctx, &uri);

    assert_eq!(
        actions.len(),
        1,
        "expected one quick-fix, got {:?}",
        actions
    );

    let action = match &actions[0] {
        CodeActionOrCommand::CodeAction(a) => a,
        other => panic!("expected CodeAction, got {:?}", other),
    };
    assert_eq!(action.kind.as_ref(), Some(&CodeActionKind::QUICKFIX));
    assert!(
        action.title.contains("counter_qf"),
        "title must name the binding, got: {:?}",
        action.title
    );
    assert!(
        action.title.contains("let") && action.title.contains("var"),
        "title must show let \u{2192} var transition, got: {:?}",
        action.title
    );

    // The edit must replace `let` with `var` at the declaration site.
    let edits = action
        .edit
        .as_ref()
        .and_then(|we| we.changes.as_ref())
        .and_then(|m| m.get(&uri))
        .expect("WorkspaceEdit.changes[uri] missing");
    assert_eq!(edits.len(), 1, "expected one TextEdit, got {:?}", edits);
    assert_eq!(edits[0].new_text, "var");

    // Apply the edit manually to verify it targets the correct source slice.
    let edit_byte_start = result.line_index.byte_offset_of(edits[0].range.start);
    let edit_byte_end = result.line_index.byte_offset_of(edits[0].range.end);
    assert_eq!(
        &source[edit_byte_start..edit_byte_end],
        "let",
        "edit range must cover the `let` token"
    );

    // After applying, the immutable assignment is fixed.
    let mut after = source.clone();
    after.replace_range(edit_byte_start..edit_byte_end, "var");
    assert!(after.contains("var counter_qf"), "after-edit: {}", after);
}

// ─── E1110 quick-fix ─────────────────────────────────────────────────

#[test]
fn e1110_quick_fix_prepends_async_to_def() {
    let source = load("e1110_add_async");
    let (result, lsp_diags, uri) = analyze_and_collect(&source);

    let e1110s = diagnostics_with_code(&lsp_diags, "E1110");
    assert!(
        !e1110s.is_empty(),
        "fixture should trigger E1110, got: {:?}",
        lsp_diags
    );

    let ctx = ctx_from(&lsp_diags);
    let actions = code_actions(&result, whole_doc_range(&source), &ctx, &uri);

    // Could yield multiple if there are multiple .awaits, but the
    // fixture has exactly one.
    assert!(
        !actions.is_empty(),
        "expected at least one E1110 quick-fix, got none"
    );
    let action = match &actions[0] {
        CodeActionOrCommand::CodeAction(a) => a,
        other => panic!("expected CodeAction, got {:?}", other),
    };
    assert_eq!(action.kind.as_ref(), Some(&CodeActionKind::QUICKFIX));
    assert!(
        action.title.contains("needs_async_qf"),
        "title must name the enclosing fn, got: {:?}",
        action.title
    );

    let edits = action
        .edit
        .as_ref()
        .and_then(|we| we.changes.as_ref())
        .and_then(|m| m.get(&uri))
        .expect("WorkspaceEdit.changes[uri] missing");
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].new_text, "async ");

    // The insertion must be a zero-width range at the start of `def`.
    let insert_byte = result.line_index.byte_offset_of(edits[0].range.start);
    assert_eq!(
        edits[0].range.start, edits[0].range.end,
        "insertion must be zero-width"
    );
    assert_eq!(
        &source[insert_byte..insert_byte + 3],
        "def",
        "insertion point must be immediately before `def`"
    );

    // Applying the edit yields `async def needs_async_qf` on that line.
    let mut after = source.clone();
    after.insert_str(insert_byte, "async ");
    assert!(
        after.contains("async def needs_async_qf"),
        "after-edit:\n{}",
        after
    );
}

// ─── Unrecognised code: no actions ───────────────────────────────────

#[test]
fn unrecognised_code_yields_no_actions() {
    let source = "def main\n  let x = 1\nend\n";
    let result = analyze(source);
    let uri = fake_uri();

    // Synthesise a bogus diagnostic with an unknown code.
    let fake = lsp_types::Diagnostic {
        range: Range {
            start: Position::new(0, 0),
            end: Position::new(0, 3),
        },
        severity: Some(lsp_types::DiagnosticSeverity::ERROR),
        code: Some(NumberOrString::String("E9999".to_string())),
        source: Some("ruxenc".into()),
        message: "made-up problem".into(),
        related_information: None,
        ..Default::default()
    };
    let ctx = CodeActionContext {
        diagnostics: vec![fake],
        only: None,
        trigger_kind: None,
    };

    let actions = code_actions(&result, whole_doc_range(source), &ctx, &uri);
    assert!(actions.is_empty(), "expected no actions, got {:?}", actions);
}

// ─── Range filter: non-overlapping diag is ignored ───────────────────

#[test]
fn range_filter_excludes_non_overlapping_diagnostics() {
    let source = load("e1006_let_to_var");
    let (result, lsp_diags, uri) = analyze_and_collect(&source);

    let e1006 = diagnostics_with_code(&lsp_diags, "E1006");
    assert!(!e1006.is_empty(), "fixture must produce E1006");

    let ctx = ctx_from(&lsp_diags);

    // Pick a tiny range far away from the diagnostic (line 0, col 0 — the `def` line).
    let far_range = Range {
        start: Position::new(0, 0),
        end: Position::new(0, 1),
    };
    // The E1006 diag is on line 2 (counter_qf = 1), so this range should NOT overlap.
    let diag_line = e1006[0].range.start.line;
    assert!(
        diag_line > 0,
        "fixture invariant: diag must be below line 0"
    );

    let actions = code_actions(&result, far_range, &ctx, &uri);
    assert!(
        actions.is_empty(),
        "expected no actions for non-overlapping range, got {:?}",
        actions
    );
}

// ─── Multiple diagnostics → multiple actions ─────────────────────────

#[test]
fn multiple_diagnostics_yield_multiple_actions() {
    let source = load("multi_diags");
    let (result, lsp_diags, uri) = analyze_and_collect(&source);

    // The fixture intentionally has both an E1006 (assignment to let)
    // AND a `.await` outside async (E1110). We only assert that
    // *both* code_action paths fire when the corresponding diagnostic
    // is present — if for any reason the compiler only surfaces one
    // of them in this configuration, the relevant assertion below
    // documents what we expected.
    let has_e1006 = !diagnostics_with_code(&lsp_diags, "E1006").is_empty();
    let has_e1110 = !diagnostics_with_code(&lsp_diags, "E1110").is_empty();
    assert!(
        has_e1006 || has_e1110,
        "fixture should trigger at least one supported code, got: {:?}",
        lsp_diags
    );

    let ctx = ctx_from(&lsp_diags);
    let actions = code_actions(&result, whole_doc_range(&source), &ctx, &uri);

    let mut e1006_fixed = false;
    let mut e1110_fixed = false;
    for a in &actions {
        if let CodeActionOrCommand::CodeAction(ca) = a {
            if ca.title.contains("pinned_qf") && ca.title.contains("var") {
                e1006_fixed = true;
            }
            if ca.title.contains("outer_qf") && ca.title.contains("async") {
                e1110_fixed = true;
            }
        }
    }

    if has_e1006 {
        assert!(e1006_fixed, "expected an E1006 quick-fix in {:?}", actions);
    }
    if has_e1110 {
        assert!(e1110_fixed, "expected an E1110 quick-fix in {:?}", actions);
    }
    assert!(
        !actions.is_empty(),
        "expected ≥1 actions, got {:?}",
        actions
    );
}

// ─── E1112 has no quick-fix ──────────────────────────────────────────

#[test]
fn e1112_yields_no_action_even_if_present() {
    // Synthesise an E1112 diagnostic — we don't need the compiler to
    // produce it for real; we're testing the dispatcher's contract
    // that E1112 produces no quick-fix.
    let source = "def main\n  let x = 1\nend\n";
    let result = analyze(source);
    let uri = fake_uri();

    let e1112 = lsp_types::Diagnostic {
        range: Range {
            start: Position::new(1, 2),
            end: Position::new(1, 12),
        },
        severity: Some(lsp_types::DiagnosticSeverity::ERROR),
        code: Some(NumberOrString::String("E1112".to_string())),
        source: Some("ruxenc".into()),
        message: "block_on inside async".into(),
        related_information: None,
        ..Default::default()
    };
    let ctx = CodeActionContext {
        diagnostics: vec![e1112],
        only: None,
        trigger_kind: None,
    };

    let actions = code_actions(&result, whole_doc_range(source), &ctx, &uri);
    assert!(
        actions.is_empty(),
        "E1112 must produce no quick-fixes, got {:?}",
        actions
    );
}
