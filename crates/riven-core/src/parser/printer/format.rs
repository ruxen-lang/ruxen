//! Free-standing formatting helpers for the AST pretty-printer.
//!
//! These build compact, one-line string forms of types, type paths,
//! expressions, and patterns. They are used both by the indented
//! [`PrettyPrinter`](super::PrettyPrinter) and by other crates that need a
//! cheap textual rendering of AST fragments (e.g. the formatter).

use super::super::ast::*;
use crate::lexer::token::StringPart;

pub(super) fn format_visibility(vis: Visibility) -> &'static str {
    // §3.2: `public` is the default and the section marker for switching back
    // to public. We emit "public " only when the AST explicitly recorded it so
    // the debug dump preserves the distinction between an implicit public item
    // and one that was tagged public to flip a private section.
    match vis {
        Visibility::Private => "private ",
        Visibility::Public => "",
        Visibility::Protected => "protected ",
    }
}

pub(super) fn format_self_mode(m: SelfMode) -> &'static str {
    // §3.12: receiver modes are reading (default, no keyword), writing
    // (`def var m`), and consuming (`def consume m`). The debug dump shows
    // them in the prefix-on-`self` form for readability.
    match m {
        SelfMode::Immutable => "self",
        SelfMode::Mutable => "var self",
        SelfMode::Consuming => "consume self",
    }
}

pub(super) fn format_opt_generic_params(gp: &Option<GenericParams>) -> String {
    match gp {
        None => String::new(),
        Some(gp) => {
            let params: Vec<String> = gp
                .params
                .iter()
                .map(|p| match p {
                    // §3.3: lifetime parameters are bare lowercase identifiers,
                    // no sigil. The lexical position in `[...]` is what marks
                    // them as lifetimes.
                    GenericParam::Lifetime { name, .. } => name.clone(),
                    GenericParam::Type { name, bounds, .. } => {
                        if bounds.is_empty() {
                            name.clone()
                        } else {
                            let bs: Vec<String> =
                                bounds.iter().map(|b| format_type_path(&b.path)).collect();
                            format!("{}: {}", name, bs.join(" + "))
                        }
                    }
                    GenericParam::Const { name, ty, .. } => {
                        format!("const {}: {}", name, format_type(ty))
                    }
                })
                .collect();
            format!("[{}]", params.join(", "))
        }
    }
}

pub(super) fn format_where_clause(w: &WhereClause) -> String {
    let preds: Vec<String> = w
        .predicates
        .iter()
        .map(|p| {
            let bounds: Vec<String> = p.bounds.iter().map(|b| format_type_path(&b.path)).collect();
            format!("{}: {}", format_type(&p.type_expr), bounds.join(" + "))
        })
        .collect();
    format!("where {}", preds.join(", "))
}

pub(super) fn format_method_sig(sig: &MethodSig) -> String {
    let generics = format_opt_generic_params(&sig.generic_params);
    let class_marker = if sig.is_class_method { "self." } else { "" };
    let self_mode = sig
        .self_mode
        .as_ref()
        .map(|m| format!("{}, ", format_self_mode(*m)))
        .unwrap_or_default();
    let params: Vec<String> = sig
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name, format_type(&p.type_expr)))
        .collect();
    let ret = sig
        .return_type
        .as_ref()
        .map(|t| format!(" -> {}", format_type(t)))
        .unwrap_or_default();
    format!(
        "{}{}{}({}{}){}",
        class_marker,
        sig.name,
        generics,
        self_mode,
        params.join(", "),
        ret
    )
}

