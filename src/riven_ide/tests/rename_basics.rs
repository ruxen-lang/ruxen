//! Wave-2 LSP rename — pin tests covering the contract laid out in
//! `docs/requirements/tier3_01_lsp.md` §5.13.
//!
//! Riven source lives in `.rvn` fixtures under `tests/fixtures/rename/`
//! (per `feedback_no_inline_rvn_in_pin_tests.md`) using unique anchor
//! identifiers that appear exactly the expected number of times in
//! the file and never inside comments — so we can locate each token
//! deterministically via plain `find`.

use lsp_types::{Position, Url};
use riven_ide::analysis::analyze;
use riven_ide::rename::{prepare_rename, rename};

// ─── Fixture loader ────────────────────────────────────────────────

fn load(stem: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/rename")
        .join(format!("{}.rvn", stem));
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn fake_uri() -> Url {
    Url::parse("file:///fixture.rvn").unwrap()
}

/// Locate the (`skip`+1)-th occurrence of `needle` in `src` and
/// return an LSP `Position` pointing at the second character of the
/// identifier (deep enough inside the token that any sane node-finder
/// lands on it).
fn pos_inside(src: &str, needle: &str, skip: usize) -> Position {
    let mut remaining = skip + 1;
    let mut cursor = 0usize;
    while remaining > 0 {
        let found = src[cursor..]
            .find(needle)
            .unwrap_or_else(|| panic!("needle `{}` (skip={}) not found", needle, skip));
        remaining -= 1;
        if remaining == 0 {
            // Land one byte into the identifier so we're unambiguously
            // inside the token, not on a delimiter.
            let byte_offset = cursor + found + 1.min(needle.len() - 1);
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
        cursor += found + needle.len();
    }
    unreachable!()
}

// ─── Tests ─────────────────────────────────────────────────────────

#[test]
fn prepare_rename_on_var_ref_returns_identifier_range() {
    let src = load("local_var");
    let result = analyze(&src);
    // Cursor inside the second `tally_anchor_kzx` (the first ref on
    // the `let a = …` line).
    let pos = pos_inside(&src, "tally_anchor_kzx", 1);
    let range = prepare_rename(&result, pos)
        .expect("prepare_rename should return Some range on a VarRef");
    // Range should be a single token width — the identifier is 16 chars
    // and entirely on line 2 (0-indexed).
    assert_eq!(range.start.line, range.end.line, "range spans a single line");
    let width = range.end.character - range.start.character;
    assert_eq!(
        width, "tally_anchor_kzx".len() as u32,
        "range width should equal identifier length, got {}",
        width
    );
}

#[test]
fn prepare_rename_on_def_name_returns_identifier_range() {
    let src = load("top_level_fn");
    let result = analyze(&src);
    // Cursor inside the `def helper_anchor_kzx` declaration name.
    let pos = pos_inside(&src, "helper_anchor_kzx", 0);
    let range = prepare_rename(&result, pos)
        .expect("prepare_rename should return Some range on a def name");
    let width = range.end.character - range.start.character;
    assert_eq!(
        width, "helper_anchor_kzx".len() as u32,
        "range width should equal def name length"
    );
}

#[test]
fn prepare_rename_on_whitespace_returns_none() {
    let src = load("local_var");
    let result = analyze(&src);
    // Position at the very start of the second line (column 0, before
    // the leading indent) — pure whitespace.
    let pos = Position {
        line: 1,
        character: 0,
    };
    let out = prepare_rename(&result, pos);
    assert!(
        out.is_none(),
        "prepare_rename on whitespace must return None, got {:?}",
        out
    );
}

#[test]
fn rename_local_variable_produces_text_edit_per_use() {
    let src = load("local_var");
    let result = analyze(&src);
    // Cursor on the second `tally_anchor_kzx` (a use in `let a = …`)
    let pos = pos_inside(&src, "tally_anchor_kzx", 1);
    let edit = rename(&result, &fake_uri(), pos, "renamed_kzx")
        .expect("rename should succeed on a local");
    let changes = edit.changes.expect("WorkspaceEdit must have `changes`");
    let edits = changes.get(&fake_uri()).expect("edits for the fixture URI");
    // 1 decl + 3 use sites = 4 textual occurrences. Each TextEdit
    // replaces a single identifier-width slice with `renamed_kzx`.
    assert_eq!(
        edits.len(),
        4,
        "expected 4 edits (1 decl + 3 uses), got: {:?}",
        edits
    );
    for e in edits {
        assert_eq!(e.new_text, "renamed_kzx");
    }
}

#[test]
fn rename_top_level_fn_produces_edits_for_def_and_calls() {
    let src = load("top_level_fn");
    let result = analyze(&src);
    let pos = pos_inside(&src, "helper_anchor_kzx", 0);
    let edit = rename(&result, &fake_uri(), pos, "renamed_kzx")
        .expect("rename on a top-level fn def must succeed");
    let edits = edit
        .changes
        .as_ref()
        .and_then(|c| c.get(&fake_uri()))
        .expect("edits for the fixture URI");
    // 1 def + 2 call sites
    assert_eq!(
        edits.len(),
        3,
        "expected 3 edits (def + 2 calls), got: {:?}",
        edits
    );
}

#[test]
fn rename_rejects_riven_keyword_as_new_name() {
    let src = load("local_var");
    let result = analyze(&src);
    let pos = pos_inside(&src, "tally_anchor_kzx", 1);
    let out = rename(&result, &fake_uri(), pos, "let");
    assert!(out.is_none(), "rename to keyword `let` must return None");
    let out = rename(&result, &fake_uri(), pos, "match");
    assert!(out.is_none(), "rename to keyword `match` must return None");
}

#[test]
fn rename_rejects_invalid_identifier_shapes() {
    let src = load("local_var");
    let result = analyze(&src);
    let pos = pos_inside(&src, "tally_anchor_kzx", 1);
    for bad in ["123name", "with space", "has-dash", "", "name!"] {
        assert!(
            rename(&result, &fake_uri(), pos, bad).is_none(),
            "rename to `{}` must return None",
            bad
        );
    }
}

#[test]
fn rename_rejects_pascal_case_for_value_binding() {
    let src = load("local_var");
    let result = analyze(&src);
    let pos = pos_inside(&src, "tally_anchor_kzx", 1);
    let out = rename(&result, &fake_uri(), pos, "WrongCase");
    assert!(
        out.is_none(),
        "renaming a local (value binding) to PascalCase must return None"
    );
}

#[test]
fn rename_rejects_snake_case_for_type_binding() {
    let src = load("class_def");
    let result = analyze(&src);
    let pos = pos_inside(&src, "WidgetAnchorKzx", 0);
    let out = rename(&result, &fake_uri(), pos, "wrong_case");
    assert!(
        out.is_none(),
        "renaming a class (type binding) to snake_case must return None"
    );
}

#[test]
fn rename_rejects_double_underscore_new_name() {
    let src = load("local_var");
    let result = analyze(&src);
    let pos = pos_inside(&src, "tally_anchor_kzx", 1);
    let out = rename(&result, &fake_uri(), pos, "__hidden");
    assert!(
        out.is_none(),
        "rename to `__`-prefixed name must return None (compiler-internal namespace)"
    );
}

#[test]
fn rename_on_builtin_puts_returns_none() {
    // `puts` has a synthetic span (the bootstrap loader merges
    // `library/std/io/src/lib.rvn` decls but `analyze` doesn't track
    // their source per-file). Rename must refuse — see module docs.
    let src = load("builtin_use");
    let result = analyze(&src);
    let pos = pos_inside(&src, "puts", 0);
    let out = rename(&result, &fake_uri(), pos, "println");
    assert!(
        out.is_none(),
        "rename of a builtin must return None, got {:?}",
        out
    );
}

#[test]
fn rename_class_def_produces_edits_for_decl_and_ctor() {
    let src = load("class_def");
    let result = analyze(&src);
    let pos = pos_inside(&src, "WidgetAnchorKzx", 0);
    let edit = rename(&result, &fake_uri(), pos, "RenamedKzx")
        .expect("rename on a class def must succeed");
    let edits = edit
        .changes
        .as_ref()
        .and_then(|c| c.get(&fake_uri()))
        .expect("edits for fixture URI");
    // 1 class decl + 1 ctor use = at least 2 edits. The use_index may
    // record extra type-reference uses (e.g. through inferred types) —
    // we only assert the lower bound to stay robust.
    assert!(
        edits.len() >= 2,
        "expected at least decl + ctor (>= 2 edits), got: {:?}",
        edits
    );
    for e in edits {
        assert_eq!(e.new_text, "RenamedKzx");
    }
}
