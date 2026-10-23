/// AST-to-Doc conversion for expressions.
use crate::lexer::token::NumericSuffix;
use crate::parser::ast::*;

use super::comments::CommentMap;
use super::doc::*;
use super::format_items::{dangling_comments_doc, with_comments};
use super::format_pattern::{format_match_pattern, format_pattern};
use super::format_type::format_type_expr;
use super::prec::{binop_prec, expr_prec, needs_parens, Side};

/// Format `child` as an operand of a parent operator, wrapping it in
/// grouping parentheses iff dropping them would re-associate or change
/// precedence on re-parse (Q34). `parent_prec` is the parent's tier
/// (`super::prec`); `side` selects the left/right associativity rule.
fn format_operand(
    child: &Expr,
    parent_prec: u8,
    side: Side,
    right_assoc: bool,
    comments: &CommentMap,
) -> Doc {
    let inner = format_expr(child, comments);
    if needs_parens(child, parent_prec, side, right_assoc) {
        concat(vec![text("("), inner, text(")")])
    } else {
        inner
    }
}

/// Format the receiver of a postfix form (`.field`, `.method()`, `[i]`,
/// `?`, `.await`). Postfix binds tighter than every operator, so an operator
/// receiver (`&var s`, `a | b`) keeps its grouping parentheses.
fn format_receiver(object: &Expr, comments: &CommentMap) -> Doc {
    let inner = format_expr(object, comments);
    if expr_prec(object).is_some() {
        concat(vec![text("("), inner, text(")")])
    } else {
        inner
    }
}

pub fn format_expr(expr: &Expr, comments: &CommentMap) -> Doc {
    format_expr_kind(&expr.kind, comments)
}

