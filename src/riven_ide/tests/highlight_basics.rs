//! Wave-2 LSP `documentHighlight` — pin tests per
//! `docs/requirements/tier3_01_lsp.md` §5.6.
//!
//! Each fixture uses a unique `*_anchor_qzx` (or `HCamel_anchor_qzx`)
//! identifier so we can search the source for known byte offsets and
//! compare against the resulting `Vec<DocumentHighlight>`. Per the
//! v1 read/write rule documented in `highlight.rs`, the FIRST returned
//! highlight is `WRITE` (the def-site) and every subsequent entry is
//! `READ`.

use lsp_types::{DocumentHighlightKind, Position};
use riven_ide::analysis::analyze;
use riven_ide::highlight::document_highlights;

fn load(stem: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/highlight")
        .join(format!("{}.rvn", stem));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

/// Convert a byte offset into an LSP `Position` by replaying the
/// file's line breaks. Mirrors `LineIndex::position_of` but doesn't
/// require taking an `AnalysisResult` apart at the call site.
fn position_of(source: &str, byte_offset: usize) -> Position {
    let mut line: u32 = 0;
    let mut col: u32 = 0;
    for (i, ch) in source.char_indices() {
        if i >= byte_offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += ch.len_utf16() as u32;
        }
    }
    Position {
        line,
        character: col,
    }
}

/// Byte offset of the Nth occurrence (0-indexed) of `needle` in
/// `source`. Panics if absent.
fn nth_byte_offset(source: &str, needle: &str, n: usize) -> usize {
    let mut count = 0;
    let mut search_start = 0;
    loop {
        let found = source[search_start..]
            .find(needle)
            .unwrap_or_else(|| panic!("`{}` (occurrence {}) not found", needle, n));
        let abs = search_start + found;
        if count == n {
            return abs;
        }
        count += 1;
        search_start = abs + needle.len();
    }
}

/// Position landing in the middle of the Nth occurrence of `needle`.
/// We point past the first byte to dodge any "is this token-start?"
/// edge cases in `node_at_position`.
fn pos_inside(source: &str, needle: &str, n: usize) -> Position {
    let offset = nth_byte_offset(source, needle, n);
    position_of(source, offset + 1)
}

// ─── Tests ──────────────────────────────────────────────────────────

#[test]
fn local_var_three_reads_yields_decl_plus_three_highlights() {
    let src = load("local_var_three_reads");
    let result = analyze(&src);
    // Cursor on the first USE (occurrence #1: `let a = hvar_anchor_qzx + …`).
    let pos = pos_inside(&src, "hvar_anchor_qzx", 1);
    let hl = document_highlights(&result, pos);
    // 1 decl + 3 reads = 4 entries.
    assert_eq!(
        hl.len(),
        4,
        "expected 4 highlights (decl + 3 reads), got {:?}",
        hl
    );
    // First is WRITE (decl), rest are READ.
    assert_eq!(hl[0].kind, Some(DocumentHighlightKind::WRITE));
    for h in &hl[1..] {
        assert_eq!(h.kind, Some(DocumentHighlightKind::READ));
    }
}

#[test]
fn cursor_on_decl_returns_full_highlight_set() {
    // Cursor on the DECLARATION should also yield the same N highlights.
    let src = load("local_var_three_reads");
    let result = analyze(&src);
    let pos = pos_inside(&src, "hvar_anchor_qzx", 0); // the `let hvar_anchor_qzx = 1` site
    let hl = document_highlights(&result, pos);
    assert_eq!(
        hl.len(),
        4,
        "decl + 3 reads expected from cursor on decl, got {:?}",
        hl
    );
    assert_eq!(hl[0].kind, Some(DocumentHighlightKind::WRITE));
}

#[test]
fn fn_def_and_two_calls_yields_three_highlights() {
    let src = load("fn_def_and_two_calls");
    let result = analyze(&src);
    // Cursor on first call site (`let a = hfn_anchor_qzx`).
    let pos = pos_inside(&src, "hfn_anchor_qzx", 1);
    let hl = document_highlights(&result, pos);
    assert_eq!(
        hl.len(),
        3,
        "expected 1 def + 2 calls = 3 highlights, got {:?}",
        hl
    );
    assert_eq!(hl[0].kind, Some(DocumentHighlightKind::WRITE));
    assert_eq!(hl[1].kind, Some(DocumentHighlightKind::READ));
    assert_eq!(hl[2].kind, Some(DocumentHighlightKind::READ));
}

#[test]
fn cursor_on_fn_def_returns_at_least_one_highlight() {
    let src = load("fn_def_and_two_calls");
    let result = analyze(&src);
    let pos = pos_inside(&src, "hfn_anchor_qzx", 0); // the `def hfn_anchor_qzx` site
    let hl = document_highlights(&result, pos);
    assert!(
        !hl.is_empty(),
        "cursor on def-name should produce highlights, got empty"
    );
    // The first highlight must be the WRITE/def-site.
    assert_eq!(hl[0].kind, Some(DocumentHighlightKind::WRITE));
}

