//! LSP `textDocument/formatting` and `textDocument/rangeFormatting`
//! — thin adapter around `ruxen_core::formatter::format`.
//!
//! The strategy is intentionally minimal (and matches what the spec
//! §5.8 prescribes): produce the canonical formatted source for the
//! whole document, then emit a **single** `TextEdit` that replaces the
//! whole document (or the requested range, when the same text would
//! result) with the formatted output. The LSP client applies the
//! replacement atomically — no per-line diff to keep in sync.
//!
//! ## Semantics
//!
//! - `format_document(source)`:
//!   * `None` if the formatter could not parse the source (syntax
//!     errors). The client should leave the buffer alone.
//!   * `Some(vec![])` if the source is already canonically formatted.
//!   * `Some(vec![one_edit])` replacing the whole document otherwise.
//!
//! - `format_range(source, range)`: the underlying formatter does not
//!   support partial input (`format_range` in `ruxen_core` simply
//!   formats the whole file). Per the task brief, when partial
//!   formatting is rejected we fall back to formatting the whole
//!   document and return a single edit that covers the **entire
//!   document** — the formatted text does not align line-by-line with
//!   the original, so a sub-range slice would silently corrupt the
//!   buffer if it spanned a re-indented block. The client gets a
//!   safe, atomic replacement.

use lsp_types::{Position, Range, TextEdit};

use crate::line_index::LineIndex;

/// LSP `textDocument/formatting` — return edits that transform
/// `source` into its canonical formatted form, or `None` if the
/// source fails to parse.
pub fn format_document(source: &str) -> Option<Vec<TextEdit>> {
    let result = ruxen_core::formatter::format(source);

    // Parse / lex failures: the formatter returns the source unchanged
    // and populates `errors`. We cannot safely emit edits in that
    // state; tell the client to leave the buffer alone.
    if !result.errors.is_empty() {
        return None;
    }

    // Already canonical — empty edit list is the LSP idiom for
    // "nothing to do".
    if !result.changed {
        return Some(Vec::new());
    }

    Some(vec![whole_document_replace(source, result.output)])
}

/// LSP `textDocument/rangeFormatting` — same contract as
/// [`format_document`], but the formatter does not support partial
/// input, so we fall back to a whole-document replacement.
///
/// The `range` argument is accepted for API conformance and to leave
/// room for a future partial-format path; today it is intentionally
/// unused.
pub fn format_range(source: &str, range: Range) -> Option<Vec<TextEdit>> {
    // Mirror `format_document`. We deliberately do NOT slice
    // `source[range]` and feed it through the formatter — Ruxen's
    // formatter is whole-program (it needs item-level context for
    // indent / blank-line rules), and a sub-range slice would in
    // general not be a syntactically valid program.
    //
    // The `range` parameter is kept on the signature so the LSP
    // handler can pass it through unchanged, and so a future partial-
    // format mode can light up without breaking callers.
    let _ = range;
    format_document(source)
}

// ─── Helpers ────────────────────────────────────────────────────────

/// Build a `TextEdit` whose range spans the whole of `original` and
/// whose new text is `formatted`. The end position is computed from
/// a fresh `LineIndex` so it correctly handles UTF-16 column counts.
fn whole_document_replace(original: &str, formatted: String) -> TextEdit {
    let line_index = LineIndex::new(original);
    let end = line_index.position_of(original.len());
    TextEdit {
        range: Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end,
        },
        new_text: formatted,
    }
}