fn format_expr_kind(kind: &ExprKind, comments: &CommentMap) -> Doc {
    match kind {
        // ── Literals ──
        ExprKind::IntLiteral(val, suffix) => format_int(*val, suffix),
        ExprKind::FloatLiteral(val, suffix) => format_float(*val, suffix),
        ExprKind::StringLiteral(s) => text(format!("\"{}\"", escape_string(s))),
        ExprKind::InterpolatedString(parts) => format_interpolated_string(parts, comments),
        ExprKind::CharLiteral(c) => text(format_char_literal(*c)),
        ExprKind::BoolLiteral(b) => text(if *b { "true" } else { "false" }),
        ExprKind::UnitLiteral => text("()"),
        ExprKind::NullLiteral => text("nil"),
        ExprKind::RegexLiteral { pattern, flags } => text(format!("/{}/{}", pattern, flags)),
        // Block Tier-2 `&`-forms in argument position. `Forward` prints as the
        // canonical `&block` (bare `&` normalizes to it — both re-parse to
        // `Forward`, so `format(format(x)) == format(x)`).
        ExprKind::BlockPass(BlockPassKind::ToProc(sym)) => text(format!("&:{}", sym)),
        ExprKind::BlockPass(BlockPassKind::Forward) => text("&block"),

        // ── Identifiers ──
        ExprKind::Identifier(name) => text(name.clone()),
        ExprKind::SelfRef => text("self"),
        ExprKind::SelfType => text("Self"),

        // ── Operators ──
        ExprKind::BinaryOp { left, op, right } => format_binary_op(left, *op, right, comments),
        ExprKind::UnaryOp { op, operand } => format_unary_op(*op, operand, comments),

        // ── Borrowing ──
        ExprKind::Borrow(inner) => concat(vec![text("&"), format_expr(inner, comments)]),
        ExprKind::BorrowMut(inner) => concat(vec![text("&var "), format_expr(inner, comments)]),

        // ── Field/Method access ──
        // `@field` as written; the parser builds it as `self.field`.
        ExprKind::FieldAccess {
            field, ivar: true, ..
        } => text(format!("@{field}")),
        ExprKind::FieldAccess { object, field, .. } => {
            // Check if this is part of a method chain
            if is_chain_start(object) {
                format_method_chain(kind, comments)
            } else {
                concat(vec![
                    format_receiver(object, comments),
                    text("."),
                    text(field.clone()),
                ])
            }
        }

        ExprKind::MethodCall {
            parens,
            object,
            method,
            generic_args,
            args,
            block,
        } => {
            if is_chain_start(object) {
                format_method_chain(kind, comments)
            } else {
                format_single_method_call(
                    object,
                    method,
                    generic_args,
                    (args, *parens),
                    block.as_deref(),
                    comments,
                )
            }
        }

        ExprKind::SafeNav { object, field } => concat(vec![
            format_receiver(object, comments),
            text("&."),
            text(field.clone()),
        ]),

        ExprKind::SafeNavCall {
            object,
            method,
            args,
        } => {
            let arg_docs: Vec<Doc> = args.iter().map(|a| format_expr(a, comments)).collect();
            concat(vec![
                format_receiver(object, comments),
                text("&."),
                text(method.clone()),
                format_call_args(arg_docs),
            ])
        }

        // ── Calls & Indexing ──
        ExprKind::Call {
            parens,
            callee,
            args,
            block,
        } => {
            let arg_docs: Vec<Doc> = args.iter().map(|a| format_expr(a, comments)).collect();
            let mut parts = vec![format_expr(callee, comments)];
            parts.push(written_call_args(*parens, arg_docs));
            if let Some(blk) = block {
                parts.push(text(" "));
                parts.push(format_expr(blk, comments));
            }
            concat(parts)
        }

        ExprKind::Index { object, index } => concat(vec![
            format_receiver(object, comments),
            text("["),
            format_expr(index, comments),
            text("]"),
        ]),

        ExprKind::ClosureCall { callee, args } => {
            let arg_docs: Vec<Doc> = args.iter().map(|a| format_expr(a, comments)).collect();
            // Grouped so the separators break only when this call itself is
            // too long, not whenever an enclosing block breaks.
            group(concat(vec![
                format_expr(callee, comments),
                text(".("),
                join(concat(vec![text(","), line()]), arg_docs),
                text(")"),
            ]))
        }

        // ── Try operator ──
        ExprKind::Try(inner) => concat(vec![format_receiver(inner, comments), text("?")]),

        // ── Assignment ──
        ExprKind::Assign { target, value } => group(concat(vec![
            format_expr(target, comments),
            text(" = "),
            assigned_value(
                format_operand(value, 1, Side::Right, false, comments),
                value,
            ),
        ])),

        ExprKind::MultiAssign {
            targets,
            values,
            splat_target,
        } => {
            let render = |exprs: &[Expr], splat: Option<usize>| {
                join(
                    text(", "),
                    exprs
                        .iter()
                        .enumerate()
                        .map(|(index, expr)| {
                            let rendered = format_expr(expr, comments);
                            if splat == Some(index) {
                                concat(vec![text("*"), rendered])
                            } else {
                                rendered
                            }
                        })
                        .collect(),
                )
            };
            group(concat(vec![
                render(targets, *splat_target),
                text(" = "),
                nest(INDENT_WIDTH, render(values, None)),
            ]))
        }

        ExprKind::CompoundAssign { target, op, value } => group(concat(vec![
            format_expr(target, comments),
            text(" "),
            text(compound_assign_op_str(*op)),
            text(" "),
            nest(
                INDENT_WIDTH,
                format_operand(value, 1, Side::Right, false, comments),
            ),
        ])),

        // ── Control flow ──
        ExprKind::If(if_expr) => format_if_expr(if_expr, comments),
        ExprKind::IfLet(if_let) => format_if_let_expr(if_let, comments),
        ExprKind::Match(match_expr) => format_match_expr(match_expr, comments),
        ExprKind::While(w) => format_while_expr(w, comments),
        ExprKind::WhileLet(wl) => format_while_let_expr(wl, comments),
        ExprKind::For(f) => format_for_expr(f, comments),
        ExprKind::Loop(l) => format_loop_expr(l, comments),

        // ── Blocks & closures ──
        ExprKind::Block(block) => format_block(block, comments),
        ExprKind::Closure(closure) => format_closure(closure, comments),

        // ── Range ──
        ExprKind::Range {
            start,
            end,
            inclusive,
        } => {
            // ruby-naming.spec.md §3.10b: ranges are Ruby-shaped —
            // `..` is INCLUSIVE, `...` is EXCLUSIVE. The `..=` spelling is
            // not valid Ruxen syntax and fails to re-lex (E0009).
            let op = if *inclusive { ".." } else { "..." };
            // Range is tier 9, non-associative. Wrap an endpoint that is a
            // looser (or, on either side, equal-tier) operator so a nested
            // range keeps its grouping on re-parse.
            let mut parts = Vec::new();
            if let Some(s) = start {
                parts.push(format_operand(s, 9, Side::Left, false, comments));
            }
            parts.push(text(op));
            if let Some(e) = end {
                parts.push(format_operand(e, 9, Side::Right, false, comments));
            }
            concat(parts)
        }

        // ── Collection literals ──
        ExprKind::ArrayLiteral(elems) => {
            if elems.is_empty() {
                text("[]")
            } else {
                let items: Vec<Doc> = elems.iter().map(|e| format_expr(e, comments)).collect();
                group(concat(vec![
                    text("["),
                    nest(
                        INDENT_WIDTH,
                        concat(vec![
                            softline(),
                            join(concat(vec![text(","), line()]), items),
                            if_break(text(","), nil()),
                        ]),
                    ),
                    softline(),
                    text("]"),
                ]))
            }
        }

        ExprKind::MapLiteral(entries) => {
            if entries.is_empty() {
                text("{}")
            } else {
                let items: Vec<Doc> = entries
                    .iter()
                    .map(|(k, v)| {
                        concat(vec![
                            format_expr(k, comments),
                            text(" => "),
                            format_expr(v, comments),
                        ])
                    })
                    .collect();
                group(concat(vec![
                    text("{"),
                    nest(
                        INDENT_WIDTH,
                        concat(vec![
                            softline(),
                            join(concat(vec![text(","), line()]), items),
                            if_break(text(","), nil()),
                        ]),
                    ),
                    softline(),
                    text("}"),
                ]))
            }
        }

        ExprKind::ArrayFill { value, count } => concat(vec![
            text("["),
            format_expr(value, comments),
            text("; "),
            format_expr(count, comments),
            text("]"),
        ]),

        ExprKind::TupleLiteral(elems) => {
            if elems.is_empty() {
                text("()")
            } else {
                let items: Vec<Doc> = elems.iter().map(|e| format_expr(e, comments)).collect();
                group(concat(vec![
                    text("("),
                    nest(
                        INDENT_WIDTH,
                        concat(vec![
                            softline(),
                            join(concat(vec![text(","), line()]), items),
                            if_break(text(","), nil()),
                        ]),
                    ),
                    softline(),
                    text(")"),
                ]))
            }
        }

        // ── Jump expressions ──
        ExprKind::Return(val) => match val {
            Some(e) => group(concat(vec![
                text("return "),
                nest(INDENT_WIDTH, format_expr(e, comments)),
            ])),
            None => text("return"),
        },

        ExprKind::Break(val) => match val {
            Some(e) => group(concat(vec![
                text("break "),
                nest(INDENT_WIDTH, format_expr(e, comments)),
            ])),
            None => text("break"),
        },

        ExprKind::Next(val) => match val {
            Some(e) => group(concat(vec![
                text("next "),
                nest(INDENT_WIDTH, format_expr(e, comments)),
            ])),
            None => text("next"),
        },

        ExprKind::Continue => text("continue"),

        ExprKind::Await(inner) => concat(vec![format_receiver(inner, comments), text(".await")]),

        // ── Yield ──
        ExprKind::Yield(exprs) => {
            let arg_docs: Vec<Doc> = exprs.iter().map(|e| format_expr(e, comments)).collect();
            if arg_docs.is_empty() {
                text("yield")
            } else {
                concat(vec![
                    text("yield "),
                    join(concat(vec![text(","), space()]), arg_docs),
                ])
            }
        }

        // ── Macros ──
        ExprKind::MacroCall {
            name,
            args,
            delimiter,
        } => {
            let arg_docs: Vec<Doc> = args.iter().map(|a| format_expr(a, comments)).collect();
            let (open, close) = match delimiter {
                MacroDelimiter::Paren => ("(", ")"),
                MacroDelimiter::Bracket => ("[", "]"),
                MacroDelimiter::Brace => ("{", "}"),
            };
            concat(vec![
                text(format!("{}!", name)),
                group(concat(vec![
                    text(open),
                    nest(
                        INDENT_WIDTH,
                        concat(vec![
                            softline(),
                            join(concat(vec![text(","), line()]), arg_docs),
                            if_break(text(","), nil()),
                        ]),
                    ),
                    softline(),
                    text(close),
                ])),
            ])
        }

        // ── Cast ──
        // `as` is tier 23. The cast subject is its left operand: wrap it
        // when it is a looser operator so `(a + b) as Int` does not collapse
        // to `a + b as Int` = `a + (b as Int)` (Q34).
        ExprKind::Cast { expr, target_type } => concat(vec![
            format_operand(expr, 23, Side::Left, false, comments),
            text(" as "),
            format_type_expr(target_type, comments),
        ]),

        // A type used as a static-call receiver with explicit generic
        // arguments (`Hash[String, Int].new`). Print the written path, not
        // the bare name: dropping the arguments here would silently rewrite
        // the program into the shape whose type inference cannot recover
        // them, which is the fmt-destructiveness class (Q23/Q30/Q34).
        ExprKind::GenericTypeRef { name: _, ty } => format_type_expr(ty, comments),

        // ── Enum variant construction ──
        ExprKind::EnumVariant {
            type_path,
            variant,
            args,
        } => {
            let path_str = if type_path.is_empty() {
                variant.clone()
            } else {
                format!("{}.{}", type_path.join("."), variant)
            };

            if args.is_empty() {
                text(path_str)
            } else {
                let arg_docs: Vec<Doc> = args
                    .iter()
                    .map(
                        |FieldArg {
                             name,
                             value,
                             span: _,
                         }| {
                            if let Some(name) = name {
                                concat(vec![
                                    text(name.clone()),
                                    text(": "),
                                    format_expr(value, comments),
                                ])
                            } else {
                                format_expr(value, comments)
                            }
                        },
                    )
                    .collect();
                concat(vec![text(path_str), format_call_args(arg_docs)])
            }
        }

        // ── Unsafe block ──
        ExprKind::UnsafeBlock(block) => {
            let body = format_block_body(block, comments);
            concat(vec![
                text("unsafe"),
                nest(INDENT_WIDTH, concat(vec![hardline(), body])),
                hardline(),
                text("end"),
            ])
        }
    }
}

