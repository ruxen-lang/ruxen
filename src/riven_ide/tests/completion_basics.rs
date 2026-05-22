//! Phase 1B LSP completion — fixture tests across the two trigger
//! shapes we ship in v1 (word-start + after-dot).
//!
//! Per feedback_no_inline_rvn_in_pin_tests.md, every Riven source
//! lives in a `.rvn` fixture file under
//! `src/riven_ide/tests/fixtures/completion/`. The cursor position is
//! derived by string-searching a unique **anchor identifier** that
//! appears EXACTLY once in the fixture (never repeated in comments,
//! or the search lands on the comment). Each test names which
//! anchor it uses and what offset relative to the anchor it wants the
//! cursor at — typically the anchor's start position (cursor before
//! the anchor), or one byte past `g` for prefix-`g` tests.

use lsp_types::CompletionItemKind;
use riven_ide::analysis::{analyze, AnalysisResult};
use riven_ide::completion::completions;

/// Load a fixture by stem name (`<stem>.rvn` under `fixtures/completion/`).
fn load(stem: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/completion")
        .join(format!("{}.rvn", stem));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

/// Run analysis + completion at a cursor offset, returning the items.
fn complete_at(
    source: &str,
    byte_offset: usize,
) -> (AnalysisResult, Vec<lsp_types::CompletionItem>) {
    let result = analyze(source);
    let position = result.line_index.position_of(byte_offset);
    let items = completions(&result, position, None);
    (result, items)
}

fn labels(items: &[lsp_types::CompletionItem]) -> Vec<&str> {
    items.iter().map(|i| i.label.as_str()).collect()
}

/// Position the cursor at `source.find(anchor).unwrap() + offset_within`.
/// The anchor MUST appear exactly once in the fixture — `source.find`
/// returns the first occurrence, so an anchor mentioned in a comment
/// would land the cursor in the comment instead of the code.
fn cursor_in_anchor(source: &str, anchor: &str, offset_within: usize) -> usize {
    let start = source
        .find(anchor)
        .unwrap_or_else(|| panic!("anchor `{}` not found in fixture", anchor));
    start + offset_within
}

// ─── Word-start completion ─────────────────────────────────────────

#[test]
fn word_start_offers_top_level_function_names() {
    let source = load("word_start_top_level_fns");
    // Cursor sits one byte into `g_anchor_unique_xyz` — the user has
    // typed `g`.
    let cursor = cursor_in_anchor(&source, "g_anchor_unique_xyz", 1);
    let (_, items) = complete_at(&source, cursor);
    let names = labels(&items);
    assert!(names.contains(&"greet"), "expected `greet` in {:?}", names);
    assert!(
        names.contains(&"goodbye"),
        "expected `goodbye` in {:?}",
        names
    );
}

#[test]
fn word_start_offers_in_scope_locals_only() {
    let source = load("word_start_scope_locals");
    // Empty prefix (cursor at start of anchor) so the visibility
    // filter is what decides which locals surface, not name prefix.
    let cursor = cursor_in_anchor(&source, "cursor_anchor", 0);
    let (_, items) = complete_at(&source, cursor);
    let names = labels(&items);
    assert!(
        names.contains(&"before_local"),
        "expected `before_local` (declared above cursor) in {:?}",
        names
    );
    assert!(
        !names.contains(&"after_local"),
        "must NOT offer `after_local` (declared below cursor): {:?}",
        names
    );
}

#[test]
fn word_start_filters_by_prefix() {
    let source = load("word_start_prefix_filter");
    let cursor = cursor_in_anchor(&source, "b_cursor_anchor", 1);
    let (_, items) = complete_at(&source, cursor);
    let names = labels(&items);
    assert!(
        names.contains(&"beta"),
        "expected `beta` matching prefix `b`: {:?}",
        names
    );
}

#[test]
fn word_start_empty_prefix_offers_full_candidate_list() {
    let source = load("word_start_empty_prefix");
    // Cursor at the START of the anchor: empty prefix.
    let cursor = cursor_in_anchor(&source, "_empty_cursor_anchor", 0);
    let (_, items) = complete_at(&source, cursor);
    let names = labels(&items);
    assert!(names.contains(&"alpha"), "expected `alpha` in {:?}", names);
    assert!(names.contains(&"beta"), "expected `beta` in {:?}", names);
    assert!(
        items
            .iter()
            .any(|i| i.kind == Some(CompletionItemKind::KEYWORD)),
        "expected at least one keyword candidate"
    );
}

#[test]
fn word_start_classifies_function_kind() {
    let source = load("word_start_fn_kind");
    let cursor = cursor_in_anchor(&source, "h_cursor_anchor", 1);
    let (_, items) = complete_at(&source, cursor);
    let helper = items
        .iter()
        .find(|i| i.label == "helper")
        .expect("helper missing");
    assert_eq!(helper.kind, Some(CompletionItemKind::FUNCTION));
}

#[test]
fn word_start_classifies_class_kind() {
    let source = load("word_start_class_kind");
    let cursor = cursor_in_anchor(&source, "Co_cursor_anchor", 2);
    let (_, items) = complete_at(&source, cursor);
    let counter = items
        .iter()
        .find(|i| i.label == "Counter")
        .expect("Counter missing");
    assert_eq!(counter.kind, Some(CompletionItemKind::CLASS));
}

#[test]
fn word_start_skips_synth_classes() {
    let source = load("word_start_skips_synth");
    let cursor = cursor_in_anchor(&source, "__cursor_synth_anchor", 2);
    let (_, items) = complete_at(&source, cursor);
    let names = labels(&items);
    for label in &names {
        assert!(
            !label.starts_with("__"),
            "synth `__*` name must not be offered: {}",
            label
        );
    }
}

// ─── After-dot completion ──────────────────────────────────────────

#[test]
fn after_dot_offers_class_methods() {
    let source = load("after_dot_class_methods");
    let cursor = cursor_in_anchor(&source, "unique_member_anchor", 0);
    let (_, items) = complete_at(&source, cursor);
    let names = labels(&items);
    assert!(
        names.contains(&"get_value"),
        "expected method `get_value` on `c.`: {:?}",
        names
    );
    assert!(
        names.contains(&"doubled"),
        "expected method `doubled` on `c.`: {:?}",
        names
    );
}

#[test]
fn after_dot_offers_class_fields() {
    let source = load("after_dot_class_fields");
    let cursor = cursor_in_anchor(&source, "unique_member_anchor", 0);
    let (_, items) = complete_at(&source, cursor);
    let names = labels(&items);
    assert!(
        names.contains(&"value"),
        "expected field `value` on `c.`: {:?}",
        names
    );
    let value = items
        .iter()
        .find(|i| i.label == "value")
        .expect("value field missing");
    assert_eq!(value.kind, Some(CompletionItemKind::FIELD));
}

#[test]
fn after_dot_returns_empty_for_unresolved_receiver() {
    let source = load("after_dot_unresolved");
    let cursor = cursor_in_anchor(&source, "unique_member_anchor", 0);
    let (_, items) = complete_at(&source, cursor);
    assert!(
        items.is_empty(),
        "unresolved receiver should yield no completions, got: {:?}",
        labels(&items)
    );
}
