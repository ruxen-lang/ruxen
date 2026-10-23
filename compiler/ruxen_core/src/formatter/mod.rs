/// Ruxen code formatter — zero-configuration, AST-based.
///
/// # Usage
///
/// ```ignore
/// use ruxenc::formatter;
///
/// let result = formatter::format(source_code);
/// if result.changed {
///     // Write result.output to the file
/// }
/// ```
pub mod comments;
pub mod doc;
pub mod format_expr;
pub mod format_items;
pub mod format_pattern;
pub mod format_type;
pub mod prec;

#[cfg(test)]
mod tests;

use crate::diagnostics::Diagnostic;
use crate::lexer::Lexer;
use crate::parser::Parser;

use comments::CommentAttacher;
use doc::render;
use format_items::format_program;

// ─── Public API ─────────────────────────────────────────────────────

/// Result of formatting a Ruxen source file.
#[derive(Debug, Clone)]
pub struct FormatResult {
    /// The formatted source code, or the original if formatting failed.
    pub output: String,
    /// Whether the source was changed.
    pub changed: bool,
    /// Any errors encountered (syntax errors in the input).
    pub errors: Vec<Diagnostic>,
}

/// Format a complete Ruxen source file.
pub fn format(source: &str) -> FormatResult {
    // 1. Lex; the lexer also records every comment's span.
    let mut lexer = Lexer::new(source);
    let tokens = match lexer.tokenize() {
        Ok(tokens) => tokens,
        Err(diagnostics) => {
            // Lexer errors — return source unchanged
            return FormatResult {
                output: source.to_string(),
                changed: false,
                errors: diagnostics,
            };
        }
    };

    let (raw_comments, fmt_off_ranges) = comments::classify_comments(source, lexer.comment_spans());

    // 2. Parse source to AST
    let mut parser = Parser::new(tokens);
    let program = match parser.parse() {
        Ok(program) => program,
        Err(diagnostics) => {
            // Parse errors — return source unchanged
            return FormatResult {
                output: source.to_string(),
                changed: false,
                errors: diagnostics,
            };
        }
    };

    // 3. Attach comments to AST nodes
    let node_spans = comments::collect_node_spans(&program);
    let mut comment_map = CommentAttacher::attach(raw_comments, &node_spans, fmt_off_ranges);
    comment_map.lines = comments::source_lines(source);

    // 4. Check for fmt: off covering the entire file
    if comment_map.is_fmt_off(0)
        && comment_map
            .fmt_off_ranges
            .first()
            .is_some_and(|r| r.end_byte.is_none())
    {
        return FormatResult {
            output: source.to_string(),
            changed: false,
            errors: vec![],
        };
    }

    // 5. Convert AST + comments to Doc IR
    let doc = format_program(&program, &comment_map);

    // A comment the formatter did not place would vanish from the output.
    // Leave the file untouched and say where, rather than delete it.
    let dropped = comment_map.dropped();
    if !dropped.is_empty() {
        return FormatResult {
            output: source.to_string(),
            changed: false,
            errors: dropped
                .into_iter()
                .map(|span| {
                    Diagnostic::error(
                        "ruxen fmt cannot place this comment, so the file was left unchanged \
                         (formatter bug: please report it)",
                        span.clone(),
                    )
                })
                .collect(),
        };
    }

    // 6. Render Doc IR to String
    let mut output = render(&doc);

    // 7. Strip trailing whitespace from each line
    output = strip_trailing_whitespace(&output);

    // 8. Ensure exactly one trailing newline
    output = ensure_trailing_newline(&output);

    // 9. Compress 3+ consecutive blank lines to 2
    output = compress_blank_lines(&output);

    let changed = output != source;

    FormatResult {
        output,
        changed,
        errors: vec![],
    }
}

/// Receiver mode for [`render_fn_signature`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SigSelf {
    /// No `self` receiver (free function / class method).
    None,
    /// `&self` — rendered with no prefix, matching `format_func_def`.
    Ref,
    /// `var self` (mutable receiver) — rendered as `var `.
    RefMut,
    /// `consume self` — rendered as `consume `.
    Consuming,
}

/// Canonical single-line `def` signature for IDE display (hover, signature
/// help, completion). Built from the same `signature_doc` parts as
/// `format_func_def`, so it cannot drift from `ruxen fmt`:
/// - no parameters → no parens (`def f`, never `def f()`)
/// - generics rendered as `[A, B]`
/// - `var` / `consume` self-mode prefixes; class methods get a `self.` marker
/// - return type appended as ` -> T` when `Some` (callers pass `None` to omit,
///   e.g. for a `Unit` return)
///
/// Callers supply already-rendered type strings, so the formatter takes no
/// dependency on the resolver's `Ty`.
pub fn render_fn_signature(
    name: &str,
    self_mode: SigSelf,
    is_class_method: bool,
    generics: &[String],
    params: &[(String, String)],
    return_ty: Option<&str>,
) -> String {
    use crate::parser::ast::SelfMode;
    use doc::{concat, text};

    let self_mode = match self_mode {
        SigSelf::None => None,
        SigSelf::Ref => Some(SelfMode::Immutable),
        SigSelf::RefMut => Some(SelfMode::Mutable),
        SigSelf::Consuming => Some(SelfMode::Consuming),
    };
    let generics = (!generics.is_empty()).then(|| text(format!("[{}]", generics.join(", "))));
    let params = params
        .iter()
        .map(|(n, t)| text(format!("{n}: {t}")))
        .collect();
    let parts = format_items::signature_doc(
        false,
        self_mode,
        is_class_method,
        name,
        generics,
        params,
        return_ty.map(text),
    );
    doc::print_doc(&concat(parts), i32::MAX)
}

// ─── Post-processing ────────────────────────────────────────────────

fn strip_trailing_whitespace(s: &str) -> String {
    s.lines()
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

fn ensure_trailing_newline(s: &str) -> String {
    if s.ends_with('\n') {
        s.to_string()
    } else {
        format!("{}\n", s)
    }
}

fn compress_blank_lines(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut consecutive_empty = 0;

    for line in s.split('\n') {
        if line.trim().is_empty() {
            consecutive_empty += 1;
            if consecutive_empty <= 2 {
                if !result.is_empty() {
                    result.push('\n');
                }
                result.push_str(line);
            }
        } else {
            consecutive_empty = 0;
            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str(line);
        }
    }

    result
}
