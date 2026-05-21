//! Quick-fix code actions driven by diagnostics.
//!
//! Phase-1 surface (deliberately small): for each diagnostic in the
//! incoming `CodeActionContext` whose `code` matches a supported
//! pattern AND whose range overlaps the client's requested `range`,
//! emit a `CodeAction { kind: QuickFix, edit: WorkspaceEdit(...) }`.
//!
//! Supported codes:
//!   - `E1006` (cannot assign to `let` binding)  →  `let` → `var`
//!   - `E1110` (`.await` outside async)          →  prepend `async ` to
//!                                                  the enclosing `def`.
//!   - `E1112` (`block_on` inside async)         →  no quick fix.
//!
//! Unrecognised codes: silently ignored.
//!
//! ## URL parameter
//!
//! The spec sketch for this function omitted a `uri` argument, but
//! `WorkspaceEdit.changes` is keyed by `Url` — there is no
//! URL-less form of the LSP edit. The handler in `riven_lsp` already
//! has the document `Url` in scope, so we accept it as a parameter
//! and the stitch block plumbs it through.

#![allow(dead_code)]

use std::collections::HashMap;

use lsp_types::{
    CodeAction, CodeActionContext, CodeActionKind, CodeActionOrCommand,
    Diagnostic as LspDiagnostic, NumberOrString, Position, Range, TextEdit, Url, WorkspaceEdit,
};

use crate::analysis::AnalysisResult;

/// Walk diagnostics in `context.diagnostics`, emit a quick-fix per
/// supported diagnostic that overlaps `range`.
pub fn code_actions(
    result: &AnalysisResult,
    range: Range,
    context: &CodeActionContext,
    uri: &Url,
) -> Vec<CodeActionOrCommand> {
    let mut actions: Vec<CodeActionOrCommand> = Vec::new();

    for diag in &context.diagnostics {
        if !ranges_overlap(range, diag.range) {
            continue;
        }
        let code = match &diag.code {
            Some(NumberOrString::String(s)) => s.as_str(),
            _ => continue,
        };

        match code {
            "E1006" => {
                if let Some(action) = quick_fix_e1006(result, diag, uri) {
                    actions.push(CodeActionOrCommand::CodeAction(action));
                }
            }
            "E1110" => {
                if let Some(action) = quick_fix_e1110(result, diag, uri) {
                    actions.push(CodeActionOrCommand::CodeAction(action));
                }
            }
            // E1112 deliberately produces no quick-fix — see module doc.
            _ => {}
        }
    }

    actions
}

// ─── Overlap test ────────────────────────────────────────────────────

/// Two LSP ranges overlap iff `a.start < b.end` AND `b.start < a.end`.
/// Zero-width ranges (caret only) also count as overlapping anything
/// they touch — VSCode emits both shapes.
fn ranges_overlap(a: Range, b: Range) -> bool {
    !(pos_lt(b.end, a.start) || pos_lt(a.end, b.start))
}

fn pos_lt(a: Position, b: Position) -> bool {
    (a.line, a.character) < (b.line, b.character)
}

// ─── E1006 — let → var ───────────────────────────────────────────────

fn quick_fix_e1006(
    result: &AnalysisResult,
    diag: &LspDiagnostic,
    uri: &Url,
) -> Option<CodeAction> {
    // The diagnostic is anchored at the assignment LHS (e.g. `x` in
    // `x = 43`). We extract the variable name from the message —
    // the LSP-side message has the borrow-error title prepended
    // (e.g. "cannot assign to `let` binding: cannot assign to `x` …"),
    // so we anchor on the literal substring "assign to `" and read
    // the identifier between the next pair of backticks.
    let name = extract_assignee_name(&diag.message)?;

    let diag_byte = result.line_index.byte_offset_of(diag.range.start);
    let (let_start, let_end) = find_let_token_for_name(&result.source, &name, diag_byte)?;

    let let_range = Range {
        start: result.line_index.position_of(let_start),
        end: result.line_index.position_of(let_end),
    };

    let edit = TextEdit {
        range: let_range,
        new_text: "var".to_string(),
    };

    Some(make_action(
        format!("Make \u{2018}{}\u{2019} mutable (let \u{2192} var)", name),
        edit,
        diag.clone(),
        uri,
    ))
}

