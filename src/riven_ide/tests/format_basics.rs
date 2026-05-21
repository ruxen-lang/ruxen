//! Wave-1 LSP formatting — fixture-driven tests for
//! [`riven_ide::format::format_document`] and
//! [`riven_ide::format::format_range`].
//!
//! Per `feedback_no_inline_rvn_in_pin_tests.md`, every Riven source
//! lives under `tests/fixtures/format/`. We assert behaviour at the
//! edit-list level (no edits / one whole-document edit / `None`)
//! rather than pinning byte-exact formatted output — the canonical
//! formatter's whitespace decisions are the formatter crate's
//! contract, not ours.

use lsp_types::{Position, Range, TextEdit};
use riven_ide::format::{format_document, format_range};

/// Load a fixture by stem name (`<stem>.rvn` under `fixtures/format/`).
fn load(stem: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/format")
        .join(format!("{}.rvn", stem));
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

/// Apply a single full-document `TextEdit` to `source` and return the
/// resulting string. Panics if the edit's start isn't (0,0) — the
/// whole-document contract is what this LSP capability promises.
fn apply_whole_document_edit(source: &str, edits: &[TextEdit]) -> String {
    assert_eq!(edits.len(), 1, "expected exactly one edit, got {}", edits.len());
    let edit = &edits[0];
    assert_eq!(
        edit.range.start,
        Position { line: 0, character: 0 },
        "edit must start at (0,0): {:?}",
        edit.range
    );
    // We don't apply incrementally — the contract is "replace whole
    // document". The caller checks that the edit's end position
    // matches the source's end.
    let _ = source;
    edit.new_text.clone()
}

// ─── format_document ───────────────────────────────────────────────

#[test]
fn already_formatted_source_yields_empty_edit_list() {
    let source = load("already_canonical");
    let edits = format_document(&source).expect("canonical source must parse");
    assert!(
        edits.is_empty(),
        "expected no edits for already-formatted source, got {} edit(s): {:?}",
        edits.len(),
        edits
    );
}

#[test]
fn misformatted_source_emits_one_whole_document_edit() {
    let source = load("extra_blank_lines");
    let edits = format_document(&source).expect("parses fine");
    assert_eq!(edits.len(), 1, "expected a single whole-document edit");

    let edit = &edits[0];
    assert_eq!(edit.range.start, Position { line: 0, character: 0 });

    // The formatter compresses 3+ blank lines to 2, so the new text
    // must differ from the original AND must itself be already
    // canonical (idempotency).
    let formatted = apply_whole_document_edit(&source, &edits);
    assert_ne!(formatted, source, "formatter should have changed the text");

    // Idempotency: re-running the formatter on the output yields no
    // further edits.
    let second = format_document(&formatted).expect("output must reparse");
    assert!(
        second.is_empty(),
        "formatter output should be idempotent, got: {:?}",
        second
    );
}

#[test]
fn trailing_whitespace_is_stripped() {
    let source = load("trailing_whitespace");
    // Sanity: the fixture really does contain trailing spaces (a
    // tooling lint stripping them would silently invalidate this test).
    assert!(
        source.lines().any(|l| l.ends_with(' ')),
        "fixture lost its trailing whitespace — restore the literal spaces"
    );

    let edits = format_document(&source).expect("parses fine");
    assert_eq!(edits.len(), 1);
    let formatted = apply_whole_document_edit(&source, &edits);
    assert!(
        formatted.lines().all(|l| !l.ends_with(' ') && !l.ends_with('\t')),
        "formatted output should have no trailing whitespace:\n{:?}",
        formatted
    );
}

#[test]
fn parse_error_returns_none() {
    let source = load("parse_error");
    let edits = format_document(&source);
    assert!(
        edits.is_none(),
        "expected None for unparseable source, got {:?}",
        edits
    );
}

#[test]
fn comments_are_preserved_in_formatted_output() {
    let source = load("with_comments");
    // The fixture may or may not be already canonical — either is
    // fine. What matters is that, in the resulting buffer (whether
    // the original or the formatted output), the comment markers
    // survive.
    let formatted = match format_document(&source).expect("parses fine") {
        edits if edits.is_empty() => source.clone(),
        edits => apply_whole_document_edit(&source, &edits),
    };

    assert!(
        formatted.contains("## A doc comment on the anchor function"),
        "doc comment was lost:\n{}",
        formatted
    );
    assert!(
        formatted.contains("marker_keep_me_q1"),
        "doc-comment marker was lost:\n{}",
        formatted
    );
}

#[test]
fn edit_range_end_covers_whole_document() {
    // The whole-document TextEdit must span from (0,0) to the
    // last-line/last-column of the source — otherwise the client
    // would prepend the formatted text instead of replacing.
    let source = load("extra_blank_lines");
    let edits = format_document(&source).expect("parses fine");
    let edit = &edits[0];

    let last_newline = source.rfind('\n');
    let expected_end_line = source.matches('\n').count() as u32;
    let expected_end_char = match last_newline {
        Some(n) => source[n + 1..].chars().count() as u32,
        None => source.chars().count() as u32,
    };
    assert_eq!(
        edit.range.end,
        Position {
            line: expected_end_line,
            character: expected_end_char,
        },
        "edit end position does not cover the whole document"
    );
}

// ─── format_range ──────────────────────────────────────────────────

#[test]
fn range_formatting_on_a_single_block_replaces_whole_document() {
    // The underlying formatter is whole-program, so range formatting
    // falls back to a whole-document replacement. The test confirms
    // both halves of that contract: a single edit comes back, and
    // it's anchored at (0,0).
    let source = load("single_block");
    // Pick a range that covers only the inner function's body —
    // realistic IDE "format this selection" gesture.
    let range = Range {
        start: Position { line: 1, character: 0 },
        end: Position { line: 4, character: 3 },
    };
    let edits = format_range(&source, range).expect("parses fine");

    // The fixture is already canonical, so we accept either: no
    // edits, OR a single edit that re-emits the same canonical text.
    // Both are valid — what we forbid is a partial-range edit that
    // would silently corrupt the buffer.
    match edits.as_slice() {
        [] => {}
        [edit] => {
            assert_eq!(edit.range.start, Position { line: 0, character: 0 });
        }
        more => panic!("expected 0 or 1 edits, got {}: {:?}", more.len(), more),
    }
}

#[test]
fn range_formatting_propagates_parse_error_as_none() {
    let source = load("parse_error");
    let range = Range {
        start: Position { line: 0, character: 0 },
        end: Position { line: 2, character: 0 },
    };
    assert!(format_range(&source, range).is_none());
}
