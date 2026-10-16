/// AST-to-Doc conversion for type expressions.
use crate::parser::ast::*;

use super::comments::CommentMap;
use super::doc::*;

pub fn format_type_expr(ty: &TypeExpr, _comments: &CommentMap) -> Doc {
    match ty {
        TypeExpr::Named(path) => format_type_path(path),

        TypeExpr::Reference {
            lifetime,
            mutable,
            inner,
            span: _,
        } => {
            let mut parts = vec![text("&")];
            if let Some(lt) = lifetime {
                parts.push(text(format!("{} ", lt)));
            }
            if *mutable {
                parts.push(text("var "));
            }
            parts.push(format_type_expr(inner, _comments));
            concat(parts)
        }

        TypeExpr::Tuple { elements, span: _ } => {
            if elements.is_empty() {
                // ruby-naming.spec.md §3.10: the unit type is spelled `nil`,
                // not `()`. The parser maps both spellings to an empty-tuple
                // node but REJECTS `()` in type position, so the formatter
                // must emit `nil` here or its output fails to re-parse.
                text("nil")
            } else {
                let items: Vec<Doc> = elements
                    .iter()
                    .map(|e| format_type_expr(e, _comments))
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
        }

        TypeExpr::Array {
            element,
            size,
            span: _,
        } => {
            let mut parts = vec![text("[")];
            parts.push(format_type_expr(element, _comments));
            if let Some(sz) = size {
                parts.push(text("; "));
                parts.push(super::format_expr::format_expr(sz, _comments));
            }
            parts.push(text("]"));
            concat(parts)
        }

        TypeExpr::Function {
            params,
            return_type,
            bracketed,
            span: _,
        } => {
            let param_docs: Vec<Doc> = params
                .iter()
                .map(|p| format_type_expr(p, _comments))
                .collect();
            let params_doc = if param_docs.is_empty() {
                text("()")
            } else {
                group(concat(vec![
                    text("("),
                    nest(
                        INDENT_WIDTH,
                        concat(vec![
                            softline(),
                            join(concat(vec![text(","), line()]), param_docs),
                        ]),
                    ),
                    softline(),
                    text(")"),
                ]))
            };
            let sig = concat(vec![
                params_doc,
                text(" -> "),
                format_type_expr(return_type, _comments),
            ]);
            if *bracketed {
                // Canonical block signature `Fn[(T…) -> R]` — preserve the
                // square-bracket spelling the author wrote (ADR D8). Never
                // rewrite it to the paren form.
                concat(vec![text("Fn["), sig, text("]")])
            } else {
                concat(vec![text("Fn"), sig])
            }
        }

        TypeExpr::SomeMixin { bounds, span: _ } => {
            concat(vec![text("some "), format_trait_bounds(bounds)])
        }

        TypeExpr::AnyMixin { bounds, span: _ } => {
            concat(vec![text("any "), format_trait_bounds(bounds)])
        }

        // The only surface spelling the type parser accepts is `Never`
        // (`parse_type` matches the `Never` TypeIdentifier); `!` is not a valid
        // type token and fails to re-parse ("expected type, found Bang").
        TypeExpr::Never { span: _ } => text("Never"),

        TypeExpr::Inferred { span: _ } => text("_"),

        TypeExpr::RawPointer {
            mutable,
            inner,
            span: _,
        } => {
            let prefix = if *mutable { "*var " } else { "*" };
            concat(vec![text(prefix), format_type_expr(inner, _comments)])
        }

        TypeExpr::ConstLit { value, span: _ } => text(value.to_string()),
        TypeExpr::ConstExprArg { expr, span: _ } => {
            super::format_expr::format_expr(expr, _comments)
        }
    }
}

pub fn format_type_path(path: &TypePath) -> Doc {
    let TypePath {
        segments,
        generic_args,
        span: _,
        rooted,
        nilable,
    } = path;
    // `T?` as written; the parser builds it as `Option[T]`.
    if *nilable {
        if let Some([inner]) = generic_args.as_deref() {
            return concat(vec![format_type_expr(inner, &CommentMap::new()), text("?")]);
        }
    }
    // `::Name` anchors the lookup at the root scope.
    let root = if *rooted { "::" } else { "" };
    let segments_doc = text(format!("{root}{}", segments.join(".")));
    match generic_args {
        None => segments_doc,
        Some(args) if args.is_empty() => segments_doc,
        Some(args) => {
            let arg_docs: Vec<Doc> = args
                .iter()
                .map(|a| format_type_expr(a, &CommentMap::new()))
                .collect();
            group(concat(vec![
                segments_doc,
                text("["),
                nest(
                    INDENT_WIDTH,
                    concat(vec![
                        softline(),
                        join(concat(vec![text(","), line()]), arg_docs),
                    ]),
                ),
                softline(),
                text("]"),
            ]))
        }
    }
}

pub fn format_generic_params(gp: &GenericParams) -> Doc {
    let GenericParams { params, span: _ } = gp;
    let param_docs: Vec<Doc> = params
        .iter()
        .map(|p| match p {
            GenericParam::Lifetime { name, span: _ } => text(name.clone()),
            GenericParam::Type {
                name,
                bounds,
                span: _,
            } => {
                if bounds.is_empty() {
                    text(name.clone())
                } else {
                    concat(vec![
                        text(name.clone()),
                        text(": "),
                        format_trait_bounds(bounds),
                    ])
                }
            }
            GenericParam::Const { name, ty, span: _ } => concat(vec![
                text("const "),
                text(name.clone()),
                text(": "),
                format_type_expr(ty, &CommentMap::new()),
            ]),
        })
        .collect();

    group(concat(vec![
        text("["),
        nest(
            INDENT_WIDTH,
            concat(vec![
                softline(),
                join(concat(vec![text(","), line()]), param_docs),
            ]),
        ),
        softline(),
        text("]"),
    ]))
}

/// ` where T: A + B, N > 0`. Trait predicates come before const
/// predicates, which is how the parser splits them anyway. It stays on the
/// header line, or on its own indented line when the author wrote it below a
/// header that starts on `header_line`.
pub fn format_where_clause(wc: &WhereClause, header_line: u32) -> Doc {
    let WhereClause {
        predicates,
        const_predicates,
        span: _,
    } = wc;
    let comments = CommentMap::new();
    let trait_docs = predicates.iter().map(|p| {
        let WherePredicate {
            type_expr,
            bounds,
            span: _,
        } = p;
        concat(vec![
            format_type_expr(type_expr, &comments),
            text(": "),
            format_trait_bounds(bounds),
        ])
    });
    let const_docs = const_predicates.iter().map(|p| {
        let ConstPredicate { expr, span: _ } = p;
        super::format_expr::format_expr(expr, &comments)
    });
    let pred_docs: Vec<Doc> = trait_docs.chain(const_docs).collect();
    let clause = concat(vec![text("where "), join(text(", "), pred_docs)]);
    if wc.span.line > header_line {
        nest(INDENT_WIDTH, concat(vec![hardline(), clause]))
    } else {
        concat(vec![text(" "), clause])
    }
}

pub fn format_trait_bounds(bounds: &[MixinBound]) -> Doc {
    let docs: Vec<Doc> = bounds
        .iter()
        .map(|MixinBound { path, span: _ }| format_type_path(path))
        .collect();
    join(text(" + "), docs)
}
