//! Wave-2 LSP `textDocument/references` — pin tests covering the
//! contract in `docs/requirements/tier3_01_lsp.md` §5.6.
//!
//! Per `feedback_no_inline_rvn_in_pin_tests.md`, every Riven source
//! lives in a `.rvn` fixture under `tests/fixtures/references/`. Each
//! fixture uses a unique anchor identifier (`*_anchor_qzx`) that
//! appears exactly the expected number of times and never inside a
//! comment, so we can position the cursor deterministically.

use lsp_types::Position;
use riven_ide::analysis::analyze;
use riven_ide::references::references;

fn load(stem: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/references")
        .join(format!("{}.rvn", stem));
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

/// Return the `Position` (line, character) of the start of the `skip`th
/// occurrence of `needle` in `src` (0-indexed line, 0-indexed UTF-16
/// column). `skip == 0` is the first occurrence.
fn pos_of(src: &str, needle: &str, skip: usize) -> Position {
    let mut remaining = skip + 1;
    let mut search_start = 0;
    while remaining > 0 {
        let found = src[search_start..]
            .find(needle)
            .unwrap_or_else(|| panic!("needle `{}` not found (skip {})", needle, skip));
        remaining -= 1;
        if remaining == 0 {
            let byte_offset = search_start + found;
            let prefix = &src[..byte_offset];
            let line = prefix.matches('\n').count() as u32;
            let col = prefix
                .rfind('\n')
                .map(|i| prefix[i + 1..].chars().count())
                .unwrap_or_else(|| prefix.chars().count()) as u32;
            return Position {
                line,
                character: col,
            };
        }
        search_start += found + needle.len();
    }
    unreachable!()
}

/// Position just past the start of the identifier — safer than landing
/// on the first character, which sits exactly on a span boundary and
/// can be ambiguous for the byte-offset → position conversion.
fn pos_inside(src: &str, needle: &str, skip: usize) -> Position {
    let p = pos_of(src, needle, skip);
    Position {
        line: p.line,
        character: p.character + 1,
    }
}

// ─── Tests ──────────────────────────────────────────────────────────

#[test]
fn local_variable_three_uses_with_decl() {
    // Cursor on the 2nd occurrence (a use site, not the decl). With
    // `include_declaration = true` we get decl + 3 uses = 4 locations.
    let src = load("local_var_three_uses");
    let result = analyze(&src);
    let cursor = pos_inside(&src, "refs_anchor_qzx", 1);
    let locs = references(&result, cursor, true);
    assert_eq!(
        locs.len(),
        4,
        "expected decl + 3 uses, got {} locations: {:?}",
        locs.len(),
        locs
    );
}

#[test]
fn local_variable_three_uses_without_decl() {
    // Same fixture, same cursor, but `include_declaration = false`
    // drops the first entry (the decl), so we expect 3 use-sites.
    let src = load("local_var_three_uses");
    let result = analyze(&src);
    let cursor = pos_inside(&src, "refs_anchor_qzx", 1);
    let locs = references(&result, cursor, false);
    assert_eq!(
        locs.len(),
        3,
        "expected 3 uses (no decl), got {} locations: {:?}",
        locs.len(),
        locs
    );
}

#[test]
fn top_level_fn_two_calls_with_decl() {
    // Cursor on the first call-site of the function. With the decl
    // included we expect 1 decl + 2 calls = 3.
    let src = load("top_level_fn_two_calls");
    let result = analyze(&src);
    let cursor = pos_inside(&src, "shout_anchor_qzx", 1);
    let locs = references(&result, cursor, true);
    assert_eq!(
        locs.len(),
        3,
        "expected decl + 2 calls, got {} locations: {:?}",
        locs.len(),
        locs
    );
}

#[test]
fn top_level_fn_two_calls_without_decl() {
    let src = load("top_level_fn_two_calls");
    let result = analyze(&src);
    let cursor = pos_inside(&src, "shout_anchor_qzx", 1);
    let locs = references(&result, cursor, false);
    assert_eq!(
        locs.len(),
        2,
        "expected 2 calls (no decl), got {} locations: {:?}",
        locs.len(),
        locs
    );
}

#[test]
fn class_method_call_site_appears() {
    // Cursor on `t.tick_anchor_qzx(2)`. Even though typeck leaves the
    // MethodCall::method as UNRESOLVED_DEF, the receiver-type fallback
    // must recover the def and `references` must return at least the
    // decl + the one call site.
    let src = load("class_method_one_call");
    let result = analyze(&src);
    let cursor = pos_inside(&src, "tick_anchor_qzx", 1);
    let locs = references(&result, cursor, true);
    assert!(
        locs.len() >= 2,
        "expected at least decl + 1 method-call, got {} locations: {:?}",
        locs.len(),
        locs
    );
}

#[test]
fn cursor_on_declaration_returns_all_uses() {
    // Cursor directly on the `let decl_anchor_qzx` decl. With the
    // decl flag on we get 1 decl + 2 uses = 3. With it off, 2 uses.
    let src = load("cursor_on_decl");
    let result = analyze(&src);
    let cursor = pos_inside(&src, "decl_anchor_qzx", 0);

    let with_decl = references(&result, cursor, true);
    let without_decl = references(&result, cursor, false);

    assert_eq!(
        with_decl.len(),
        3,
        "decl + 2 uses, got {} locations: {:?}",
        with_decl.len(),
        with_decl
    );
    assert_eq!(
        without_decl.len(),
        2,
        "2 uses without decl, got {} locations: {:?}",
        without_decl.len(),
        without_decl
    );
}

#[test]
fn parameter_used_twice_in_body() {
    // Cursor on the param `value_anchor_qzx` in the signature. With
    // the decl, expect 1 param-decl + 2 body refs = 3.
    let src = load("param_used_twice");
    let result = analyze(&src);
    let cursor = pos_inside(&src, "value_anchor_qzx", 0);
    let locs = references(&result, cursor, true);
    assert_eq!(
        locs.len(),
        3,
        "expected param decl + 2 body refs, got {} locations: {:?}",
        locs.len(),
        locs
    );
}

#[test]
fn cursor_on_whitespace_returns_empty() {
    // Position at the start of an empty line — no node, no DefId. The
    // function must never panic and must return an empty Vec.
    let src = load("empty_main");
    let result = analyze(&src);
    let cursor = Position {
        line: 0,
        character: 0,
    };
    let locs = references(&result, cursor, true);
    assert!(
        locs.is_empty(),
        "expected empty Vec on whitespace, got {:?}",
        locs
    );
}

#[test]
fn cursor_past_end_of_file_returns_empty() {
    // Hard-defensive: line far past EOF. No panic, empty Vec.
    let src = load("empty_main");
    let result = analyze(&src);
    let cursor = Position {
        line: 9999,
        character: 0,
    };
    let locs = references(&result, cursor, true);
    assert!(locs.is_empty(), "expected empty Vec past EOF, got {:?}", locs);
}

#[test]
fn placeholder_uri_is_set_on_every_location() {
    // The LSP handler rewrites the URI to the document's real URI; this
    // contract test pins the placeholder so handler authors know what
    // to look for.
    let src = load("local_var_three_uses");
    let result = analyze(&src);
    let cursor = pos_inside(&src, "refs_anchor_qzx", 1);
    let locs = references(&result, cursor, true);
    assert!(!locs.is_empty());
    for loc in &locs {
        assert_eq!(
            loc.uri.as_str(),
            "file:///__placeholder__",
            "every location must carry the placeholder URI; got {}",
            loc.uri
        );
    }
}