// ─── Helpers ────────────────────────────────────────────────────────

fn format_int(val: i64, suffix: &Option<NumericSuffix>) -> Doc {
    match suffix {
        Some(s) => text(format!("{}{}", val, s.as_str())),
        None => text(format!("{}", val)),
    }
}

fn format_float(val: f64, suffix: &Option<NumericSuffix>) -> Doc {
    let s = if val.fract() == 0.0 && !format!("{}", val).contains('.') {
        format!("{}.0", val)
    } else {
        format!("{}", val)
    };
    match suffix {
        Some(sf) => text(format!("{}{}", s, sf.as_str())),
        None => text(s),
    }
}

/// Source form of string-literal text. A literal `#{` is written `\#{` so it
/// does not re-lex as an interpolation.
fn escape_string(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '#' if chars.peek() == Some(&'{') => out.push_str("\\#"),
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

/// Render a `Char` literal in the Ruby-style `?` form
/// (ruby-naming.spec.md §3.11). Single quotes are RAW STRINGS now, so the
/// old `'c'` spelling would re-parse as a `&str`. The lexer only accepts a
/// bare char after `?` when it is alphanumeric or `_`; named escapes
/// (`?\n`, `?\t`, `?\r`, `?\\`, `?\'`) cover the common control chars, and
/// everything else (spaces, punctuation, non-ASCII) goes through the
/// `?\u{hex}` escape so the output always re-lexes to the same `CharLiteral`.
fn format_char_literal(c: char) -> String {
    match c {
        '\\' => "?\\\\".to_string(),
        '\'' => "?\\'".to_string(),
        '\n' => "?\\n".to_string(),
        '\t' => "?\\t".to_string(),
        '\r' => "?\\r".to_string(),
        '\0' => "?\\u{0}".to_string(),
        c if c.is_ascii_alphanumeric() || c == '_' => format!("?{}", c),
        c => format!("?\\u{{{:X}}}", c as u32),
    }
}

/// An interpolated string, quotes included. Each `#{...}` expression is
/// formatted with `format_expr` and rendered flat, since the string must
/// stay on one line; the format spec is re-emitted when non-default.
fn format_interpolated_string(parts: &[StringPart], comments: &CommentMap) -> Doc {
    let mut out = String::from("\"");
    for part in parts {
        match part {
            StringPart::Literal(s) => out.push_str(&escape_string(s)),
            StringPart::Expr { expr, spec } => {
                let inner = print_doc(&format_expr(expr, comments), i32::MAX);
                out.push_str(&format!("#{{{}{}}}", inner, format_spec_to_source(spec)));
            }
        }
    }
    out.push('"');
    text(out)
}

/// Phase 2 #06.B: reconstruct a `FormatSpec` to its source form.
/// Returns `""` for the default spec (so bare `"#{x}"` round-trips
/// unchanged), or `:fill_align_width.precision?` for non-default.
fn format_spec_to_source(spec: &crate::lexer::token::FormatSpec) -> String {
    if spec.is_default() {
        return String::new();
    }
    let mut out = String::from(":");
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
    out
}

fn compound_assign_op_str(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+=",
        BinOp::Sub => "-=",
        BinOp::Mul => "*=",
        BinOp::Div => "/=",
        BinOp::Mod => "%=",
        BinOp::Shl => "<<=",
        BinOp::Shr => ">>=",
        BinOp::BitAnd => "&=",
        BinOp::BitOr => "|=",
        BinOp::BitXor => "^=",
        _ => "=",
    }
}

