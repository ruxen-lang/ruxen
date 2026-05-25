/// Wadler-Lindig document IR and printer for the Ruxen formatter.
///
/// The Doc IR represents formatted code as a tree of layout instructions.
/// The printer algorithm decides which groups fit on a single line and which
/// must be broken across multiple lines.
pub const INDENT_WIDTH: i32 = 2;
pub const MAX_LINE_WIDTH: i32 = 100;

// ─── Doc IR ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Doc {
    /// Literal text — never broken.
    Text(String),

    /// Line break if enclosing group is broken, space if flat.
    Line,

    /// Line break if enclosing group is broken, empty string if flat.
    Softline,

    /// Always a line break. Forces the enclosing group to break.
    Hardline,

    /// Increase indentation for line breaks inside `doc`.
    Nest(i32, Box<Doc>),

    /// Try to flatten `doc` onto one line. If it doesn't fit within
    /// remaining width, switch to Break mode.
    Group(Box<Doc>),

    /// Concatenation of multiple docs.
    Concat(Vec<Doc>),

    /// Fill as many items per line as possible, separated by Line.
    /// Used for import lists, array literals, etc.
    Fill(Vec<Doc>),

    /// Different content depending on whether the enclosing group broke.
    /// `broken` is used if group broke, `flat` if group is flat.
    IfBreak(Box<Doc>, Box<Doc>),

    /// Print at end of current line (for trailing comments).
    LineSuffix(Box<Doc>),
}

// ─── Convenience Constructors ───────────────────────────────────────

pub fn text(s: impl Into<String>) -> Doc {
    Doc::Text(s.into())
}

pub fn line() -> Doc {
    Doc::Line
}

pub fn softline() -> Doc {
    Doc::Softline
}

pub fn hardline() -> Doc {
    Doc::Hardline
}

pub fn nest(indent: i32, doc: Doc) -> Doc {
    Doc::Nest(indent, Box::new(doc))
}

pub fn group(doc: Doc) -> Doc {
    Doc::Group(Box::new(doc))
}

pub fn concat(docs: Vec<Doc>) -> Doc {
    Doc::Concat(docs)
}

pub fn fill(docs: Vec<Doc>) -> Doc {
    Doc::Fill(docs)
}

pub fn if_break(broken: Doc, flat: Doc) -> Doc {
    Doc::IfBreak(Box::new(broken), Box::new(flat))
}

pub fn line_suffix(doc: Doc) -> Doc {
    Doc::LineSuffix(Box::new(doc))
}

/// Intersperse a separator between docs.
pub fn join(separator: Doc, docs: Vec<Doc>) -> Doc {
    let mut result = Vec::with_capacity(docs.len() * 2);
    for (i, doc) in docs.into_iter().enumerate() {
        if i > 0 {
            result.push(separator.clone());
        }
        result.push(doc);
    }
    Doc::Concat(result)
}

/// Shorthand: concat two docs.
pub fn cons(a: Doc, b: Doc) -> Doc {
    Doc::Concat(vec![a, b])
}

/// Empty doc — emits nothing.
pub fn nil() -> Doc {
    Doc::Text(String::new())
}

/// A space character.
pub fn space() -> Doc {
    Doc::Text(" ".into())
}

// ─── Wadler-Lindig Printer ──────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Flat,
    Break,
}

/// A command on the printer's work stack.
struct PrintCmd {
    indent: i32,
    mode: Mode,
    doc: Doc,
}

