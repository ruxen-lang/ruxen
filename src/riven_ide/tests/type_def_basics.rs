//! Phase 1B LSP `textDocument/typeDefinition` — fixture tests for the
//! six core shapes the v1 implementation supports.
//!
//! Per `feedback_no_inline_rvn_in_pin_tests.md`, every Riven source
//! lives in a `.rvn` file under `tests/fixtures/type_def/`. The cursor
//! is located by string-searching a unique **anchor identifier** that
//! appears EXACTLY once in the fixture (never repeated in comments —
//! that would land the cursor in the comment instead of in code).

use riven_ide::analysis::{analyze, AnalysisResult};
use riven_ide::type_def::type_definition;

/// Load `<stem>.rvn` from `tests/fixtures/type_def/`.
fn load(stem: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/type_def")
        .join(format!("{}.rvn", stem));
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

/// Compute the cursor byte offset at the first occurrence of `anchor`.
fn cursor_at(source: &str, anchor: &str, within: usize) -> usize {
    let start = source
        .find(anchor)
        .unwrap_or_else(|| panic!("anchor `{}` not found in fixture", anchor));
    start + within
}

fn type_def_at(
    source: &str,
    byte_offset: usize,
) -> (AnalysisResult, Option<lsp_types::Location>) {
    let result = analyze(source);
    let position = result.line_index.position_of(byte_offset);
    let loc = type_definition(&result, position);
    (result, loc)
}

/// Return the (line, byte_offset) at which `needle` first appears.
fn line_of(source: &str, needle: &str) -> u32 {
    let off = source
        .find(needle)
        .unwrap_or_else(|| panic!("needle `{}` not found", needle));
    source[..off].matches('\n').count() as u32
}

#[test]
fn local_of_class_type_jumps_to_class_decl() {
    let source = load("local_of_class_type");
    // Cursor sits one byte into the *reference* to the local — the
    // second occurrence of `cdef_anchor_local` ("let _ = …"). The
    // anchor appears in two places (the let binding + the
    // reference), so we search for the reference-site form.
    let needle = "= cdef_anchor_local";
    let cursor = cursor_at(&source, needle, "= ".len() + 1);
    let (_, loc) = type_def_at(&source, cursor);
    let loc = loc.expect("expected a location for `cdef_anchor_local`'s type");
    assert_eq!(
        loc.range.start.line,
        line_of(&source, "class CounterTypeDefAnchor"),
        "expected to land on the `class CounterTypeDefAnchor` line"
    );
}

#[test]
fn method_call_return_type_jumps_to_return_class() {
    let source = load("method_call_return");
    // The cursor is on the call expression `make_widget_anchor_fn`
    // (the second occurrence — the first is its own def, which would
    // route to `Definition` rather than `FnCall`).
    let needle = "= make_widget_anchor_fn";
    let cursor = cursor_at(&source, needle, "= ".len() + 1);
    let (_, loc) = type_def_at(&source, cursor);
    let loc = loc.expect("expected a location for the call's return type");
    assert_eq!(
        loc.range.start.line,
        line_of(&source, "class WidgetReturnAnchor"),
        "method-call return type should land on `class WidgetReturnAnchor`"
    );
}

#[test]
fn primitive_type_returns_none() {
    let source = load("primitive_int");
    let needle = "= primitive_anchor_local";
    let cursor = cursor_at(&source, needle, "= ".len() + 1);
    let (_, loc) = type_def_at(&source, cursor);
    assert!(
        loc.is_none(),
        "primitive Int has no source decl — expected None, got {:?}",
        loc
    );
}

#[test]
fn reference_type_peels_to_target_class() {
    let source = load("ref_peel");
    // Cursor on the `bparam_anchor` parameter inside the body of
    // `borrow_consumer`. There's only one occurrence of that anchor.
    let cursor = cursor_at(&source, "bparam_anchor", 2);
    let (_, loc) = type_def_at(&source, cursor);
    let loc = loc.expect("expected location for &BorrowTargetAnchor (peeled)");
    assert_eq!(
        loc.range.start.line,
        line_of(&source, "class BorrowTargetAnchor"),
        "`&BorrowTargetAnchor` should peel to `class BorrowTargetAnchor`"
    );
}

#[test]
fn field_declaration_type_jumps_to_field_type_class() {
    let source = load("field_type");
    // Cursor on the field name `field_anchor_inside` inside the
    // class body — the cursor sits on the field definition node;
    // its type should resolve to `InnerHolderAnchor`.
    let cursor = cursor_at(&source, "field_anchor_inside", 1);
    let (_, loc) = type_def_at(&source, cursor);
    let loc = loc.expect("expected location for field's declared type");
    assert_eq!(
        loc.range.start.line,
        line_of(&source, "class InnerHolderAnchor"),
        "field declared `InnerHolderAnchor` should land on its class"
    );
}

#[test]
fn field_access_expression_jumps_to_field_type_class() {
    let source = load("field_access");
    // Cursor inside `fa_field_anchor` on the access site
    // (`fa_outer_anchor_var.fa_field_anchor`).
    let cursor = cursor_at(&source, ".fa_field_anchor", 2);
    let (_, loc) = type_def_at(&source, cursor);
    let loc = loc.expect("expected location for field-access type");
    assert_eq!(
        loc.range.start.line,
        line_of(&source, "class FaInnerAnchor"),
        "field-access `.fa_field_anchor` of type FaInnerAnchor should land on its class"
    );
}

#[test]
fn generic_class_local_lands_on_generic_decl() {
    let source = load("generic_peel");
    let needle = "= gen_anchor_local";
    let cursor = cursor_at(&source, needle, "= ".len() + 1);
    let (_, loc) = type_def_at(&source, cursor);
    let loc = loc.expect("expected location for `Holder[T]` local — peel generic args");
    assert_eq!(
        loc.range.start.line,
        line_of(&source, "class GenericHolderAnchor"),
        "`GenericHolderAnchor[T]` should land on the class declaration"
    );
}

#[test]
fn cursor_on_empty_source_returns_none() {
    let source = "";
    let (_, loc) = type_def_at(source, 0);
    assert!(loc.is_none());
}