/// Format a type expression into a compact string.
pub fn format_type(t: &TypeExpr) -> String {
    match t {
        TypeExpr::Named(path) => format_type_path(path),
        TypeExpr::Reference {
            lifetime,
            mutable,
            inner,
            ..
        } => {
            let lt = lifetime
                .as_ref()
                .map(|l| format!("{} ", l))
                .unwrap_or_default();
            let m = if *mutable { "var " } else { "" };
            format!("&{}{}{}", lt, m, format_type(inner))
        }
        TypeExpr::Tuple { elements, .. } => {
            let elems: Vec<String> = elements.iter().map(format_type).collect();
            format!("({})", elems.join(", "))
        }
        TypeExpr::Array { element, size, .. } => match size {
            Some(sz) => format!("[{}; {}]", format_type(element), format_expr_short(sz)),
            None => format!("[{}]", format_type(element)),
        },
        TypeExpr::Function {
            params,
            return_type,
            ..
        } => {
            let ps: Vec<String> = params.iter().map(format_type).collect();
            format!("Fn({}) -> {}", ps.join(", "), format_type(return_type))
        }
        TypeExpr::SomeMixin { bounds, .. } => {
            let bs: Vec<String> = bounds.iter().map(|b| format_type_path(&b.path)).collect();
            format!("some {}", bs.join(" + "))
        }
        TypeExpr::AnyMixin { bounds, .. } => {
            let bs: Vec<String> = bounds.iter().map(|b| format_type_path(&b.path)).collect();
            format!("any {}", bs.join(" + "))
        }
        TypeExpr::Never { .. } => "!".to_string(),
        TypeExpr::Inferred { .. } => "_".to_string(),
        TypeExpr::RawPointer { mutable, inner, .. } => {
            if *mutable {
                format!("*var {}", format_type(inner))
            } else {
                format!("*{}", format_type(inner))
            }
        }
        TypeExpr::ConstLit { value, .. } => value.to_string(),
        TypeExpr::ConstExprArg { expr, .. } => format_expr_short(expr),
    }
}

/// Format a type path like `std.collections.Map[K, V]`.
pub fn format_type_path(p: &TypePath) -> String {
    let base = p.segments.join(".");
    match &p.generic_args {
        None => base,
        Some(args) => {
            let a: Vec<String> = args.iter().map(format_type).collect();
            format!("{}[{}]", base, a.join(", "))
        }
    }
}