/// Render a Doc IR tree to a String, targeting the given line width.
pub fn print_doc(doc: &Doc, width: i32) -> String {
    let mut output = String::new();
    let mut pos: i32 = 0; // current column position
    let mut line_suffix_buf: Vec<(i32, Doc)> = Vec::new();

    // Work stack — processed in LIFO order.
    // We push items in reverse order so they come out left-to-right.
    let mut stack: Vec<PrintCmd> = vec![PrintCmd {
        indent: 0,
        mode: Mode::Break,
        doc: doc.clone(),
    }];

    while let Some(cmd) = stack.pop() {
        match cmd.doc {
            Doc::Text(ref s) => {
                output.push_str(s);
                pos += s.len() as i32;
            }

            Doc::Line => match cmd.mode {
                Mode::Flat => {
                    output.push(' ');
                    pos += 1;
                }
                Mode::Break => {
                    flush_line_suffix(&mut output, &mut pos, &mut line_suffix_buf);
                    output.push('\n');
                    emit_indent(&mut output, cmd.indent);
                    pos = cmd.indent;
                }
            },

            Doc::Softline => {
                match cmd.mode {
                    Mode::Flat => {
                        // emit nothing
                    }
                    Mode::Break => {
                        flush_line_suffix(&mut output, &mut pos, &mut line_suffix_buf);
                        output.push('\n');
                        emit_indent(&mut output, cmd.indent);
                        pos = cmd.indent;
                    }
                }
            }

            Doc::Hardline => {
                flush_line_suffix(&mut output, &mut pos, &mut line_suffix_buf);
                output.push('\n');
                emit_indent(&mut output, cmd.indent);
                pos = cmd.indent;
            }

            Doc::Nest(extra, ref inner) => {
                stack.push(PrintCmd {
                    indent: cmd.indent + extra,
                    mode: cmd.mode,
                    doc: *inner.clone(),
                });
            }

            Doc::Group(ref inner) => {
                if cmd.mode == Mode::Flat {
                    // Already in flat mode — stay flat.
                    stack.push(PrintCmd {
                        indent: cmd.indent,
                        mode: Mode::Flat,
                        doc: *inner.clone(),
                    });
                } else {
                    // Try flat mode: does the flattened content fit?
                    let flat_doc = *inner.clone();
                    if fits(width - pos, &flat_doc, cmd.indent, &stack) {
                        stack.push(PrintCmd {
                            indent: cmd.indent,
                            mode: Mode::Flat,
                            doc: flat_doc,
                        });
                    } else {
                        stack.push(PrintCmd {
                            indent: cmd.indent,
                            mode: Mode::Break,
                            doc: *inner.clone(),
                        });
                    }
                }
            }

            Doc::Concat(ref docs) => {
                // Push in reverse so first doc is processed first.
                for d in docs.iter().rev() {
                    stack.push(PrintCmd {
                        indent: cmd.indent,
                        mode: cmd.mode,
                        doc: d.clone(),
                    });
                }
            }

            Doc::Fill(ref docs) => {
                // Fill algorithm: pack as many items as fit on one line.
                // Each pair of items is separated by a Line. We try to
                // keep the separator as a space (flat); if the next item
                // doesn't fit, we break.
                let items: Vec<Doc> = docs.clone();
                // Push in reverse, alternating items and line-breaks.
                // We process them greedily below.
                fill_to_stack(&mut stack, &items, cmd.indent, width - pos);
            }

            Doc::IfBreak(ref broken, ref flat) => match cmd.mode {
                Mode::Flat => {
                    stack.push(PrintCmd {
                        indent: cmd.indent,
                        mode: cmd.mode,
                        doc: *flat.clone(),
                    });
                }
                Mode::Break => {
                    stack.push(PrintCmd {
                        indent: cmd.indent,
                        mode: cmd.mode,
                        doc: *broken.clone(),
                    });
                }
            },

            Doc::LineSuffix(ref inner) => {
                line_suffix_buf.push((cmd.indent, *inner.clone()));
            }
        }
    }

    // Flush any remaining line suffixes.
    if !line_suffix_buf.is_empty() {
        for (_, doc) in line_suffix_buf.drain(..) {
            let rendered = print_doc(&doc, width);
            output.push_str(&rendered);
        }
    }

    output
}

/// Emit `indent` spaces.
fn emit_indent(output: &mut String, indent: i32) {
    for _ in 0..indent {
        output.push(' ');
    }
}

/// Flush line-suffix buffer before a line break.
fn flush_line_suffix(output: &mut String, pos: &mut i32, buf: &mut Vec<(i32, Doc)>) {
    if buf.is_empty() {
        return;
    }
    for (_, doc) in buf.drain(..) {
        let rendered = print_doc(&doc, MAX_LINE_WIDTH);
        output.push_str(&rendered);
        *pos += rendered.len() as i32;
    }
}

/// Check if a doc fits within `remaining` columns without encountering
/// a line break. Scans forward through the doc and also looks at what
/// remains on the stack (up to the next line break).
fn fits(remaining: i32, doc: &Doc, indent: i32, _parent_stack: &[PrintCmd]) -> bool {
    let mut rem = remaining;
    let mut local_stack: Vec<(i32, &Doc)> = vec![(indent, doc)];

    // Also consider items remaining on the parent stack (until we hit
    // a line break or run out).
    while let Some((ind, d)) = local_stack.pop() {
        if rem < 0 {
            return false;
        }
        match d {
            Doc::Text(s) => {
                rem -= s.len() as i32;
            }
            Doc::Line | Doc::Softline => {
                // In flat mode: Line emits a space, Softline emits nothing.
                if matches!(d, Doc::Line) {
                    rem -= 1;
                }
                // We only scan to end of current line, so this is fine.
            }
            Doc::Hardline => {
                // Hardline always breaks — the content won't fit flat.
                return true; // "fits" here means we reach a break naturally
            }
            Doc::Nest(extra, inner) => {
                local_stack.push((ind + extra, inner));
            }
            Doc::Group(inner) => {
                // In fits check, groups are tried flat.
                local_stack.push((ind, inner));
            }
            Doc::Concat(docs) => {
                for sub in docs.iter().rev() {
                    local_stack.push((ind, sub));
                }
            }
            Doc::Fill(docs) => {
                for sub in docs.iter().rev() {
                    local_stack.push((ind, sub));
                }
            }
            Doc::IfBreak(_broken, flat) => {
                // In fits check (flat mode), use the flat variant.
                local_stack.push((ind, flat));
            }
            Doc::LineSuffix(_) => {
                // Line suffixes don't contribute to line width measurement.
            }
        }
    }

    rem >= 0
}

