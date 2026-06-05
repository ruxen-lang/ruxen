/// AST-to-Doc conversion for patterns.
use crate::parser::ast::*;

use super::comments::CommentMap;
use super::doc::*;
use super::format_expr::format_expr;

pub fn format_pattern(pat: &Pattern, comments: &CommentMap) -> Doc {
    match pat {
        Pattern::Literal { expr, .. } => format_expr(expr, comments),

        Pattern::Identifier { mutable, name, .. } => {
            if *mutable {
                concat(vec![text("var "), text(name.clone())])
            } else {
                text(name.clone())
            }
        }

        Pattern::Wildcard { .. } => text("_"),

        Pattern::Tuple { elements, .. } => {
            let items: Vec<Doc> = elements
                .iter()
                .map(|e| format_pattern(e, comments))
                .collect();
            group(concat(vec![
                text("("),
                nest(
                    INDENT_WIDTH,
                    concat(vec![
                        softline(),
                        join(concat(vec![text(","), line()]), items),
                    ]),
                ),
                softline(),
                text(")"),
            ]))
        }

        Pattern::Enum {
            path,
            variant,
            fields,
            ..
        } => {
            // The Option::None variant is spelled `nil` on the surface
            // (ruby-naming.spec.md §3.10); its internal variant name is
            // still "None", so emit `nil` when printing a bare `None`
            // pattern — otherwise the formatter round-trips to the now-
            // forbidden `None` identifier and the reparse fails E0008.
            if path.is_empty() && variant == "None" && fields.is_empty() {
                return text("nil");
            }

            let path_str = if path.is_empty() {
                variant.clone()
            } else {
                format!("{}.{}", path.join("."), variant)
            };

            if fields.is_empty() {
                text(path_str)
            } else {
                let field_docs: Vec<Doc> =
                    fields.iter().map(|f| format_pattern(f, comments)).collect();
                group(concat(vec![
                    text(path_str),
                    text("("),
                    nest(
                        INDENT_WIDTH,
                        concat(vec![
                            softline(),
                            join(concat(vec![text(","), line()]), field_docs),
                        ]),
                    ),
                    softline(),
                    text(")"),
                ]))
            }
        }

        Pattern::Struct {
            path, fields, rest, ..
        } => {
            let path_str = path.join(".");
            let mut field_docs: Vec<Doc> = fields
                .iter()
                .map(|f| {
                    match &f.name {
                        Some(name) => {
                            // Named field: `name: pattern`
                            concat(vec![
                                text(name.clone()),
                                text(": "),
                                format_pattern(&f.pattern, comments),
                            ])
                        }
                        None => {
                            // Shorthand: just the pattern
                            format_pattern(&f.pattern, comments)
                        }
                    }
                })
                .collect();

            if *rest {
                field_docs.push(text(".."));
            }

            group(concat(vec![
                text(path_str),
                text(" { "),
                nest(
                    INDENT_WIDTH,
                    join(concat(vec![text(","), line()]), field_docs),
                ),
                text(" }"),
            ]))
        }

        Pattern::Or { patterns, .. } => {
            let pat_docs: Vec<Doc> = patterns
                .iter()
                .map(|p| format_pattern(p, comments))
                .collect();
            join(text(" | "), pat_docs)
        }

        Pattern::Ref { mutable, name, .. } => {
            if *mutable {
                concat(vec![text("ref var "), text(name.clone())])
            } else {
                concat(vec![text("ref "), text(name.clone())])
            }
        }

        Pattern::Rest { .. } => text(".."),
    }
}

/// Format a pattern for use in match arm context — may need special wrapping.
pub fn format_match_pattern(pat: &Pattern, guard: Option<&Expr>, comments: &CommentMap) -> Doc {
    let pat_doc = format_pattern(pat, comments);
    match guard {
        Some(g) => concat(vec![pat_doc, text(" if "), format_expr(g, comments)]),
        None => pat_doc,
    }
}