fn format_binary_op(left: &Expr, op: BinOp, right: &Expr, comments: &CommentMap) -> Doc {
    // Re-parenthesise operands whose precedence is looser than this
    // operator's (or equal, on the right) so the formatted output re-parses
    // to the SAME tree — the parser discards grouping parens, so the
    // formatter must re-derive them from tree shape (Q34).
    let p = binop_prec(op);
    let right_assoc = matches!(op, BinOp::Pow | BinOp::NilCoalesce);
    group(concat(vec![
        format_operand(left, p, Side::Left, right_assoc, comments),
        text(" "),
        text(op.symbol()),
        line(),
        format_operand(right, p, Side::Right, right_assoc, comments),
    ]))
}

fn format_unary_op(op: UnaryOp, operand: &Expr, comments: &CommentMap) -> Doc {
    if op == UnaryOp::KeywordNot {
        return concat(vec![
            text("not "),
            // Assignment shares this tier and is consumed by `not`; keyword
            // `and` / `or` are looser and therefore require grouping.
            format_operand(operand, 1, Side::Right, true, comments),
        ]);
    }

    let op_str = match op {
        UnaryOp::Neg => "-",
        UnaryOp::Not => "!",
        UnaryOp::BitNot => "~",
        UnaryOp::KeywordNot => unreachable!(),
        UnaryOp::Deref => "*",
    };
    // A prefix unary binds at tier 25 — tighter than every binary/range/cast
    // operator — so its operand must be parenthesised when it is a looser (or
    // equal-tier) operator: `-(a + b)`, `-(a as Int)`, `!(a && b)`. Without
    // this, `-(a + b)` would reformat to `-a + b` = `(-a) + b` (Q34).
    concat(vec![
        text(op_str),
        format_operand(operand, 25, Side::Right, false, comments),
    ])
}