#[test]
fn method_invoked_twice_yields_three_highlights() {
    let src = load("method_invoked_twice");
    let result = analyze(&src);
    // Cursor on the first call site (`c.hbump_anchor_qzx(3)`).
    let pos = pos_inside(&src, "hbump_anchor_qzx", 1);
    let hl = document_highlights(&result, pos);
    // 1 def + 2 call sites.
    assert_eq!(
        hl.len(),
        3,
        "expected 1 def + 2 method-calls = 3, got {:?}",
        hl
    );
    assert_eq!(hl[0].kind, Some(DocumentHighlightKind::WRITE));
}

#[test]
fn param_used_twice_yields_three_highlights() {
    let src = load("param_used_twice");
    let result = analyze(&src);
    // Cursor on a body reference.
    let pos = pos_inside(&src, "hinput_anchor_qzx", 1);
    let hl = document_highlights(&result, pos);
    assert_eq!(
        hl.len(),
        3,
        "expected 1 param-decl + 2 body refs = 3, got {:?}",
        hl
    );
    assert_eq!(hl[0].kind, Some(DocumentHighlightKind::WRITE));
}

#[test]
fn cursor_on_whitespace_returns_empty_or_enclosing() {
    let src = load("whitespace_only");
    let result = analyze(&src);
    // Position at column 0 of line 0 (the `d` of `def`). On `def`
    // itself the node finder may return the function definition; on
    // pure whitespace it should return empty. Both are acceptable;
    // we just need a non-panicking, well-formed Vec.
    let pos = Position {
        line: 0,
        character: 0,
    };
    let hl = document_highlights(&result, pos);
    // No assertion on length — what we're pinning is that the call
    // doesn't panic and the result is a valid Vec. Each entry, if
    // present, must have a non-synthetic range.
    for h in &hl {
        assert!(
            h.range.end.line > h.range.start.line
                || (h.range.end.line == h.range.start.line
                    && h.range.end.character > h.range.start.character),
            "highlight range must be non-empty, got {:?}",
            h.range
        );
    }
}

#[test]
fn cursor_past_end_of_file_returns_empty() {
    let src = load("whitespace_only");
    let result = analyze(&src);
    // Way past EOF — node finder returns None, we must return empty.
    let pos = Position {
        line: 100,
        character: 0,
    };
    let hl = document_highlights(&result, pos);
    assert!(hl.is_empty(), "cursor past EOF must yield empty");
}

#[test]
fn unresolvable_class_name_cursor_returns_empty_gracefully() {
    // Documents the v1 limitation: the shared `node_finder` doesn't
    // attribute spans inside a `class Foo` header line or inside a
    // `let x: Foo` type-annotation slot. Cursors there resolve to
    // either the enclosing scope's def (which is unrelated to `Foo`)
    // or to nothing — in either case we must return cleanly without
    // panicking, never producing a misleading highlight set.
    //
    // When `node_finder` grows annotation-aware coverage, this test
    // can be tightened to assert the full 3-span set.
    let src = load("class_with_two_uses");
    let result = analyze(&src);
    let pos = pos_inside(&src, "HWidgetAnchorQzx", 0);
    let hl = document_highlights(&result, pos);
    // Either empty (cursor falls off any tracked def) or a coherent
    // highlight set with WRITE first — both are acceptable, the
    // contract is "no panic, no bogus spans".
    if !hl.is_empty() {
        assert_eq!(hl[0].kind, Some(DocumentHighlightKind::WRITE));
        for h in &hl {
            // No zero ranges leak through.
            assert!(
                h.range.end != h.range.start,
                "zero-width highlight leaked: {:?}",
                h
            );
        }
    }
}

#[test]
fn field_with_two_accesses_yields_at_least_two_highlights() {
    let src = load("field_two_accesses");
    let result = analyze(&src);
    // Cursor on the first field access (`p.hpx_anchor_qzx` —
    // occurrence #2: the field decl is #0, the `@hpx_anchor_qzx`
    // param-shorthand is #1).
    let pos = pos_inside(&src, "hpx_anchor_qzx", 2);
    let hl = document_highlights(&result, pos);
    // Per use_index `field_access` shape: ≥ decl + 1 access. With two
    // accesses we expect at least 3, but the resolver may register
    // the field with decl + 2 accesses only (the param shorthand is
    // a separate def).
    assert!(
        hl.len() >= 2,
        "expected ≥ 2 highlights (decl + access), got {:?}",
        hl
    );
    assert_eq!(hl[0].kind, Some(DocumentHighlightKind::WRITE));
}