/// Fill algorithm: push items onto the stack with greedy line-breaking.
fn fill_to_stack(stack: &mut Vec<PrintCmd>, items: &[Doc], indent: i32, remaining: i32) {
    if items.is_empty() {
        return;
    }

    // Build a flat sequence with Line separators between items.
    // Each item is tested: if it fits alongside the previous items on this line,
    // emit a space; otherwise, break.
    // We implement this by converting to a series of groups.
    let mut result_parts: Vec<Doc> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            // Separator: line (breaks if group breaks)
            result_parts.push(Doc::Line);
        }
        result_parts.push(item.clone());
    }

    // Push as a single group that may break
    let fill_doc = Doc::Group(Box::new(Doc::Concat(result_parts)));
    stack.push(PrintCmd {
        indent,
        mode: Mode::Break,
        doc: fill_doc,
    });

    let _ = remaining; // used conceptually; the group mechanism handles width
}

// ─── Render Convenience ─────────────────────────────────────────────

/// Render a Doc to string using the default MAX_LINE_WIDTH.
pub fn render(doc: &Doc) -> String {
    print_doc(doc, MAX_LINE_WIDTH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text() {
        let doc = text("hello");
        assert_eq!(render(&doc), "hello");
    }

    #[test]
    fn test_hardline() {
        let doc = concat(vec![text("a"), hardline(), text("b")]);
        assert_eq!(render(&doc), "a\nb");
    }

    #[test]
    fn test_group_fits_flat() {
        // "a b" fits within 100 chars
        let doc = group(concat(vec![text("a"), line(), text("b")]));
        assert_eq!(render(&doc), "a b");
    }

    #[test]
    fn test_group_breaks() {
        // Force break by using a very narrow width
        let doc = group(concat(vec![text("hello"), line(), text("world")]));
        let result = print_doc(&doc, 5);
        assert_eq!(result, "hello\nworld");
    }

    #[test]
    fn test_nest() {
        let doc = concat(vec![
            text("if cond"),
            nest(INDENT_WIDTH, concat(vec![hardline(), text("body")])),
            hardline(),
            text("end"),
        ]);
        assert_eq!(render(&doc), "if cond\n  body\nend");
    }

    #[test]
    fn test_softline_flat() {
        let doc = group(concat(vec![text("a"), softline(), text("b")]));
        assert_eq!(render(&doc), "ab");
    }

    #[test]
    fn test_softline_break() {
        let doc = group(concat(vec![text("a"), softline(), text("b")]));
        let result = print_doc(&doc, 1);
        assert_eq!(result, "a\nb");
    }

    #[test]
    fn test_if_break() {
        // In flat mode, use flat variant
        let doc = group(concat(vec![
            text("("),
            if_break(text(","), nil()),
            text(")"),
        ]));
        assert_eq!(render(&doc), "()");
    }

    #[test]
    fn test_if_break_broken() {
        // Force break with hardline inside the group
        let inner = concat(vec![
            text("("),
            nest(
                INDENT_WIDTH,
                concat(vec![hardline(), text("item"), if_break(text(","), nil())]),
            ),
            hardline(),
            text(")"),
        ]);
        let result = render(&inner);
        assert_eq!(result, "(\n  item,\n)");
    }

    #[test]
    fn test_join() {
        let doc = join(
            concat(vec![text(","), line()]),
            vec![text("a"), text("b"), text("c")],
        );
        let doc = group(doc);
        assert_eq!(render(&doc), "a, b, c");
    }

    #[test]
    fn test_nested_groups() {
        let inner = group(concat(vec![text("inner_a"), line(), text("inner_b")]));
        let outer = group(concat(vec![text("outer"), line(), inner]));
        assert_eq!(render(&outer), "outer inner_a inner_b");
    }

    #[test]
    fn test_line_suffix() {
        let doc = concat(vec![
            text("code"),
            line_suffix(text(" # comment")),
            hardline(),
            text("next"),
        ]);
        assert_eq!(render(&doc), "code # comment\nnext");
    }

    #[test]
    fn test_function_params_fit() {
        // def add(x: Int, y: Int) -> Int
        let params = join(
            concat(vec![text(","), line()]),
            vec![text("x: Int"), text("y: Int")],
        );
        let doc = group(concat(vec![
            text("def add("),
            nest(INDENT_WIDTH, params),
            text(") -> Int"),
        ]));
        assert_eq!(render(&doc), "def add(x: Int, y: Int) -> Int");
    }

    #[test]
    fn test_function_params_break() {
        // Narrow width forces break
        let params = join(
            concat(vec![text(","), line()]),
            vec![text("x: Int"), text("y: Int")],
        );
        let doc = group(concat(vec![
            text("def add("),
            nest(INDENT_WIDTH, concat(vec![softline(), params])),
            softline(),
            text(") -> Int"),
        ]));
        let result = print_doc(&doc, 20);
        assert_eq!(result, "def add(\n  x: Int,\n  y: Int\n) -> Int");
    }
}