/// Format function call arguments with line-breaking.
pub fn format_call_args(args: Vec<Doc>) -> Doc {
    if args.is_empty() {
        return text("()");
    }

    group(concat(vec![
        text("("),
        nest(
            INDENT_WIDTH,
            concat(vec![
                softline(),
                join(concat(vec![text(","), line()]), args),
                if_break(text(","), nil()),
            ]),
        ),
        softline(),
        text(")"),
    ]))
}

// ─── Method Chains ──────────────────────────────────────────────────

/// Check if an expression is the start of a method chain (has nested
/// FieldAccess/MethodCall in the object position).
fn is_chain_start(expr: &Expr) -> bool {
    matches!(
        expr.kind,
        ExprKind::MethodCall { .. } | ExprKind::FieldAccess { .. }
    )
}

/// Collect all the chain links from a chain expression.
fn collect_chain(kind: &ExprKind) -> (Doc, Vec<Doc>) {
    // We need to store intermediate data, not Doc references
    let mut links: Vec<ExprKind> = Vec::new();
    let mut current = kind;
    let mut base: Option<&Expr> = None;

    // An `@field` read is the chain's receiver, not a `.field` link.
    while let ExprKind::MethodCall { object, .. }
    | ExprKind::FieldAccess {
        object,
        ivar: false,
        ..
    } = current
    {
        links.push(current.clone());
        base = Some(object);
        current = &object.kind;
    }

    // `base` is now the receiver of the chain.
    let comments = CommentMap::new();
    let receiver = match base {
        Some(base) => format_receiver(base, &comments),
        None => format_expr_kind(current, &comments),
    };

    // Links are in reverse order (innermost first), so reverse.
    links.reverse();
    let link_docs: Vec<Doc> = links
        .iter()
        .map(|link| match link {
            ExprKind::FieldAccess {
                object: _, field, ..
            } => concat(vec![text("."), text(field.clone())]),
            ExprKind::MethodCall {
                parens,
                object: _,
                method,
                generic_args,
                args,
                block,
            } => {
                let arg_docs: Vec<Doc> = args.iter().map(|a| format_expr(a, &comments)).collect();
                let mut parts = vec![
                    text("."),
                    text(method.clone()),
                    format_method_generic_args(generic_args, &comments),
                ];
                // The argument list as written (see `written_call_args`).
                parts.push(written_call_args(*parens, arg_docs));
                if let Some(blk) = block {
                    parts.push(text(" "));
                    parts.push(format_expr(blk, &comments));
                }
                concat(parts)
            }
            _ => nil(),
        })
        .collect();

    (receiver, link_docs)
}

fn format_method_chain(kind: &ExprKind, _comments: &CommentMap) -> Doc {
    let (receiver, links) = collect_chain(kind);

    if links.len() <= 1 {
        // Single method call — not really a chain, just format inline.
        return concat(std::iter::once(receiver).chain(links).collect());
    }

    // Try flat first, break at each `.` if too long.
    group(concat(vec![
        receiver,
        nest(
            INDENT_WIDTH,
            concat(
                links
                    .into_iter()
                    .map(|link| concat(vec![softline(), link]))
                    .collect(),
            ),
        ),
    ]))
}

/// A call's argument list the way the source wrote it. With `parens` it is
/// always `(...)`, even when empty: `f()` and `x.m()` are calls, and dropping
/// the parens would turn them into a name or a field access (Q30). Without
/// parens it is the command form, ` a` (the parser takes one bare
/// argument), or nothing for a block-only call (`x.map { }`) and a bare
/// `x.m[T]`; fmt never adds parens the author left out.
fn written_call_args(parens: bool, arg_docs: Vec<Doc>) -> Doc {
    if parens {
        format_call_args(arg_docs)
    } else if arg_docs.is_empty() {
        nil()
    } else {
        concat(vec![text(" "), join(text(", "), arg_docs)])
    }
}

/// The `[T, U]` explicit type arguments of a method call, or nothing.
fn format_method_generic_args(generic_args: &[TypeExpr], comments: &CommentMap) -> Doc {
    if generic_args.is_empty() {
        return nil();
    }
    let docs = generic_args
        .iter()
        .map(|t| format_type_expr(t, comments))
        .collect();
    concat(vec![text("["), join(text(", "), docs), text("]")])
}