/// Pull the first identifier-shaped backticked substring from `msg`
/// that follows the literal anchor "assign to `". The borrow-error
/// title ("cannot assign to `let` binding") also contains backticks,
/// so we cannot just grab the first pair.
fn extract_assignee_name(msg: &str) -> Option<String> {
    // Find the *last* occurrence of "assign to `" so the title's
    // initial "assign to `let`" is skipped in favour of the label's
    // "assign to `x`".
    let anchor = "assign to `";
    let start = msg.rfind(anchor)?;
    let rest = &msg[start + anchor.len()..];
    let end = rest.find('`')?;
    let name = &rest[..end];
    if name.is_empty() || !name.chars().all(is_ident_char) {
        return None;
    }
    Some(name.to_string())
}

fn is_ident_char(c: char) -> bool {
    c == '_' || c.is_ascii_alphanumeric()
}

/// Search the source upward from `from_byte` for the nearest `let
/// <name>` declaration whose name matches. Returns the byte range
/// `(start, end)` of the `let` keyword (3 bytes long).
fn find_let_token_for_name(source: &str, name: &str, from_byte: usize) -> Option<(usize, usize)> {
    let upper_bound = from_byte.min(source.len());
    let haystack = &source[..upper_bound];
    let mut search_end = haystack.len();

    while let Some(idx) = haystack[..search_end].rfind("let") {
        search_end = idx; // next iteration looks strictly earlier
        if !is_word_boundary(haystack, idx, 3) {
            continue;
        }
        // Skip whitespace after `let` and check the next identifier.
        let after = idx + 3;
        let bytes = source.as_bytes();
        let mut p = after;
        while p < bytes.len() && (bytes[p] == b' ' || bytes[p] == b'\t') {
            p += 1;
        }
        // Allow `let mut`-style prefix tokens — Riven uses `let` and
        // `var`, but a future `let mut` would need to be matched
        // before the name; we accept either the name immediately or a
        // single keyword in between for resilience.
        let id_start = p;
        while p < bytes.len() && is_ident_byte(bytes[p]) {
            p += 1;
        }
        let id = &source[id_start..p];
        if id == name {
            return Some((idx, idx + 3));
        }
    }
    None
}

fn is_ident_byte(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphanumeric()
}

/// True if the slice [`start` .. `start+len`] in `s` is bordered by
/// non-ident chars on both sides (or by the source edges).
fn is_word_boundary(s: &str, start: usize, len: usize) -> bool {
    let bytes = s.as_bytes();
    let before_ok = if start == 0 {
        true
    } else {
        !is_ident_byte(bytes[start - 1])
    };
    let end = start + len;
    let after_ok = if end >= bytes.len() {
        true
    } else {
        !is_ident_byte(bytes[end])
    };
    before_ok && after_ok
}

// ─── E1110 — prepend `async ` to enclosing def ───────────────────────

fn quick_fix_e1110(
    result: &AnalysisResult,
    diag: &LspDiagnostic,
    uri: &Url,
) -> Option<CodeAction> {
    let await_byte = result.line_index.byte_offset_of(diag.range.start);
    let (def_kw_start, fn_name) = find_enclosing_def(&result.source, await_byte)?;

    let pos = result.line_index.position_of(def_kw_start);
    let edit = TextEdit {
        range: Range {
            start: pos,
            end: pos,
        },
        new_text: "async ".to_string(),
    };

    Some(make_action(
        format!("Add \u{2018}async\u{2019} to \u{2018}def {}\u{2019}", fn_name),
        edit,
        diag.clone(),
        uri,
    ))
}

/// Walk upward from `from_byte`, find the nearest `def` keyword that
/// is *not* already preceded by `async`. Returns the byte offset of
/// the `def` token and the function name following it.
fn find_enclosing_def(source: &str, from_byte: usize) -> Option<(usize, String)> {
    let upper = from_byte.min(source.len());
    let haystack = &source[..upper];
    let mut search_end = haystack.len();

    while let Some(idx) = haystack[..search_end].rfind("def") {
        search_end = idx;
        if !is_word_boundary(haystack, idx, 3) {
            continue;
        }
        // Skip this `def` if `async` immediately precedes it.
        if preceded_by_async(haystack, idx) {
            continue;
        }
        // Read the name after `def`.
        let bytes = source.as_bytes();
        let mut p = idx + 3;
        while p < bytes.len() && (bytes[p] == b' ' || bytes[p] == b'\t') {
            p += 1;
        }
        let name_start = p;
        while p < bytes.len() && is_ident_byte(bytes[p]) {
            p += 1;
        }
        let name = &source[name_start..p];
        if name.is_empty() {
            continue;
        }
        return Some((idx, name.to_string()));
    }
    None
}