/// Format an expression in abbreviated (one-line) form.
pub fn format_expr_short(e: &Expr) -> String {
    match &e.kind {
        ExprKind::IntLiteral(v, suffix) => format_numeric(*v as f64, suffix),
        ExprKind::FloatLiteral(v, suffix) => format_numeric(*v, suffix),
        ExprKind::StringLiteral(s) => format!("\"{}\"", s),
        ExprKind::InterpolatedString(parts) => {
            let mut out = String::from("\"");
            for part in parts {
                match part {
                    StringPart::Literal(s) => out.push_str(s),
                    StringPart::Expr { spec, .. } => {
                        if spec.is_default() {
                            out.push_str("#{...}");
                        } else {
                            out.push_str("#{...:");
                            if let Some(c) = spec.fill {
                                out.push(c);
                            }
                            if let Some(c) = spec.align {
                                out.push(c);
                            }
                            if let Some(w) = spec.width {
                                out.push_str(&w.to_string());
                            }
                            if let Some(p) = spec.precision {
                                out.push('.');
                                out.push_str(&p.to_string());
                            }
                            if spec.debug {
                                out.push('?');
                            }
                            out.push('}');
                        }
                    }
                }
            }
            out.push('"');
            out
        }
        ExprKind::CharLiteral(c) => format!("'{}'", c),
        ExprKind::BoolLiteral(b) => b.to_string(),
        ExprKind::UnitLiteral => "()".to_string(),
        ExprKind::Identifier(name) => name.clone(),
        ExprKind::SelfRef => "self".to_string(),
        ExprKind::SelfType => "Self".to_string(),

        ExprKind::BinaryOp { left, op, right } => {
            format!(
                "({} {:?} {})",
                format_expr_short(left),
                op,
                format_expr_short(right)
            )
        }
        ExprKind::UnaryOp { op, operand } => {
            format!("({:?} {})", op, format_expr_short(operand))
        }

        ExprKind::Borrow(inner) => format!("&{}", format_expr_short(inner)),
        ExprKind::BorrowMut(inner) => format!("&var {}", format_expr_short(inner)),

        ExprKind::FieldAccess { object, field } => {
            format!("{}.{}", format_expr_short(object), field)
        }
        ExprKind::MethodCall {
            object,
            method,
            generic_args,
            args,
            ..
        } => {
            let a: Vec<String> = args.iter().map(format_expr_short).collect();
            let generics = if generic_args.is_empty() {
                String::new()
            } else {
                let parts: Vec<String> = generic_args.iter().map(format_type).collect();
                format!("[{}]", parts.join(", "))
            };
            format!(
                "{}.{}{}({})",
                format_expr_short(object),
                method,
                generics,
                a.join(", ")
            )
        }
        ExprKind::SafeNav { object, field } => {
            format!("{}?.{}", format_expr_short(object), field)
        }
        ExprKind::SafeNavCall {
            object,
            method,
            args,
        } => {
            let a: Vec<String> = args.iter().map(format_expr_short).collect();
            format!(
                "{}?.{}({})",
                format_expr_short(object),
                method,
                a.join(", ")
            )
        }

        ExprKind::Call { callee, args, .. } => {
            let a: Vec<String> = args.iter().map(format_expr_short).collect();
            format!("{}({})", format_expr_short(callee), a.join(", "))
        }
        ExprKind::Index { object, index } => {
            format!(
                "{}[{}]",
                format_expr_short(object),
                format_expr_short(index)
            )
        }
        ExprKind::ClosureCall { callee, args } => {
            let a: Vec<String> = args.iter().map(format_expr_short).collect();
            format!("{}.call({})", format_expr_short(callee), a.join(", "))
        }

        ExprKind::Try(inner) => format!("{}?", format_expr_short(inner)),
        ExprKind::Await(inner) => format!("{}.await", format_expr_short(inner)),

        ExprKind::Assign { target, value } => {
            format!(
                "{} = {}",
                format_expr_short(target),
                format_expr_short(value)
            )
        }
        ExprKind::CompoundAssign { target, op, value } => {
            format!(
                "{} {:?}= {}",
                format_expr_short(target),
                op,
                format_expr_short(value)
            )
        }

        ExprKind::If(_) => "<if ...>".to_string(),
        ExprKind::IfLet(_) => "<if let ...>".to_string(),
        ExprKind::Match(_) => "<match ...>".to_string(),
        ExprKind::While(_) => "<while ...>".to_string(),
        ExprKind::WhileLet(_) => "<while let ...>".to_string(),
        ExprKind::For(_) => "<for ...>".to_string(),
        ExprKind::Loop(_) => "<loop ...>".to_string(),
        ExprKind::Block(_) => "<block>".to_string(),
        ExprKind::Closure(_) => "<closure>".to_string(),

        ExprKind::Range {
            start,
            end,
            inclusive,
        } => {
            let s = start
                .as_ref()
                .map(|e| format_expr_short(e))
                .unwrap_or_default();
            let e = end
                .as_ref()
                .map(|e| format_expr_short(e))
                .unwrap_or_default();
            let op = if *inclusive { "..=" } else { ".." };
            format!("{}{}{}", s, op, e)
        }

        ExprKind::ArrayLiteral(elems) => {
            if elems.len() <= 3 {
                let items: Vec<String> = elems.iter().map(format_expr_short).collect();
                format!("[{}]", items.join(", "))
            } else {
                format!("[...{} items]", elems.len())
            }
        }
        ExprKind::MapLiteral(entries) => {
            if entries.len() <= 2 {
                let pairs: Vec<String> = entries
                    .iter()
                    .map(|(k, v)| format!("{} => {}", format_expr_short(k), format_expr_short(v)))
                    .collect();
                format!("{{{}}}", pairs.join(", "))
            } else {
                format!("{{...{} entries}}", entries.len())
            }
        }
        ExprKind::ArrayFill { value, count } => {
            format!(
                "[{}; {}]",
                format_expr_short(value),
                format_expr_short(count)
            )
        }
        ExprKind::TupleLiteral(elems) => {
            let items: Vec<String> = elems.iter().map(format_expr_short).collect();
            format!("({})", items.join(", "))
        }

        ExprKind::Return(val) => match val {
            Some(v) => format!("return {}", format_expr_short(v)),
            None => "return".to_string(),
        },
        ExprKind::Break(val) => match val {
            Some(v) => format!("break {}", format_expr_short(v)),
            None => "break".to_string(),
        },
        ExprKind::Continue => "continue".to_string(),

        ExprKind::Yield(exprs) => {
            let items: Vec<String> = exprs.iter().map(format_expr_short).collect();
            format!("yield {}", items.join(", "))
        }

        ExprKind::MacroCall { name, args, .. } => {
            let a: Vec<String> = args.iter().map(format_expr_short).collect();
            format!("{}!({})", name, a.join(", "))
        }

        ExprKind::Cast { expr, target_type } => {
            format!(
                "{} as {}",
                format_expr_short(expr),
                format_type(target_type)
            )
        }

        ExprKind::EnumVariant {
            type_path,
            variant,
            args,
        } => {
            let path = type_path.join(".");
            if args.is_empty() {
                format!("{}.{}", path, variant)
            } else {
                let a: Vec<String> = args
                    .iter()
                    .map(|fa| {
                        fa.name
                            .as_ref()
                            .map(|n| format!("{}: {}", n, format_expr_short(&fa.value)))
                            .unwrap_or_else(|| format_expr_short(&fa.value))
                    })
                    .collect();
                format!("{}.{}({})", path, variant, a.join(", "))
            }
        }

        ExprKind::UnsafeBlock(_) => "unsafe ... end".to_string(),
        ExprKind::NullLiteral => "nil".to_string(),
    }
}