fn format_single_method_call(
    object: &Expr,
    method: &str,
    generic_args: &[TypeExpr],
    (args, parens): (&[Expr], bool),
    block: Option<&Expr>,
    comments: &CommentMap,
) -> Doc {
    let arg_docs: Vec<Doc> = args.iter().map(|a| format_expr(a, comments)).collect();
    let mut parts = vec![
        format_receiver(object, comments),
        text("."),
        text(method.to_string()),
        format_method_generic_args(generic_args, comments),
    ];
    parts.push(written_call_args(parens, arg_docs));
    if let Some(blk) = block {
        parts.push(text(" "));
        parts.push(format_expr(blk, comments));
    }
    concat(parts)
}

// ─── Control Flow ───────────────────────────────────────────────────

fn format_if_expr(if_expr: &IfExpr, comments: &CommentMap) -> Doc {
    let IfExpr {
        condition,
        then_body,
        elsif_clauses,
        else_body,
        span: _,
    } = if_expr;
    let mut parts = vec![
        text("if "),
        format_expr(condition, comments),
        indented_block(then_body, comments),
    ];

    for ElsifClause {
        condition,
        body,
        span: _,
    } in elsif_clauses
    {
        parts.push(hardline());
        parts.push(text("elsif "));
        parts.push(format_expr(condition, comments));
        parts.push(indented_block(body, comments));
    }

    if let Some(else_body) = else_body {
        parts.push(hardline());
        parts.push(text("else"));
        parts.push(indented_block(else_body, comments));
    }

    parts.push(hardline());
    parts.push(text("end"));

    concat(parts)
}

fn format_if_let_expr(if_let: &IfLetExpr, comments: &CommentMap) -> Doc {
    let IfLetExpr {
        pattern,
        value,
        then_body,
        else_body,
        span: _,
    } = if_let;
    let mut parts = vec![
        text("if let "),
        format_pattern(pattern, comments),
        text(" = "),
        format_expr(value, comments),
        indented_block(then_body, comments),
    ];

    if let Some(else_body) = else_body {
        parts.push(hardline());
        parts.push(text("else"));
        parts.push(indented_block(else_body, comments));
    }

    parts.push(hardline());
    parts.push(text("end"));

    concat(parts)
}

fn format_match_expr(match_expr: &MatchExpr, comments: &CommentMap) -> Doc {
    let MatchExpr {
        subject,
        arms,
        span: _,
    } = match_expr;
    let mut parts = vec![text("match "), format_expr(subject, comments)];

    let arm_docs: Vec<Doc> = arms
        .iter()
        .map(|arm| with_comments(arm.span.start, format_match_arm(arm, comments), comments))
        .collect();

    parts.push(nest(
        INDENT_WIDTH,
        concat(
            arm_docs
                .into_iter()
                .map(|a| concat(vec![hardline(), a]))
                .collect(),
        ),
    ));

    parts.push(hardline());
    parts.push(text("end"));

    concat(parts)
}

fn format_match_arm(arm: &MatchArm, comments: &CommentMap) -> Doc {
    let MatchArm {
        pattern,
        guard,
        body,
        span: _,
    } = arm;
    let pattern_doc = format_match_pattern(pattern, guard.as_deref(), comments);

    match body {
        MatchArmBody::Expr(expr) => {
            // Single expression: Pattern -> expr
            // The arm's span runs to the next arm, so a comment at the end
            // of the arm line trails the body expression.
            group(concat(vec![
                pattern_doc,
                text(" -> "),
                with_comments(expr.span.start, format_expr(expr, comments), comments),
            ]))
        }
        MatchArmBody::Block(block) => {
            if block.statements.is_empty() {
                // Empty block
                concat(vec![pattern_doc, text(" ->")])
            } else if block.statements.len() == 1 {
                // Single statement — try inline
                let body_doc = format_block_body(block, comments);
                group(concat(vec![pattern_doc, text(" -> "), body_doc]))
            } else {
                // Multi-statement arm: break after `->` and indent the body.
                // Match arms have NO closing `end` — the parser
                // (`parse_match_arm_body`) collects statements until the next
                // sibling arm header or the match's `end`. Emitting a per-arm
                // `end` injects a spurious delimiter that breaks the re-parse.
                let body_doc = format_block_body(block, comments);
                concat(vec![
                    pattern_doc,
                    text(" ->"),
                    nest(INDENT_WIDTH, concat(vec![hardline(), body_doc])),
                ])
            }
        }
    }
}

fn format_while_expr(w: &WhileExpr, comments: &CommentMap) -> Doc {
    let WhileExpr {
        condition,
        body,
        span: _,
    } = w;
    concat(vec![
        text("while "),
        format_expr(condition, comments),
        indented_block(body, comments),
        hardline(),
        text("end"),
    ])
}

