/// AST-to-Doc conversion for expressions.
use crate::lexer::token::{NumericSuffix, StringPart};
use crate::parser::ast::*;

use super::comments::CommentMap;
use super::doc::*;
use super::format_pattern::{format_match_pattern, format_pattern};
use super::format_type::format_type_expr;

pub fn format_expr(expr: &Expr, comments: &CommentMap) -> Doc {
    format_expr_kind(&expr.kind, comments)
}

fn format_expr_kind(kind: &ExprKind, comments: &CommentMap) -> Doc {
    match kind {
        // ── Literals ──
        ExprKind::IntLiteral(val, suffix) => format_int(*val, suffix),
        ExprKind::FloatLiteral(val, suffix) => format_float(*val, suffix),
        ExprKind::StringLiteral(s) => text(format!("\"{}\"", escape_string(s))),
        ExprKind::InterpolatedString(parts) => format_interpolated_string(parts),
        ExprKind::CharLiteral(c) => text(format_char_literal(*c)),
        ExprKind::BoolLiteral(b) => text(if *b { "true" } else { "false" }),
        ExprKind::UnitLiteral => text("()"),
        ExprKind::NullLiteral => text("nil"),
        ExprKind::RegexLiteral { pattern, flags } => text(format!("/{}/{}", pattern, flags)),

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
        ExprKind::FieldAccess { object, field } => {
            // Check if this is part of a method chain
            if is_chain_start(object) {
                format_method_chain(kind, comments)
            } else {
                concat(vec![
                    format_expr(object, comments),
                    text("."),
                    text(field.clone()),
                ])
            }
        }

        ExprKind::MethodCall {
            object,
            method,
            args,
            block,
            ..
        } => {
            if is_chain_start(object) {
                format_method_chain(kind, comments)
            } else {
                format_single_method_call(object, method, args, block.as_deref(), comments)
            }
        }

        ExprKind::SafeNav { object, field } => concat(vec![
            format_expr(object, comments),
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
                format_expr(object, comments),
                text("&."),
                text(method.clone()),
                format_call_args(arg_docs),
            ])
        }

        // ── Calls & Indexing ──
        ExprKind::Call {
            callee,
            args,
            block,
        } => {
            let arg_docs: Vec<Doc> = args.iter().map(|a| format_expr(a, comments)).collect();
            let mut parts = vec![format_expr(callee, comments)];
            // Emit parens if there are args, a block, or if the callee isn't
            // a simple identifier (e.g., `Foo.new` with no args can omit parens).
            if !arg_docs.is_empty() || block.is_some() {
                parts.push(format_call_args(arg_docs));
            }
            if let Some(blk) = block {
                parts.push(text(" "));
                parts.push(format_expr(blk, comments));
            }
            concat(parts)
        }

        ExprKind::Index { object, index } => concat(vec![
            format_expr(object, comments),
            text("["),
            format_expr(index, comments),
            text("]"),
        ]),

        ExprKind::ClosureCall { callee, args } => {
            let arg_docs: Vec<Doc> = args.iter().map(|a| format_expr(a, comments)).collect();
            concat(vec![
                format_expr(callee, comments),
                text(".("),
                join(concat(vec![text(","), line()]), arg_docs),
                text(")"),
            ])
        }

        // ── Try operator ──
        ExprKind::Try(inner) => concat(vec![format_expr(inner, comments), text("?")]),

        // ── Assignment ──
        ExprKind::Assign { target, value } => group(concat(vec![
            format_expr(target, comments),
            text(" = "),
            nest(INDENT_WIDTH, format_expr(value, comments)),
        ])),

        ExprKind::CompoundAssign { target, op, value } => group(concat(vec![
            format_expr(target, comments),
            text(" "),
            text(compound_assign_op_str(*op)),
            text(" "),
            nest(INDENT_WIDTH, format_expr(value, comments)),
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
            let mut parts = Vec::new();
            if let Some(s) = start {
                parts.push(format_expr(s, comments));
            }
            parts.push(text(op));
            if let Some(e) = end {
                parts.push(format_expr(e, comments));
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

        ExprKind::Continue => text("continue"),

        ExprKind::Await(inner) => concat(vec![format_expr(inner, comments), text(".await")]),

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
        ExprKind::Cast { expr, target_type } => concat(vec![
            format_expr(expr, comments),
            text(" as "),
            format_type_expr(target_type, comments),
        ]),

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
                    .map(|a| {
                        if let Some(name) = &a.name {
                            concat(vec![
                                text(name.clone()),
                                text(": "),
                                format_expr(&a.value, comments),
                            ])
                        } else {
                            format_expr(&a.value, comments)
                        }
                    })
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
        Some(s) => text(format!("{}{}", val, suffix_str(s))),
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
        Some(sf) => text(format!("{}{}", s, suffix_str(sf))),
        None => text(s),
    }
}

fn suffix_str(s: &NumericSuffix) -> &'static str {
    match s {
        NumericSuffix::I8 => "i8",
        NumericSuffix::I16 => "i16",
        NumericSuffix::I32 => "i32",
        NumericSuffix::I64 => "i64",
        NumericSuffix::U => "u",
        NumericSuffix::U8 => "u8",
        NumericSuffix::U16 => "u16",
        NumericSuffix::U32 => "u32",
        NumericSuffix::U64 => "u64",
        NumericSuffix::ISize => "isize",
        NumericSuffix::USize => "usize",
        NumericSuffix::F32 => "f32",
        NumericSuffix::F64 => "f64",
    }
}

fn escape_string(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
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

fn format_interpolated_string(parts: &[StringPart]) -> Doc {
    let mut segments = vec![text("\"")];
    for part in parts {
        match part {
            StringPart::Literal(s) => segments.push(text(escape_string(s))),
            StringPart::Expr { tokens, spec } => {
                // Preserve interpolation content exactly as-is. Phase
                // 2 #06.B: spec is also reconstructed when non-default,
                // matching the input syntax (`:?`, `:>10.2`, etc.).
                let content: String = tokens
                    .iter()
                    .map(|t| token_to_source(t).to_string())
                    .collect::<Vec<_>>()
                    .join("");
                let spec_str = format_spec_to_source(spec);
                segments.push(text(format!("#{{{}{}}}", content, spec_str)));
            }
        }
    }
    segments.push(text("\""));
    concat(segments)
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

/// Best-effort reconstruction of a token to its source form.
fn token_to_source(token: &crate::lexer::token::Token) -> String {
    use crate::lexer::token::TokenKind;
    match &token.kind {
        TokenKind::Identifier(s) | TokenKind::TypeIdentifier(s) => s.clone(),
        TokenKind::IntLiteral(v, sfx) => {
            let base = format!("{}", v);
            match sfx {
                Some(s) => format!("{}{}", base, suffix_str(s)),
                None => base,
            }
        }
        TokenKind::FloatLiteral(v, sfx) => {
            let base = format!("{}", v);
            match sfx {
                Some(s) => format!("{}{}", base, suffix_str(s)),
                None => base,
            }
        }
        TokenKind::StringLiteral(s) => format!("\"{}\"", s),
        TokenKind::CharLiteral(c) => format_char_literal(*c),
        TokenKind::DocComment(s) => format!("## {}", s),
        TokenKind::Dot => ".".to_string(),
        TokenKind::Comma => ",".to_string(),
        TokenKind::Colon => ":".to_string(),
        // §3.9: `::` is retired; `.` is the path separator everywhere.
        TokenKind::ColonColon => ".".to_string(),
        TokenKind::Semicolon => ";".to_string(),
        TokenKind::Plus => "+".to_string(),
        TokenKind::Minus => "-".to_string(),
        TokenKind::Star => "*".to_string(),
        TokenKind::Slash => "/".to_string(),
        TokenKind::Percent => "%".to_string(),
        TokenKind::EqEq => "==".to_string(),
        TokenKind::NotEq => "!=".to_string(),
        TokenKind::Lt => "<".to_string(),
        TokenKind::Gt => ">".to_string(),
        TokenKind::LtEq => "<=".to_string(),
        TokenKind::GtEq => ">=".to_string(),
        TokenKind::AmpAmp => "&&".to_string(),
        TokenKind::PipePipe => "||".to_string(),
        TokenKind::Bang => "!".to_string(),
        TokenKind::Amp => "&".to_string(),
        TokenKind::Pipe => "|".to_string(),
        TokenKind::Caret => "^".to_string(),
        TokenKind::Shl => "<<".to_string(),
        TokenKind::Shr => ">>".to_string(),
        TokenKind::Eq => "=".to_string(),
        TokenKind::Arrow => "->".to_string(),
        TokenKind::DotDot => "..".to_string(),
        TokenKind::DotDotDot => "...".to_string(),
        TokenKind::Question => "?".to_string(),
        TokenKind::AmpDot => "&.".to_string(),
        TokenKind::At => "@".to_string(),
        TokenKind::AmpMut => "&var".to_string(),
        TokenKind::LParen => "(".to_string(),
        TokenKind::RParen => ")".to_string(),
        TokenKind::LBracket => "[".to_string(),
        TokenKind::RBracket => "]".to_string(),
        TokenKind::LBrace => "{".to_string(),
        TokenKind::RBrace => "}".to_string(),
        TokenKind::True => "true".to_string(),
        TokenKind::False => "false".to_string(),
        TokenKind::SelfValue => "self".to_string(),
        TokenKind::SelfType => "Self".to_string(),
        TokenKind::Lifetime(s) => format!("'{}", s),
        TokenKind::Newline | TokenKind::Eof => String::new(),
        // Keywords
        TokenKind::Let => "let".to_string(),
        TokenKind::Mut => "var".to_string(), // legacy variant — never produced by lexer now
        TokenKind::If => "if".to_string(),
        TokenKind::Else => "else".to_string(),
        TokenKind::Match => "match".to_string(),
        TokenKind::Return => "return".to_string(),
        TokenKind::While => "while".to_string(),
        TokenKind::For => "for".to_string(),
        TokenKind::In => "in".to_string(),
        TokenKind::Do => "do".to_string(),
        TokenKind::End => "end".to_string(),
        TokenKind::Def => "def".to_string(),
        TokenKind::Class => "class".to_string(),
        TokenKind::SomeKw => "Some".to_string(),
        TokenKind::OkKw => "Ok".to_string(),
        TokenKind::ErrKw => "Err".to_string(),
        TokenKind::Nil => "nil".to_string(),
        TokenKind::As => "as".to_string(),
        _ => format!("{:?}", token.kind),
    }
}

fn bin_op_str(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
        BinOp::Eq => "==",
        BinOp::NotEq => "!=",
        BinOp::Lt => "<",
        BinOp::Gt => ">",
        BinOp::LtEq => "<=",
        BinOp::GtEq => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
        BinOp::BitAnd => "&",
        BinOp::BitOr => "|",
        BinOp::BitXor => "^",
        BinOp::Shl => "<<",
        BinOp::Shr => ">>",
        BinOp::MatchOp => "~=",
    }
}

fn compound_assign_op_str(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+=",
        BinOp::Sub => "-=",
        BinOp::Mul => "*=",
        BinOp::Div => "/=",
        BinOp::Mod => "%=",
        _ => "=",
    }
}

fn format_binary_op(left: &Expr, op: BinOp, right: &Expr, comments: &CommentMap) -> Doc {
    group(concat(vec![
        format_expr(left, comments),
        text(" "),
        text(bin_op_str(op)),
        line(),
        format_expr(right, comments),
    ]))
}

fn format_unary_op(op: UnaryOp, operand: &Expr, comments: &CommentMap) -> Doc {
    let op_str = match op {
        UnaryOp::Neg => "-",
        UnaryOp::Not => "!",
        UnaryOp::Deref => "*",
    };
    concat(vec![text(op_str), format_expr(operand, comments)])
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

    while let ExprKind::MethodCall { object, .. } | ExprKind::FieldAccess { object, .. } = current {
        links.push(current.clone());
        current = &object.kind;
    }

    // `current` is now the receiver (base of the chain).
    let comments = CommentMap::new();
    let receiver = format_expr_kind(current, &comments);

    // Links are in reverse order (innermost first), so reverse.
    links.reverse();
    let link_docs: Vec<Doc> = links
        .iter()
        .map(|link| match link {
            ExprKind::FieldAccess { field, .. } => concat(vec![text("."), text(field.clone())]),
            ExprKind::MethodCall {
                method,
                args,
                block,
                ..
            } => {
                let arg_docs: Vec<Doc> = args.iter().map(|a| format_expr(a, &comments)).collect();
                let mut parts = vec![text("."), text(method.clone())];
                if !arg_docs.is_empty() || block.is_some() {
                    parts.push(format_call_args(arg_docs));
                }
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

fn format_single_method_call(
    object: &Expr,
    method: &str,
    args: &[Expr],
    block: Option<&Expr>,
    comments: &CommentMap,
) -> Doc {
    let arg_docs: Vec<Doc> = args.iter().map(|a| format_expr(a, comments)).collect();
    let mut parts = vec![
        format_expr(object, comments),
        text("."),
        text(method.to_string()),
    ];
    // Only emit parens if there are args or a block (no-arg method calls don't need parens)
    if !arg_docs.is_empty() || block.is_some() {
        parts.push(format_call_args(arg_docs));
    }
    if let Some(blk) = block {
        parts.push(text(" "));
        parts.push(format_expr(blk, comments));
    }
    concat(parts)
}

// ─── Control Flow ───────────────────────────────────────────────────

fn format_if_expr(if_expr: &IfExpr, comments: &CommentMap) -> Doc {
    let mut parts = vec![
        text("if "),
        format_expr(&if_expr.condition, comments),
        nest(
            INDENT_WIDTH,
            concat(vec![
                hardline(),
                format_block_body(&if_expr.then_body, comments),
            ]),
        ),
    ];

    for elsif in &if_expr.elsif_clauses {
        parts.push(hardline());
        parts.push(text("elsif "));
        parts.push(format_expr(&elsif.condition, comments));
        parts.push(nest(
            INDENT_WIDTH,
            concat(vec![hardline(), format_block_body(&elsif.body, comments)]),
        ));
    }

    if let Some(else_body) = &if_expr.else_body {
        parts.push(hardline());
        parts.push(text("else"));
        parts.push(nest(
            INDENT_WIDTH,
            concat(vec![hardline(), format_block_body(else_body, comments)]),
        ));
    }

    parts.push(hardline());
    parts.push(text("end"));

    concat(parts)
}

fn format_if_let_expr(if_let: &IfLetExpr, comments: &CommentMap) -> Doc {
    let mut parts = vec![
        text("if let "),
        format_pattern(&if_let.pattern, comments),
        text(" = "),
        format_expr(&if_let.value, comments),
        nest(
            INDENT_WIDTH,
            concat(vec![
                hardline(),
                format_block_body(&if_let.then_body, comments),
            ]),
        ),
    ];

    if let Some(else_body) = &if_let.else_body {
        parts.push(hardline());
        parts.push(text("else"));
        parts.push(nest(
            INDENT_WIDTH,
            concat(vec![hardline(), format_block_body(else_body, comments)]),
        ));
    }

    parts.push(hardline());
    parts.push(text("end"));

    concat(parts)
}

fn format_match_expr(match_expr: &MatchExpr, comments: &CommentMap) -> Doc {
    let mut parts = vec![text("match "), format_expr(&match_expr.subject, comments)];

    let arm_docs: Vec<Doc> = match_expr
        .arms
        .iter()
        .map(|arm| format_match_arm(arm, comments))
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
    let pattern_doc = format_match_pattern(&arm.pattern, arm.guard.as_deref(), comments);

    match &arm.body {
        MatchArmBody::Expr(expr) => {
            // Single expression: Pattern -> expr
            group(concat(vec![
                pattern_doc,
                text(" -> "),
                format_expr(expr, comments),
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
    concat(vec![
        text("while "),
        format_expr(&w.condition, comments),
        nest(
            INDENT_WIDTH,
            concat(vec![hardline(), format_block_body(&w.body, comments)]),
        ),
        hardline(),
        text("end"),
    ])
}

fn format_while_let_expr(wl: &WhileLetExpr, comments: &CommentMap) -> Doc {
    concat(vec![
        text("while let "),
        format_pattern(&wl.pattern, comments),
        text(" = "),
        format_expr(&wl.value, comments),
        nest(
            INDENT_WIDTH,
            concat(vec![hardline(), format_block_body(&wl.body, comments)]),
        ),
        hardline(),
        text("end"),
    ])
}

fn format_for_expr(f: &ForExpr, comments: &CommentMap) -> Doc {
    concat(vec![
        text("for "),
        format_pattern(&f.pattern, comments),
        text(" in "),
        format_expr(&f.iterable, comments),
        nest(
            INDENT_WIDTH,
            concat(vec![hardline(), format_block_body(&f.body, comments)]),
        ),
        hardline(),
        text("end"),
    ])
}

fn format_loop_expr(l: &LoopExpr, comments: &CommentMap) -> Doc {
    concat(vec![
        text("loop"),
        nest(
            INDENT_WIDTH,
            concat(vec![hardline(), format_block_body(&l.body, comments)]),
        ),
        hardline(),
        text("end"),
    ])
}

// ─── Blocks ─────────────────────────────────────────────────────────

pub fn format_block(block: &Block, comments: &CommentMap) -> Doc {
    // `ExprKind::Block` is a `do ... end` block expression (parser
    // `parse_do_block_expr`). The `do`/`end` delimiters are part of the
    // surface syntax — emitting only the inner statements collapses the block
    // into its surrounding context and no longer parses.
    if block.statements.is_empty() {
        return concat(vec![text("do"), hardline(), text("end")]);
    }
    concat(vec![
        text("do"),
        nest(
            INDENT_WIDTH,
            concat(vec![hardline(), format_block_body(block, comments)]),
        ),
        hardline(),
        text("end"),
    ])
}

pub fn format_block_body(block: &Block, comments: &CommentMap) -> Doc {
    let stmt_docs: Vec<Doc> = block
        .statements
        .iter()
        .map(|stmt| format_statement(stmt, comments))
        .collect();

    join(hardline(), stmt_docs)
}

pub fn format_statement(stmt: &Statement, comments: &CommentMap) -> Doc {
    match stmt {
        Statement::Let(l) => format_let_binding(l, comments),
        Statement::Expression(e) => format_expr(e, comments),
    }
}

fn format_let_binding(binding: &LetBinding, comments: &CommentMap) -> Doc {
    // §3.1: `var x = ...` is the writable binding form; `let mut x` is retired.
    let head = if binding.mutable { "var " } else { "let " };
    let mut parts = vec![text(head)];

    parts.push(format_pattern(&binding.pattern, comments));

    if let Some(ty) = &binding.type_annotation {
        parts.push(text(": "));
        parts.push(format_type_expr(ty, comments));
    }

    if let Some(val) = &binding.value {
        parts.push(text(" = "));
        parts.push(nest(INDENT_WIDTH, format_expr(val, comments)));
    }

    group(concat(parts))
}

// ─── Closures ───────────────────────────────────────────────────────

pub fn format_closure(closure: &ClosureExpr, comments: &CommentMap) -> Doc {
    let params_doc = format_closure_params(&closure.params, comments);

    // `async` / `move` modifiers (parser keyword order is `async move`). A move
    // closure written `{ |x| ... }` without the `move` keyword does not re-parse
    // at expression position — the leading `{` is taken as a block — so the
    // modifier must be preserved.
    let mut prefix: Vec<Doc> = Vec::new();
    if closure.is_async {
        prefix.push(text("async "));
    }
    if closure.is_move {
        prefix.push(text("move "));
    }
    let prefix_doc = concat(prefix);

    let body = match &closure.body {
        ClosureBody::Expr(expr) => {
            // Single expression: normalize to { |params| expr }
            let body_doc = format_expr(expr, comments);
            group(concat(vec![
                text("{ "),
                params_doc,
                text(" "),
                body_doc,
                text(" }"),
            ]))
        }
        ClosureBody::Block(block) => {
            if block.statements.len() <= 1 {
                // Single statement — try brace form first
                let body_doc = format_block_body(block, comments);
                let brace_form = group(concat(vec![
                    text("{ "),
                    params_doc.clone(),
                    text(" "),
                    body_doc.clone(),
                    text(" }"),
                ]));

                // If it doesn't fit, use do...end
                let do_form = concat(vec![
                    text("do "),
                    params_doc,
                    nest(INDENT_WIDTH, concat(vec![hardline(), body_doc])),
                    hardline(),
                    text("end"),
                ]);

                // Use group with if_break to choose between forms
                group(if_break(do_form, brace_form))
            } else {
                // Multi-statement: always use do...end
                let body_doc = format_block_body(block, comments);
                concat(vec![
                    text("do "),
                    params_doc,
                    nest(INDENT_WIDTH, concat(vec![hardline(), body_doc])),
                    hardline(),
                    text("end"),
                ])
            }
        }
    };

    concat(vec![prefix_doc, body])
}

fn format_closure_params(params: &[ClosureParam], comments: &CommentMap) -> Doc {
    if params.is_empty() {
        return nil();
    }

    let param_docs: Vec<Doc> = params
        .iter()
        .map(|p| {
            if let Some(ty) = &p.type_expr {
                concat(vec![
                    text(p.name.clone()),
                    text(": "),
                    format_type_expr(ty, comments),
                ])
            } else {
                text(p.name.clone())
            }
        })
        .collect();

    concat(vec![
        text("|"),
        join(concat(vec![text(","), space()]), param_docs),
        text("|"),
    ])
}