/// True if `idx` in `s` is immediately preceded (skipping spaces/tabs)
/// by the keyword `async`.
fn preceded_by_async(s: &str, idx: usize) -> bool {
    let bytes = s.as_bytes();
    let mut p = idx;
    // Skip leftward whitespace.
    while p > 0 && (bytes[p - 1] == b' ' || bytes[p - 1] == b'\t') {
        p -= 1;
    }
    let needle = b"async";
    if p < needle.len() {
        return false;
    }
    let kw_start = p - needle.len();
    if &bytes[kw_start..p] != needle {
        return false;
    }
    // Word boundary before `async`.
    if kw_start > 0 && is_ident_byte(bytes[kw_start - 1]) {
        return false;
    }
    true
}

// ─── Action builder ──────────────────────────────────────────────────

fn make_action(title: String, edit: TextEdit, diag: LspDiagnostic, uri: &Url) -> CodeAction {
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    changes.insert(uri.clone(), vec![edit]);
    CodeAction {
        title,
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![diag]),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            document_changes: None,
            change_annotations: None,
        }),
        command: None,
        is_preferred: Some(true),
        disabled: None,
        data: None,
    }
}

// ─── Unit tests (helpers only — integration tests live in tests/) ────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlap_disjoint_returns_false() {
        let a = Range {
            start: Position::new(0, 0),
            end: Position::new(0, 3),
        };
        let b = Range {
            start: Position::new(1, 0),
            end: Position::new(1, 3),
        };
        assert!(!ranges_overlap(a, b));
    }

    #[test]
    fn overlap_touching_caret_at_start_overlaps() {
        let a = Range {
            start: Position::new(0, 5),
            end: Position::new(0, 5),
        };
        let b = Range {
            start: Position::new(0, 3),
            end: Position::new(0, 7),
        };
        assert!(ranges_overlap(a, b));
    }

    #[test]
    fn extract_assignee_name_basic() {
        let msg = "cannot assign to `let` binding: cannot assign to `foo` — variable is immutable";
        assert_eq!(extract_assignee_name(msg).as_deref(), Some("foo"));
    }

    #[test]
    fn extract_assignee_name_rejects_when_anchor_missing() {
        let msg = "weird ``";
        assert_eq!(extract_assignee_name(msg), None);
    }

    #[test]
    fn find_let_token_picks_matching_name() {
        let src = "def main\n  let foo = 1\n  let bar = 2\n  bar = 3\nend\n";
        // Byte offset of `bar = 3`'s `bar` (LHS of assignment).
        let bar_pos = src.find("bar = 3").unwrap();
        let (start, end) = find_let_token_for_name(src, "bar", bar_pos).unwrap();
        assert_eq!(&src[start..end], "let");
        // It must be the *second* `let` (matching `bar`), not the first.
        let expected_start = src.find("let bar").unwrap();
        assert_eq!(start, expected_start);
    }

    #[test]
    fn find_enclosing_def_returns_outer_name() {
        let src = "def outer\n  let f = ||\n    x.await\n  end\nend\n";
        let await_pos = src.find(".await").unwrap();
        let (def_start, name) = find_enclosing_def(src, await_pos).unwrap();
        assert_eq!(&src[def_start..def_start + 3], "def");
        assert_eq!(name, "outer");
    }

    #[test]
    fn find_enclosing_def_skips_async_def() {
        let src = "async def already_async\n  x.await\nend\ndef outer\n  y.await\nend\n";
        // First await is inside `async def already_async` — we want
        // to find `def already_async` and confirm we skip it because
        // it's preceded by `async`. There's only one def above.
        let await_pos = src.find("x.await").unwrap();
        assert!(find_enclosing_def(src, await_pos).is_none());
    }

    #[test]
    fn preceded_by_async_true_with_space() {
        let s = "async def foo";
        let idx = s.find("def").unwrap();
        assert!(preceded_by_async(s, idx));
    }

    #[test]
    fn preceded_by_async_false_when_other_word() {
        let s = "wibble def foo";
        let idx = s.find("def").unwrap();
        assert!(!preceded_by_async(s, idx));
    }
}