fn format_while_let_expr(wl: &WhileLetExpr, comments: &CommentMap) -> Doc {
    let WhileLetExpr {
        pattern,
        value,
        body,
        span: _,
    } = wl;
    concat(vec![
        text("while let "),
        format_pattern(pattern, comments),
        text(" = "),
        format_expr(value, comments),
        indented_block(body, comments),
        hardline(),
        text("end"),
    ])
}

fn format_for_expr(f: &ForExpr, comments: &CommentMap) -> Doc {
    let ForExpr {
        pattern,
        iterable,
        body,
        span: _,
    } = f;
    concat(vec![
        text("for "),
        format_pattern(pattern, comments),
        text(" in "),
        format_expr(iterable, comments),
        indented_block(body, comments),
        hardline(),
        text("end"),
    ])
}

fn format_loop_expr(l: &LoopExpr, comments: &CommentMap) -> Doc {
    let LoopExpr { body, span: _ } = l;
    concat(vec![
        text("loop"),
        indented_block(body, comments),
        hardline(),
        text("end"),
    ])
}

// ─── Blocks ─────────────────────────────────────────────────────────

pub fn format_block(block: &Block, comments: &CommentMap) -> Doc {
    let Block {
        statements,
        span: _,
    } = block;
    // `ExprKind::Block` is a synthesized `do ... end` block (e.g. async
    // lowering's tail blocks). The `do`/`end` delimiters are part of the
    // surface syntax — emitting only the inner statements collapses the block
    // into its surrounding context and no longer parses.
    if statements.is_empty() {
        return concat(vec![text("do"), hardline(), text("end")]);
    }
    concat(vec![
        text("do"),
        indented_block(block, comments),
        hardline(),
        text("end"),
    ])
}

/// A block body on its own indented lines under its header, or nothing for
/// an empty body (no statements, no comments), so `if c` / `else` with
/// nothing between them gets no blank line.
fn indented_block(block: &Block, comments: &CommentMap) -> Doc {
    if block.statements.is_empty() && !comments.has_dangling(block.span.start) {
        nil()
    } else {
        nest(
            INDENT_WIDTH,
            concat(vec![hardline(), format_block_body(block, comments)]),
        )
    }
}

pub fn format_block_body(block: &Block, comments: &CommentMap) -> Doc {
    let Block { statements, span } = block;
    let mut parts = Vec::new();
    for (i, stmt) in statements.iter().enumerate() {
        let start = match stmt {
            Statement::Let(l) => l.span.start,
            Statement::Expression(e) => e.span.start,
        };
        if i > 0 {
            parts.push(hardline());
            // Keep one blank line the author left between two statements.
            if comments.blank_line_before(start) {
                parts.push(hardline());
            }
        }
        parts.push(with_comments(
            start,
            format_statement(stmt, comments),
            comments,
        ));
    }
    if let Some(tail) = dangling_comments_doc(span.start, comments) {
        if !parts.is_empty() {
            parts.push(hardline());
        }
        parts.push(tail);
    }
    concat(parts)
}

pub fn format_statement(stmt: &Statement, comments: &CommentMap) -> Doc {
    match stmt {
        Statement::Let(l) => format_let_binding(l, comments),
        Statement::Expression(e) => format_expr(e, comments),
    }
}

fn format_let_binding(binding: &LetBinding, comments: &CommentMap) -> Doc {
    let LetBinding {
        mutable,
        pattern,
        type_annotation,
        value,
        span: _,
    } = binding;
    // §3.1: `var x = ...` is the writable binding form; `let mut x` is retired.
    let head = if *mutable { "var " } else { "let " };
    let mut parts = vec![text(head)];

    parts.push(format_pattern(pattern, comments));

    if let Some(ty) = type_annotation {
        parts.push(text(": "));
        parts.push(format_type_expr(ty, comments));
    }

    if let Some(val) = value {
        parts.push(text(" = "));
        parts.push(assigned_value(format_expr(val, comments), val));
    }

    group(concat(parts))
}

/// The right side of `=`. A value that wraps onto continuation lines is
/// indented one step; a block form (`match`, `if`, a call with a `do ... end`
/// block) keeps its body and `end` aligned with the statement, as written:
///
/// ```text
/// let x = match y
///   ...
/// end
/// ```
fn assigned_value(doc: Doc, value: &Expr) -> Doc {
    if ends_in_block_form(value) {
        doc
    } else {
        nest(INDENT_WIDTH, doc)
    }
}

fn ends_in_block_form(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::If(_)
        | ExprKind::IfLet(_)
        | ExprKind::Match(_)
        | ExprKind::While(_)
        | ExprKind::WhileLet(_)
        | ExprKind::For(_)
        | ExprKind::Loop(_)
        | ExprKind::Block(_)
        | ExprKind::UnsafeBlock(_)
        | ExprKind::Closure(_) => true,
        ExprKind::Call { block, .. } | ExprKind::MethodCall { block, .. } => block.is_some(),
        _ => false,
    }
}

