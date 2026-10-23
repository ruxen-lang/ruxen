//! Quick-fix code actions druxen by diagnostics.
//!
//! Phase-1 surface (deliberately small): for each diagnostic in the
//! incoming `CodeActionContext` whose `code` matches a supported
//! pattern AND whose range overlaps the client's requested `range`,
//! emit a `CodeAction { kind: QuickFix, edit: WorkspaceEdit(...) }`.
//!
//! Supported codes:
//!   - `E1006` (cannot assign to `let` binding)  →  `let` → `var`
//!   - `E1110` (`.await` outside async)          →  prepend `async ` to
//!     the enclosing `def`.
//!   - `E1112` (`block_on` inside async)         →  no quick fix.
//!
//! Unrecognised codes: silently ignored.
//!
//! ## URL parameter
//!
//! The spec sketch for this function omitted a `uri` argument, but
//! `WorkspaceEdit.changes` is keyed by `Url` — there is no
//! URL-less form of the LSP edit. The handler in `ruxen_lsp` already
//! has the document `Url` in scope, so we accept it as a parameter
//! and the stitch block plumbs it through.

use std::collections::HashMap;

use lsp_types::{
    CodeAction, CodeActionContext, CodeActionKind, CodeActionOrCommand,
    Diagnostic as LspDiagnostic, NumberOrString, Position, Range, TextEdit, Url, WorkspaceEdit,
};

use crate::analysis::AnalysisResult;
use crate::node_finder::enclosing_function;

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

fn quick_fix_e1006(result: &AnalysisResult, diag: &LspDiagnostic, uri: &Url) -> Option<CodeAction> {
    // The diagnostic span is the assignment, so it starts at the
    // assigned variable's name (`x` in `x = 43`). Read the name from the
    // source rather than parsing it out of the message text.
    let diag_byte = result.line_index.byte_offset_of(diag.range.start);
    let name = ident_at(&result.source, diag_byte)?;
    let (let_start, let_end) = find_let_token_for_name(&result.source, name, diag_byte)?;

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

/// The identifier starting at byte `at` in `source`, if any.
fn ident_at(source: &str, at: usize) -> Option<&str> {
    let rest = source.get(at..)?;
    let len = rest.bytes().take_while(|b| is_ident_byte(*b)).count();
    (len > 0).then(|| &rest[..len])
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
        // Allow `let mut`-style prefix tokens — Ruxen uses `let` and
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

fn quick_fix_e1110(result: &AnalysisResult, diag: &LspDiagnostic, uri: &Url) -> Option<CodeAction> {
    let await_byte = result.line_index.byte_offset_of(diag.range.start);
    let enclosing = enclosing_function(result.program.as_ref()?, await_byte)?;
    // An `.await` inside an `async def` is not E1110's fix to make.
    if enclosing.is_async {
        return None;
    }

    let pos = result.line_index.position_of(enclosing.span.start);
    let edit = TextEdit {
        range: Range {
            start: pos,
            end: pos,
        },
        new_text: "async ".to_string(),
    };

    Some(make_action(
        format!(
            "Add \u{2018}async\u{2019} to \u{2018}def {}\u{2019}",
            enclosing.name
        ),
        edit,
        diag.clone(),
        uri,
    ))
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
    fn ident_at_reads_the_assigned_name() {
        let src = "  foo_1 = 3\n";
        assert_eq!(ident_at(src, 2), Some("foo_1"));
        assert_eq!(ident_at(src, 8), None);
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
}