/// Format a pattern into a compact string.
pub fn format_pattern(p: &Pattern) -> String {
    match p {
        Pattern::Literal { expr, .. } => format_expr_short(expr),
        Pattern::Identifier { mutable, name, .. } => {
            if *mutable {
                format!("var {}", name)
            } else {
                name.clone()
            }
        }
        Pattern::Wildcard { .. } => "_".to_string(),
        Pattern::Tuple { elements, .. } => {
            let elems: Vec<String> = elements.iter().map(format_pattern).collect();
            format!("({})", elems.join(", "))
        }
        Pattern::Enum {
            path,
            variant,
            fields,
            ..
        } => {
            let base = if path.is_empty() {
                variant.clone()
            } else {
                format!("{}.{}", path.join("."), variant)
            };
            if fields.is_empty() {
                base
            } else {
                let fs: Vec<String> = fields.iter().map(format_pattern).collect();
                format!("{}({})", base, fs.join(", "))
            }
        }
        Pattern::Struct {
            path, fields, rest, ..
        } => {
            let base = path.join(".");
            let mut fs: Vec<String> = fields
                .iter()
                .map(|f| {
                    f.name
                        .as_ref()
                        .map(|n| format!("{}: {}", n, format_pattern(&f.pattern)))
                        .unwrap_or_else(|| format_pattern(&f.pattern))
                })
                .collect();
            if *rest {
                fs.push("..".to_string());
            }
            format!("{} {{ {} }}", base, fs.join(", "))
        }
        Pattern::Or { patterns, .. } => {
            let ps: Vec<String> = patterns.iter().map(format_pattern).collect();
            ps.join(" | ")
        }
        Pattern::Ref { mutable, name, .. } => {
            if *mutable {
                format!("ref var {}", name)
            } else {
                format!("ref {}", name)
            }
        }
        Pattern::Rest { .. } => "..".to_string(),
    }
}

fn format_numeric(v: f64, suffix: &Option<crate::lexer::token::NumericSuffix>) -> String {
    let base = if v == (v as i64) as f64 {
        format!("{}", v as i64)
    } else {
        format!("{}", v)
    };
    match suffix {
        Some(s) => format!("{}{:?}", base, s),
        None => base,
    }
}