// ─── Closures ───────────────────────────────────────────────────────

pub fn format_closure(closure: &ClosureExpr, comments: &CommentMap) -> Doc {
    let ClosureExpr {
        is_async,
        is_move,
        params,
        body,
        span,
        pipes,
        do_end,
        kind,
        spelling,
    } = closure;
    // A bare closure that is not a block is a lambda value: it prints as
    // `->(x) { ... }`, the spelling that cannot be misread as a block.
    let spelling = match spelling {
        ClosureSpelling::Bare if *kind != ClosureKind::Block => ClosureSpelling::Stabby,
        other => *other,
    };
    let has_header = *pipes || !params.is_empty();
    // The parameter header as written: `|x|`, `||`, or none at all; a `->`
    // lambda writes `(x)` / `()` before its body instead.
    let header = if has_header && spelling != ClosureSpelling::Stabby {
        concat(vec![format_closure_params(params, comments), text(" ")])
    } else {
        nil()
    };
    let head = match spelling {
        ClosureSpelling::Bare => nil(),
        ClosureSpelling::Stabby if has_header => concat(vec![
            text("->("),
            format_closure_param_list(params, comments),
            text(") "),
        ]),
        ClosureSpelling::Stabby => text("-> "),
        ClosureSpelling::Lambda => text("lambda "),
        ClosureSpelling::Proc => text("proc "),
        ClosureSpelling::ProcNew => text("Proc.new "),
    };

    // `async` / `move` modifiers (parser keyword order is `async move`). A move
    // closure written `{ |x| ... }` without the `move` keyword does not re-parse
    // at expression position — the leading `{` is taken as a block — so the
    // modifier must be preserved.
    let mut prefix: Vec<Doc> = Vec::new();
    if *is_async {
        prefix.push(text("async "));
    }
    if *is_move {
        prefix.push(text("move "));
    }
    let prefix_doc = concat(prefix);

    // The author's form (`do ... end` or `{ ... }`) and line layout: a body
    // written on the opening line stays there, any other body gets its own
    // indented lines.
    let (open, close) = if *do_end { ("do", "end") } else { ("{", "}") };
    let one_line = match body {
        ClosureBody::Expr(expr) => expr.span.line == span.line,
        ClosureBody::Block(block) => match block.statements.as_slice() {
            [] => true,
            [only] => statement_line(only) == span.line,
            _ => false,
        },
    };
    let body_doc = match body {
        ClosureBody::Expr(expr) => format_expr(expr, comments),
        ClosureBody::Block(block) => format_block_body(block, comments),
    };
    let empty = match body {
        ClosureBody::Expr(expr) => matches!(expr.kind, ExprKind::UnitLiteral),
        ClosureBody::Block(block) => block.statements.is_empty(),
    };
    let body = if empty {
        concat(vec![text(open), text(" "), header, text(close)])
    } else if one_line {
        concat(vec![
            text(open),
            text(" "),
            header,
            body_doc,
            text(" "),
            text(close),
        ])
    } else {
        concat(vec![
            text(open),
            text(" "),
            header,
            nest(INDENT_WIDTH, concat(vec![hardline(), body_doc])),
            hardline(),
            text(close),
        ])
    };

    concat(vec![prefix_doc, head, body])
}

/// The source line a statement starts on.
fn statement_line(stmt: &Statement) -> u32 {
    match stmt {
        Statement::Let(l) => l.span.line,
        Statement::Expression(e) => e.span.line,
    }
}

fn format_closure_params(params: &[ClosureParam], comments: &CommentMap) -> Doc {
    if params.is_empty() {
        // An empty header the author wrote stays `||`: `{ || App.build(...) }`
        // collapsing to `{ App.build(...) }` re-parses as a block, not a
        // closure, in expression position (Q30). `format_closure` asks for
        // the header only when the source had one (`ClosureExpr::pipes`).
        return text("||");
    }
    concat(vec![
        text("|"),
        format_closure_param_list(params, comments),
        text("|"),
    ])
}

/// `x, y: Int` — closure parameters without their delimiters.
fn format_closure_param_list(params: &[ClosureParam], comments: &CommentMap) -> Doc {
    let param_docs: Vec<Doc> = params
        .iter()
        .map(
            |ClosureParam {
                 name,
                 type_expr,
                 span: _,
             }| {
                if let Some(ty) = type_expr {
                    concat(vec![
                        text(name.clone()),
                        text(": "),
                        format_type_expr(ty, comments),
                    ])
                } else {
                    text(name.clone())
                }
            },
        )
        .collect();
    join(concat(vec![text(","), space()]), param_docs)
}
