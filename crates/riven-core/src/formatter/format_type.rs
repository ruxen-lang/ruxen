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
                parts.push(text(format!("'{} ", lt)));
            }
            if *mutable {
                parts.push(text("mut "));
            }
            parts.push(format_type_expr(inner, _comments));
            concat(parts)
        }

        TypeExpr::Tuple { elements, .. } => {
            if elements.is_empty() {
                text("()")
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
            concat(vec![
                text("Fn"),
                params_doc,
                text(" -> "),
                format_type_expr(return_type, _comments),
            ])
        }

        TypeExpr::ImplTrait { bounds, .. } => {
            let bound_docs: Vec<Doc> = bounds.iter().map(|b| format_type_path(&b.path)).collect();
            concat(vec![text("impl "), join(text(" + "), bound_docs)])
        }

        TypeExpr::DynTrait { bounds, .. } => {
            let bound_docs: Vec<Doc> = bounds.iter().map(|b| format_type_path(&b.path)).collect();
            concat(vec![text("dyn "), join(text(" + "), bound_docs)])
        }

        TypeExpr::Never { .. } => text("!"),

        TypeExpr::Inferred { .. } => text("_"),

        TypeExpr::RawPointer { mutable, inner, .. } => {
            let prefix = if *mutable { "*mut " } else { "*" };
            concat(vec![text(prefix), format_type_expr(inner, _comments)])
        }

        TypeExpr::ConstLit { value, .. } => text(value.to_string()),
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
            GenericParam::Lifetime { name, .. } => text(format!("'{}", name)),
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

pub fn format_trait_bounds(bounds: &[TraitBound]) -> Doc {
    let docs: Vec<Doc> = bounds.iter().map(|b| format_type_path(&b.path)).collect();
    join(text(" + "), docs)
}
