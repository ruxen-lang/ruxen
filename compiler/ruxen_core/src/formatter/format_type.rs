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
            ..
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

        TypeExpr::Tuple { elements, .. } => {
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

        TypeExpr::Array { element, size, .. } => {
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
            ..
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

        TypeExpr::SomeMixin { bounds, .. } => {
            let bound_docs: Vec<Doc> = bounds.iter().map(|b| format_type_path(&b.path)).collect();
            concat(vec![text("some "), join(text(" + "), bound_docs)])
        }

        TypeExpr::AnyMixin { bounds, .. } => {
            let bound_docs: Vec<Doc> = bounds.iter().map(|b| format_type_path(&b.path)).collect();
            concat(vec![text("any "), join(text(" + "), bound_docs)])
        }

        // The only surface spelling the type parser accepts is `Never`
        // (`parse_type` matches the `Never` TypeIdentifier); `!` is not a valid
        // type token and fails to re-parse ("expected type, found Bang").
        TypeExpr::Never { .. } => text("Never"),

        TypeExpr::Inferred { .. } => text("_"),

        TypeExpr::RawPointer { mutable, inner, .. } => {
            let prefix = if *mutable { "*var " } else { "*" };
            concat(vec![text(prefix), format_type_expr(inner, _comments)])
        }

        TypeExpr::ConstLit { value, .. } => text(value.to_string()),
        // For now route ConstExprArg through the parser's
        // short-printer; a Doc-native printer for arithmetic in
        // const-arg position can come later when formatting
        // const-generic arithmetic gets style attention.
        TypeExpr::ConstExprArg { expr, .. } => {
            text(crate::parser::printer::format_expr_short(expr))
        }
    }
}

pub fn format_type_path(path: &TypePath) -> Doc {
    let segments_doc = text(path.segments.join("."));
    match &path.generic_args {
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
    let param_docs: Vec<Doc> = gp
        .params
        .iter()
        .map(|p| match p {
            GenericParam::Lifetime { name, .. } => text(name.clone()),
            GenericParam::Type { name, bounds, .. } => {
                if bounds.is_empty() {
                    text(name.clone())
                } else {
                    let bound_docs: Vec<Doc> =
                        bounds.iter().map(|b| format_type_path(&b.path)).collect();
                    concat(vec![
                        text(name.clone()),
                        text(": "),
                        join(text(" + "), bound_docs),
                    ])
                }
            }
            GenericParam::Const { name, ty, .. } => concat(vec![
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

pub fn format_where_clause(wc: &WhereClause) -> Doc {
    let pred_docs: Vec<Doc> = wc
        .predicates
        .iter()
        .map(|p| {
            let bound_docs: Vec<Doc> = p.bounds.iter().map(|b| format_type_path(&b.path)).collect();
            concat(vec![
                format_type_expr(&p.type_expr, &CommentMap::new()),
                text(": "),
                join(text(" + "), bound_docs),
            ])
        })
        .collect();

    if pred_docs.len() == 1 {
        // Short where clause on same line
        group(concat(vec![
            text(" where "),
            pred_docs.into_iter().next().unwrap(),
        ]))
    } else {
        // Multi-predicate: one per line
        group(concat(vec![
            hardline(),
            text("where"),
            nest(
                INDENT_WIDTH,
                concat(
                    pred_docs
                        .into_iter()
                        .map(|p| concat(vec![hardline(), p, text(",")]))
                        .collect(),
                ),
            ),
        ]))
    }
}

pub fn format_trait_bounds(bounds: &[MixinBound]) -> Doc {
    let docs: Vec<Doc> = bounds.iter().map(|b| format_type_path(&b.path)).collect();
    join(text(" + "), docs)
}
