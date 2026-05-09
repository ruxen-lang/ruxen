//! Expression parsing for the Riven language using Pratt-style precedence climbing.

use crate::lexer::token::{Span, TokenKind};
use crate::parser::ast::*;
use crate::parser::Parser;

/// Binding power pairs (left, right). Higher = tighter binding.
/// Right-associative: left < right. Left-associative: left > right (or left == right-1).
/// Non-associative: left == right.
fn infix_binding_power(kind: &TokenKind) -> Option<(u8, u8)> {
    match kind {
        // Assignment: right-associative (1, 2)
        TokenKind::Eq
        | TokenKind::PlusEq
        | TokenKind::MinusEq
        | TokenKind::StarEq
        | TokenKind::SlashEq
        | TokenKind::PercentEq => Some((1, 2)),

        // Logical OR: left-associative (3, 4)
        TokenKind::PipePipe => Some((3, 4)),

        // Logical AND: left-associative (5, 6)
        TokenKind::AmpAmp => Some((5, 6)),

        // Comparison: non-associative (7, 8)
        TokenKind::EqEq
        | TokenKind::NotEq
        | TokenKind::Lt
        | TokenKind::Gt
        | TokenKind::LtEq
        | TokenKind::GtEq => Some((7, 8)),

        // Range: non-associative (9, 10)
        TokenKind::DotDot | TokenKind::DotDotEq => Some((9, 10)),

        // Bitwise OR (11, 12)
        TokenKind::Pipe => Some((11, 12)),

        // Bitwise XOR (13, 14)
        TokenKind::Caret => Some((13, 14)),

        // Bitwise AND (15, 16)
        TokenKind::Amp => Some((15, 16)),

        // Shift (17, 18)
        TokenKind::Shl | TokenKind::Shr => Some((17, 18)),

        // Add/Sub (19, 20)
        TokenKind::Plus | TokenKind::Minus => Some((19, 20)),

        // Mul/Div/Mod (21, 22)
        TokenKind::Star | TokenKind::Slash | TokenKind::Percent => Some((21, 22)),

        // Cast (23, 24)
        TokenKind::As => Some((23, 24)),

        _ => None,
    }
}

// Prefix binding power is used directly in parse_prefix (value: 25).
// Kept as reference:
// TokenKind::Minus | TokenKind::Bang | TokenKind::Amp | TokenKind::AmpMut => 25

/// Postfix binding power
const POSTFIX_BP: u8 = 27;

impl Parser {
    /// Parse an expression with the given minimum binding power.
    pub(crate) fn parse_expression(&mut self) -> Expr {
        self.parse_expr_bp(0)
    }

    /// Core Pratt parser.
    fn parse_expr_bp(&mut self, min_bp: u8) -> Expr {
        self.skip_newlines();
        let mut lhs = self.parse_prefix();

        loop {
            self.skip_newlines_if_continuation();

            let kind = self.current_kind().clone();

            // Check for postfix operators
            if self.is_postfix_op(&kind) && POSTFIX_BP >= min_bp {
                lhs = self.parse_postfix(lhs);
                continue;
            }

            // Check for infix operators
            if let Some((l_bp, r_bp)) = infix_binding_power(&kind) {
                if l_bp < min_bp {
                    break;
                }
                lhs = self.parse_infix(lhs, &kind.clone(), r_bp);
                continue;
            }

            break;
        }

        lhs
    }

    fn is_postfix_op(&self, kind: &TokenKind) -> bool {
        matches!(
            kind,
            TokenKind::Dot
                | TokenKind::QuestionDot
                | TokenKind::LBracket
                | TokenKind::Question
                | TokenKind::LParen
        )
    }

    /// Skip newlines only if the next meaningful token continues the expression
    /// (e.g., `.method`, `?.field`). This handles method chaining across lines.
    fn skip_newlines_if_continuation(&mut self) {
        if !self.at(TokenKind::Newline) {
            return;
        }
        // Peek past all newlines to find the next meaningful token
        let mut offset = 0;
        loop {
            let kind = self.peek_at_kind(offset);
            if kind == TokenKind::Newline {
                offset += 1;
                continue;
            }
            // If next meaningful token is `.` or `?.`, skip the newlines
            if matches!(kind, TokenKind::Dot | TokenKind::QuestionDot) {
                // skip all the newlines
                while self.at(TokenKind::Newline) {
                    self.advance();
                }
            }
            break;
        }
    }

    fn parse_prefix(&mut self) -> Expr {
        self.skip_newlines();
        let start = self.current_span();
        let kind = self.current_kind().clone();

        match kind {
            // Unary operators
            TokenKind::Minus => {
                self.advance();
                let operand = self.parse_expr_bp(25);
                let span = self.span_from(&start);
                Expr {
                    kind: ExprKind::UnaryOp {
                        op: UnaryOp::Neg,
                        operand: Box::new(operand),
                    },
                    span,
                }
            }
            TokenKind::Bang => {
                self.advance();
                let operand = self.parse_expr_bp(25);
                let span = self.span_from(&start);
                Expr {
                    kind: ExprKind::UnaryOp {
                        op: UnaryOp::Not,
                        operand: Box::new(operand),
                    },
                    span,
                }
            }
            TokenKind::Amp => {
                self.advance();
                let operand = self.parse_expr_bp(25);
                let span = self.span_from(&start);
                Expr {
                    kind: ExprKind::Borrow(Box::new(operand)),
                    span,
                }
            }
            TokenKind::AmpMut => {
                self.advance();
                let operand = self.parse_expr_bp(25);
                let span = self.span_from(&start);
                Expr {
                    kind: ExprKind::BorrowMut(Box::new(operand)),
                    span,
                }
            }

            // Prefix `*` — dereference
            TokenKind::Star => {
                self.advance();
                let operand = self.parse_expr_bp(25);
                let span = self.span_from(&start);
                Expr {
                    kind: ExprKind::UnaryOp {
                        op: UnaryOp::Deref,
                        operand: Box::new(operand),
                    },
                    span,
                }
            }

            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Expr {
        self.skip_newlines();
        let start = self.current_span();
        let kind = self.current_kind().clone();

        match kind {
            // Literals
            TokenKind::IntLiteral(val, suffix) => {
                self.advance();
                Expr {
                    kind: ExprKind::IntLiteral(val, suffix),
                    span: start,
                }
            }
            TokenKind::FloatLiteral(val, suffix) => {
                self.advance();
                Expr {
                    kind: ExprKind::FloatLiteral(val, suffix),
                    span: start,
                }
            }
            TokenKind::StringLiteral(ref val) => {
                let val = val.clone();
                self.advance();
                Expr {
                    kind: ExprKind::StringLiteral(val),
                    span: start,
                }
            }
            TokenKind::InterpolatedString(ref parts) => {
                let parts = parts.clone();
                self.advance();
                Expr {
                    kind: ExprKind::InterpolatedString(parts),
                    span: start,
                }
            }
            TokenKind::CharLiteral(val) => {
                self.advance();
                Expr {
                    kind: ExprKind::CharLiteral(val),
                    span: start,
                }
            }
            TokenKind::True => {
                self.advance();
                Expr {
                    kind: ExprKind::BoolLiteral(true),
                    span: start,
                }
            }
            TokenKind::False => {
                self.advance();
                Expr {
                    kind: ExprKind::BoolLiteral(false),
                    span: start,
                }
            }

            // self
            TokenKind::SelfValue => {
                self.advance();
                Expr {
                    kind: ExprKind::SelfRef,
                    span: start,
                }
            }

            // Self
            TokenKind::SelfType => {
                self.advance();
                Expr {
                    kind: ExprKind::SelfType,
                    span: start,
                }
            }

            // None, Some, Ok, Err — used as enum constructors
            TokenKind::NoneKw => {
                self.advance();
                Expr {
                    kind: ExprKind::Identifier("None".to_string()),
                    span: start,
                }
            }
            TokenKind::SomeKw => {
                self.advance();
                self.parse_constructor_args("Some", vec![], start)
            }
            TokenKind::OkKw => {
                self.advance();
                self.parse_constructor_args("Ok", vec![], start)
            }
            TokenKind::ErrKw => {
                self.advance();
                self.parse_constructor_args("Err", vec![], start)
            }

            // Type identifier — could be enum constructor, type path, etc.
            TokenKind::TypeIdentifier(ref name) => {
                let name = name.clone();
                self.advance();
                self.parse_type_expr_primary(name, start)
            }

            // Plain identifier
            TokenKind::Identifier(ref name) => {
                let name = name.clone();
                self.advance();
                // Check for macro call: name!(...) — also handle ident ending with !
                if name.ends_with('!') {
                    // Already has !, parse macro args
                    let trimmed = name.trim_end_matches('!').to_string();
                    self.parse_macro_call_args(trimmed, start)
                } else if self.at(TokenKind::Bang) {
                    // name followed by !
                    self.advance(); // consume !
                    self.parse_macro_call_args(name, start)
                } else if self.at(TokenKind::LParen) {
                    // Function call
                    let args = self.parse_call_args();
                    let block = self.maybe_parse_block_arg();
                    let span = self.span_from(&start);
                    Expr {
                        kind: ExprKind::Call {
                            callee: Box::new(Expr {
                                kind: ExprKind::Identifier(name),
                                span: start,
                            }),
                            args,
                            block: block.map(Box::new),
                        },
                        span,
                    }
                } else if self.is_bare_call_arg_start(&name) {
                    // Bare function call without parens: `puts "hello"`, `puts msg`
                    let arg = self.parse_expression();
                    let span = self.span_from(&start);
                    Expr {
                        kind: ExprKind::Call {
                            callee: Box::new(Expr {
                                kind: ExprKind::Identifier(name),
                                span: start,
                            }),
                            args: vec![arg],
                            block: None,
                        },
                        span,
                    }
                } else if self.is_trailing_block_start() {
                    // Bare function call with a trailing block only, no parens
                    // and no other arguments: `with_x do |n| ... end` or
                    // `with_x { |n| ... }`.
                    let block = self.maybe_parse_block_arg();
                    let span = self.span_from(&start);
                    Expr {
                        kind: ExprKind::Call {
                            callee: Box::new(Expr {
                                kind: ExprKind::Identifier(name),
                                span: start,
                            }),
                            args: vec![],
                            block: block.map(Box::new),
                        },
                        span,
                    }
                } else {
                    Expr {
                        kind: ExprKind::Identifier(name),
                        span: start,
                    }
                }
            }

            // Parenthesized expression or tuple
            TokenKind::LParen => self.parse_paren_or_tuple(),

            // Array literal
            TokenKind::LBracket => self.parse_array_literal(),

            // Block expressions (closures)
            TokenKind::LBrace => self.parse_brace_closure(false, false),

            // do ... end — either a closure (if `|params|` follow) or a
            // block expression whose value is the last expression.
            TokenKind::Do => {
                // Peek past newlines for closure-param markers.
                let mut look = 1;
                while matches!(self.peek_at_kind(look), TokenKind::Newline) {
                    look += 1;
                }
                if matches!(
                    self.peek_at_kind(look),
                    TokenKind::Pipe | TokenKind::PipePipe
                ) {
                    self.parse_do_closure(false, false)
                } else {
                    self.parse_do_block_expr()
                }
            }

            // move closure
            TokenKind::Move => {
                self.advance();
                if self.at(TokenKind::LBrace) {
                    self.parse_brace_closure(false, true)
                } else if self.at(TokenKind::Do) {
                    self.parse_do_closure(false, true)
                } else {
                    self.error("expected `{` or `do` after `move`");
                    Expr {
                        kind: ExprKind::Identifier("_error".to_string()),
                        span: start,
                    }
                }
            }

            TokenKind::Async => {
                self.advance();
                let is_move = self.eat(TokenKind::Move);
                if self.at(TokenKind::LBrace) {
                    self.parse_brace_closure(true, is_move)
                } else if self.at(TokenKind::Do) {
                    self.parse_do_closure(true, is_move)
                } else {
                    self.error("expected `def`, `do`, or `{` after `async`");
                    Expr {
                        kind: ExprKind::Identifier("_error".to_string()),
                        span: start,
                    }
                }
            }

            TokenKind::Await => {
                self.advance();
                self.error("prefix `await` is not supported; use postfix `.await`");
                Expr {
                    kind: ExprKind::Identifier("_error".to_string()),
                    span: start,
                }
            }

            // Unsafe block: `unsafe ... end`
            TokenKind::Unsafe => {
                self.advance(); // consume `unsafe`
                self.skip_newlines();
                let body = self.parse_body();
                self.expect(TokenKind::End);
                let span = self.span_from(&start);
                Expr {
                    kind: ExprKind::UnsafeBlock(body),
                    span,
                }
            }

            // Null literal
            TokenKind::Null => {
                self.advance();
                Expr {
                    kind: ExprKind::NullLiteral,
                    span: start,
                }
            }

            // Control flow expressions
            TokenKind::If => self.parse_if_expr(),
            TokenKind::Match => self.parse_match_expr(),
            TokenKind::While => self.parse_while_expr(),
            TokenKind::For => self.parse_for_expr(),
            TokenKind::Loop => self.parse_loop_expr(),

            // Return, break, continue
            TokenKind::Return => {
                self.advance();
                let value = if self.is_expression_start() {
                    Some(Box::new(self.parse_expression()))
                } else {
                    None
                };
                let span = self.span_from(&start);
                Expr {
                    kind: ExprKind::Return(value),
                    span,
                }
            }
            TokenKind::Break => {
                self.advance();
                let value = if self.is_expression_start() {
                    Some(Box::new(self.parse_expression()))
                } else {
                    None
                };
                let span = self.span_from(&start);
                Expr {
                    kind: ExprKind::Break(value),
                    span,
                }
            }
            TokenKind::Continue => {
                self.advance();
                Expr {
                    kind: ExprKind::Continue,
                    span: start,
                }
            }

            // Yield
            TokenKind::Yield => {
                self.advance();
                let mut args = Vec::new();
                if self.is_expression_start() {
                    args.push(self.parse_expression());
                    while self.eat(TokenKind::Comma) {
                        self.skip_newlines();
                        args.push(self.parse_expression());
                    }
                }
                let span = self.span_from(&start);
                Expr {
                    kind: ExprKind::Yield(args),
                    span,
                }
            }

            // Super (for constructor calls like super(...))
            TokenKind::Super => {
                self.advance();
                if self.at(TokenKind::LParen) {
                    let args = self.parse_call_args();
                    let span = self.span_from(&start);
                    Expr {
                        kind: ExprKind::Call {
                            callee: Box::new(Expr {
                                kind: ExprKind::Identifier("super".to_string()),
                                span: start,
                            }),
                            args,
                            block: None,
                        },
                        span,
                    }
                } else {
                    Expr {
                        kind: ExprKind::Identifier("super".to_string()),
                        span: start,
                    }
                }
            }

            _ => {
                self.error(&format!(
                    "expected expression, found {:?}",
                    self.current_kind()
                ));
                self.advance();
                Expr {
                    kind: ExprKind::Identifier("_error".to_string()),
                    span: start,
                }
            }
        }
    }

    /// Heuristic: the current `[` opens a list of type arguments, not an
    /// indexing expression. True when the token immediately after `[` is a
    /// TypeIdentifier, Self, or a lifetime and the matching `]` is followed
    /// by `.` or `(` (a method call or constructor).
    fn looks_like_type_args(&self) -> bool {
        if !matches!(self.current_kind(), TokenKind::LBracket) {
            return false;
        }
        // Peek past any newlines after `[`.
        let mut idx = 1;
        while matches!(self.peek_at_kind(idx), TokenKind::Newline) {
            idx += 1;
        }
        // First token after `[` should look like a type.
        let first = self.peek_at_kind(idx);
        if !matches!(
            first,
            TokenKind::TypeIdentifier(_)
                | TokenKind::SelfType
                | TokenKind::Lifetime(_)
                | TokenKind::Amp
                | TokenKind::AmpMut
        ) {
            return false;
        }
        // Scan for the matching `]`, tracking bracket depth.
        let mut depth: i32 = 1;
        let mut j = idx;
        while depth > 0 {
            match self.peek_at_kind(j) {
                TokenKind::LBracket => depth += 1,
                TokenKind::RBracket => depth -= 1,
                TokenKind::Eof => return false,
                _ => {}
            }
            j += 1;
            if j > 256 {
                return false; // safety bound
            }
        }
        // After the matching `]` (j is one past it), check for `.` or `(`.
        let after = self.peek_at_kind(j);
        matches!(after, TokenKind::Dot | TokenKind::LParen)
    }

    /// After seeing a TypeIdentifier, parse the rest of the primary.
    /// This handles: TypeName.method/field, TypeName.Variant(...), TypeName[GenericArgs](...), TypeName.new(...)
    fn parse_type_expr_primary(&mut self, name: String, start: Span) -> Expr {
        // Check for generic type arguments: Name[T, U].method(...)
        // Distinguish type-application from indexing: a type-application has
        // one or more type-like tokens (TypeIdentifier/Self) inside. Generic
        // args are erased — they're inferred from constructor/method args.
        if self.at(TokenKind::LBracket) && self.looks_like_type_args() {
            let _generic_args = self.parse_generic_args();
            // Fall through to normal handling as if we had just the TypeIdentifier.
        }

        // Check if followed by . — could be enum variant construction or static method
        if self.at(TokenKind::Dot) {
            // Peek what follows the dot
            let after_dot = self.peek_at_kind(1);
            // The stdlib variant keywords (Some/None/Ok/Err) also serve as
            // variant names for user-defined generic enums that re-use
            // those identifiers (e.g. `enum MyOpt[T] { Some(T), None }`),
            // so treat them like TypeIdentifiers for `Type.Variant(...)`
            // and `Type.Variant` parsing.
            let is_variant_kw = matches!(
                after_dot,
                TokenKind::SomeKw | TokenKind::NoneKw | TokenKind::OkKw | TokenKind::ErrKw
            );
            if is_variant_kw {
                // `Type.Some` / `Type.None` / `Type.Ok` / `Type.Err` —
                // user-defined generic enums that re-use a stdlib keyword
                // as a variant name.
                self.advance(); // consume .
                let variant_name = match self.current_kind() {
                    TokenKind::SomeKw => "Some".to_string(),
                    TokenKind::NoneKw => "None".to_string(),
                    TokenKind::OkKw => "Ok".to_string(),
                    TokenKind::ErrKw => "Err".to_string(),
                    _ => {
                        self.error("expected variant name");
                        "_error".to_string()
                    }
                };
                self.advance(); // consume the keyword
                let path = vec![name.clone()];
                if self.at(TokenKind::LParen) {
                    let args = self.parse_field_args();
                    let span = self.span_from(&start);
                    Expr {
                        kind: ExprKind::EnumVariant {
                            type_path: path,
                            variant: variant_name,
                            args,
                        },
                        span,
                    }
                } else {
                    let span = self.span_from(&start);
                    Expr {
                        kind: ExprKind::EnumVariant {
                            type_path: path,
                            variant: variant_name,
                            args: vec![],
                        },
                        span,
                    }
                }
            } else {
                match after_dot {
                    // TypeName.Variant or TypeName.method
                    TokenKind::TypeIdentifier(_) => {
                        // Enum variant: Status.InProgress(...)
                        self.advance(); // consume .
                        let mut path = vec![name.clone()];
                        let variant_name;
                        // Collect path: A.B.C — all TypeIdentifiers
                        loop {
                            if let TokenKind::TypeIdentifier(ref vname) =
                                self.current_kind().clone()
                            {
                                let vname = vname.clone();
                                self.advance();
                                if self.at(TokenKind::Dot) {
                                    if let TokenKind::TypeIdentifier(_) = self.peek_kind() {
                                        path.push(vname);
                                        self.advance(); // consume .
                                        continue;
                                    }
                                }
                                variant_name = vname;
                                break;
                            } else {
                                self.error("expected variant name");
                                variant_name = "_error".to_string();
                                break;
                            }
                        }

                        // Check for constructor args
                        if self.at(TokenKind::LParen) {
                            let args = self.parse_field_args();
                            let span = self.span_from(&start);
                            Expr {
                                kind: ExprKind::EnumVariant {
                                    type_path: path,
                                    variant: variant_name,
                                    args,
                                },
                                span,
                            }
                        } else {
                            // Unit variant
                            let span = self.span_from(&start);
                            Expr {
                                kind: ExprKind::EnumVariant {
                                    type_path: path,
                                    variant: variant_name,
                                    args: vec![],
                                },
                                span,
                            }
                        }
                    }
                    _ => {
                        // TypeName.method(...) or TypeName.field
                        // Return the type as an identifier, postfix will handle .method
                        Expr {
                            kind: ExprKind::Identifier(name),
                            span: start,
                        }
                    }
                }
            }
        } else if self.at(TokenKind::LParen) {
            // TypeName(...) — constructor call
            let args = self.parse_call_args();
            let block = self.maybe_parse_block_arg();
            let span = self.span_from(&start);
            Expr {
                kind: ExprKind::Call {
                    callee: Box::new(Expr {
                        kind: ExprKind::Identifier(name),
                        span: start,
                    }),
                    args,
                    block: block.map(Box::new),
                },
                span,
            }
        } else {
            Expr {
                kind: ExprKind::Identifier(name),
                span: start,
            }
        }
    }

    /// Parse constructor/enum variant arguments: `(expr, name: expr, ...)`
    fn parse_field_args(&mut self) -> Vec<FieldArg> {
        self.expect(TokenKind::LParen);
        self.skip_newlines();
        let mut args = Vec::new();

        if !self.at(TokenKind::RParen) {
            args.push(self.parse_field_arg());
            while self.eat(TokenKind::Comma) {
                self.skip_newlines();
                if self.at(TokenKind::RParen) {
                    break;
                }
                args.push(self.parse_field_arg());
            }
        }
        self.skip_newlines();
        self.expect(TokenKind::RParen);
        args
    }

    fn parse_field_arg(&mut self) -> FieldArg {
        let start = self.current_span();
        self.skip_newlines();

        // Check for named field: name: expr
        if let TokenKind::Identifier(ref name) = self.current_kind().clone() {
            let name_val = name.clone();
            if self.peek_kind() == TokenKind::Colon {
                self.advance(); // consume name
                self.advance(); // consume :
                self.skip_newlines();
                let value = self.parse_expression();
                let span = self.span_from(&start);
                return FieldArg {
                    name: Some(name_val),
                    value,
                    span,
                };
            }
        }

        let value = self.parse_expression();
        let span = self.span_from(&start);
        FieldArg {
            name: None,
            value,
            span,
        }
    }

    fn parse_constructor_args(&mut self, name: &str, type_path: Vec<String>, start: Span) -> Expr {
        if self.at(TokenKind::LParen) {
            let args = self.parse_field_args();
            let span = self.span_from(&start);
            Expr {
                kind: ExprKind::EnumVariant {
                    type_path,
                    variant: name.to_string(),
                    args,
                },
                span,
            }
        } else {
            Expr {
                kind: ExprKind::Identifier(name.to_string()),
                span: start,
            }
        }
    }

    fn parse_macro_call_args(&mut self, name: String, start: Span) -> Expr {
        let (args, delimiter) = match self.current_kind() {
            TokenKind::LParen => {
                let args = self.parse_call_args();
                (args, MacroDelimiter::Paren)
            }
            TokenKind::LBracket => {
                self.advance();
                self.skip_newlines();
                let mut args = Vec::new();
                if !self.at(TokenKind::RBracket) {
                    args.push(self.parse_expression());
                    while self.eat(TokenKind::Comma) {
                        self.skip_newlines();
                        if self.at(TokenKind::RBracket) {
                            break;
                        }
                        args.push(self.parse_expression());
                    }
                }
                self.skip_newlines();
                self.expect(TokenKind::RBracket);
                (args, MacroDelimiter::Bracket)
            }
            TokenKind::LBrace => {
                self.advance();
                self.skip_newlines();
                let mut args = Vec::new();
                // Special handling for `hash!{ k => v, ... }` — parse key/value
                // pairs and flatten into a [k1, v1, k2, v2, ...] arg list so
                // downstream HIR/MIR can treat them pair-wise.
                let is_hash_macro = name == "hash";
                if !self.at(TokenKind::RBrace) {
                    let first = self.parse_expression();
                    if is_hash_macro && self.at(TokenKind::FatArrow) {
                        self.advance(); // =>
                        self.skip_newlines();
                        let value = self.parse_expression();
                        args.push(first);
                        args.push(value);
                        while self.eat(TokenKind::Comma) {
                            self.skip_newlines();
                            if self.at(TokenKind::RBrace) {
                                break;
                            }
                            let k = self.parse_expression();
                            self.expect(TokenKind::FatArrow);
                            self.skip_newlines();
                            let v = self.parse_expression();
                            args.push(k);
                            args.push(v);
                        }
                    } else {
                        args.push(first);
                        while self.eat(TokenKind::Comma) {
                            self.skip_newlines();
                            if self.at(TokenKind::RBrace) {
                                break;
                            }
                            args.push(self.parse_expression());
                        }
                    }
                }
                self.skip_newlines();
                self.expect(TokenKind::RBrace);
                (args, MacroDelimiter::Brace)
            }
            _ => {
                // Macro with no args
                (vec![], MacroDelimiter::Paren)
            }
        };

        let span = self.span_from(&start);
        Expr {
            kind: ExprKind::MacroCall {
                name,
                args,
                delimiter,
            },
            span,
        }
    }

    /// Parse call arguments: (expr, expr, ...)
    pub(crate) fn parse_call_args(&mut self) -> Vec<Expr> {
        self.expect(TokenKind::LParen);
        self.skip_newlines();
        let mut args = Vec::new();
        if !self.at(TokenKind::RParen) {
            args.push(self.parse_expression());
            while self.eat(TokenKind::Comma) {
                self.skip_newlines();
                if self.at(TokenKind::RParen) {
                    break;
                }
                args.push(self.parse_expression());
            }
        }
        self.skip_newlines();
        self.expect(TokenKind::RParen);
        args
    }

    fn parse_postfix(&mut self, lhs: Expr) -> Expr {
        let start_span = lhs.span.clone();

        match self.current_kind().clone() {
            // Method/field access: .name or .name(args) [block]
            TokenKind::Dot => {
                self.advance(); // consume .
                if self.at(TokenKind::Await) {
                    self.advance();
                    let span = self.span_from(&start_span);
                    return Expr {
                        kind: ExprKind::Await(Box::new(lhs)),
                        span,
                    };
                }
                // .( for closure call
                if self.at(TokenKind::LParen) {
                    let args = self.parse_call_args();
                    let span = self.span_from(&start_span);
                    return Expr {
                        kind: ExprKind::ClosureCall {
                            callee: Box::new(lhs),
                            args,
                        },
                        span,
                    };
                }

                // Tuple field access: `t.0`, `t.1`, …
                // After `.` the next token may be an integer literal that
                // names the tuple field. Treat the integer's decimal
                // digits as the field name so HIR/MIR see a `FieldAccess`
                // with a numeric field (tuple types already handle this
                // via typeck → GetField).
                let field = if let TokenKind::IntLiteral(val, _) = self.current_kind().clone() {
                    self.advance();
                    let name = val.to_string();
                    let span = self.span_from(&start_span);
                    return Expr {
                        kind: ExprKind::FieldAccess {
                            object: Box::new(lhs),
                            field: name,
                        },
                        span,
                    };
                } else if let TokenKind::FloatLiteral(val, _) = self.current_kind().clone() {
                    // `t.0.1` — the lexer fuses `0.1` into a single float
                    // literal. Split on `.` to produce two successive
                    // tuple-field accesses.
                    self.advance();
                    let s = format!("{}", val);
                    let mut parts = s.split('.');
                    let first = parts.next().unwrap_or("0").to_string();
                    let second = parts.next().unwrap_or("0").to_string();
                    let mid_span = self.span_from(&start_span);
                    let inner = Expr {
                        kind: ExprKind::FieldAccess {
                            object: Box::new(lhs),
                            field: first,
                        },
                        span: mid_span.clone(),
                    };
                    let span = self.span_from(&start_span);
                    return Expr {
                        kind: ExprKind::FieldAccess {
                            object: Box::new(inner),
                            field: second,
                        },
                        span,
                    };
                } else {
                    self.expect_any_identifier()
                };

                if self.at(TokenKind::LParen) {
                    let args = self.parse_call_args();
                    let block = self.maybe_parse_block_arg();
                    let span = self.span_from(&start_span);
                    Expr {
                        kind: ExprKind::MethodCall {
                            object: Box::new(lhs),
                            method: field,
                            args,
                            block: block.map(Box::new),
                        },
                        span,
                    }
                } else {
                    // Check for block arg after field access (method call with no parens but with block)
                    let block = self.maybe_parse_block_arg();
                    if block.is_some() {
                        let span = self.span_from(&start_span);
                        Expr {
                            kind: ExprKind::MethodCall {
                                object: Box::new(lhs),
                                method: field,
                                args: vec![],
                                block: block.map(Box::new),
                            },
                            span,
                        }
                    } else {
                        let span = self.span_from(&start_span);
                        Expr {
                            kind: ExprKind::FieldAccess {
                                object: Box::new(lhs),
                                field,
                            },
                            span,
                        }
                    }
                }
            }

            // Safe navigation: ?.name or ?.name(args)
            TokenKind::QuestionDot => {
                self.advance(); // consume ?.
                let field = self.expect_any_identifier();

                if self.at(TokenKind::LParen) {
                    let args = self.parse_call_args();
                    let span = self.span_from(&start_span);
                    Expr {
                        kind: ExprKind::SafeNavCall {
                            object: Box::new(lhs),
                            method: field,
                            args,
                        },
                        span,
                    }
                } else {
                    let span = self.span_from(&start_span);
                    Expr {
                        kind: ExprKind::SafeNav {
                            object: Box::new(lhs),
                            field,
                        },
                        span,
                    }
                }
            }

            // Indexing: [expr]
            TokenKind::LBracket => {
                self.advance(); // consume [
                self.skip_newlines();
                let index = self.parse_expression();
                self.skip_newlines();
                self.expect(TokenKind::RBracket);
                let span = self.span_from(&start_span);
                Expr {
                    kind: ExprKind::Index {
                        object: Box::new(lhs),
                        index: Box::new(index),
                    },
                    span,
                }
            }

            // Try: ?
            TokenKind::Question => {
                self.advance();
                let span = self.span_from(&start_span);
                Expr {
                    kind: ExprKind::Try(Box::new(lhs)),
                    span,
                }
            }

            // Function call: expr(args)
            TokenKind::LParen => {
                let args = self.parse_call_args();
                let block = self.maybe_parse_block_arg();
                let span = self.span_from(&start_span);
                Expr {
                    kind: ExprKind::Call {
                        callee: Box::new(lhs),
                        args,
                        block: block.map(Box::new),
                    },
                    span,
                }
            }

            _ => lhs,
        }
    }

    fn parse_infix(&mut self, lhs: Expr, op_kind: &TokenKind, r_bp: u8) -> Expr {
        let start_span = lhs.span.clone();
        let op = op_kind.clone();
        self.advance(); // consume operator
        self.skip_newlines();

        match op {
            // Assignment
            TokenKind::Eq => {
                let rhs = self.parse_expr_bp(r_bp);
                let span = self.span_from(&start_span);
                Expr {
                    kind: ExprKind::Assign {
                        target: Box::new(lhs),
                        value: Box::new(rhs),
                    },
                    span,
                }
            }

            // Compound assignment
            TokenKind::PlusEq => self.make_compound_assign(lhs, BinOp::Add, r_bp, &start_span),
            TokenKind::MinusEq => self.make_compound_assign(lhs, BinOp::Sub, r_bp, &start_span),
            TokenKind::StarEq => self.make_compound_assign(lhs, BinOp::Mul, r_bp, &start_span),
            TokenKind::SlashEq => self.make_compound_assign(lhs, BinOp::Div, r_bp, &start_span),
            TokenKind::PercentEq => self.make_compound_assign(lhs, BinOp::Mod, r_bp, &start_span),

            // Range
            TokenKind::DotDot => {
                let rhs = if self.is_expression_start() {
                    Some(Box::new(self.parse_expr_bp(r_bp)))
                } else {
                    None
                };
                let span = self.span_from(&start_span);
                Expr {
                    kind: ExprKind::Range {
                        start: Some(Box::new(lhs)),
                        end: rhs,
                        inclusive: false,
                    },
                    span,
                }
            }
            TokenKind::DotDotEq => {
                let rhs = self.parse_expr_bp(r_bp);
                let span = self.span_from(&start_span);
                Expr {
                    kind: ExprKind::Range {
                        start: Some(Box::new(lhs)),
                        end: Some(Box::new(rhs)),
                        inclusive: true,
                    },
                    span,
                }
            }

            // Cast
            TokenKind::As => {
                let target_type = self.parse_type();
                let span = self.span_from(&start_span);
                Expr {
                    kind: ExprKind::Cast {
                        expr: Box::new(lhs),
                        target_type,
                    },
                    span,
                }
            }

            // Binary operators
            _ => {
                let bin_op = token_to_binop(&op);
                let rhs = self.parse_expr_bp(r_bp);
                let span = self.span_from(&start_span);
                Expr {
                    kind: ExprKind::BinaryOp {
                        left: Box::new(lhs),
                        op: bin_op,
                        right: Box::new(rhs),
                    },
                    span,
                }
            }
        }
    }

    fn make_compound_assign(&mut self, lhs: Expr, op: BinOp, r_bp: u8, start: &Span) -> Expr {
        let rhs = self.parse_expr_bp(r_bp);
        let span = self.span_from(start);
        Expr {
            kind: ExprKind::CompoundAssign {
                target: Box::new(lhs),
                op,
                value: Box::new(rhs),
            },
            span,
        }
    }

    /// Check if current token starts a bare (paren-less) call argument.
    /// Only string literals and interpolated strings qualify — this prevents
    /// `x y` from being misread as a call when `x` is just a variable.
    /// Check if the next token can be a bare (no-parens) call argument.
    ///
    /// String literals are always accepted (any function can be called with
    /// a bare string: `puts "hello"`). Identifiers and other expression
    /// tokens are only accepted for known IO functions (`puts`, `print`,
    /// `eputs`) to avoid ambiguity where `foo bar` could be misread as
    /// `foo(bar)` when they are two separate expressions.
    fn is_bare_call_arg_start(&self, callee_name: &str) -> bool {
        // String literals are always valid bare args for any function.
        if matches!(
            self.current_kind(),
            TokenKind::StringLiteral(_) | TokenKind::InterpolatedString(_)
        ) {
            return true;
        }

        // For known IO/statement functions, allow broader expression args.
        let is_bare_call_fn = matches!(
            callee_name,
            "puts" | "print" | "eputs" | "require" | "include" | "raise"
        );
        if !is_bare_call_fn {
            return false;
        }

        matches!(
            self.current_kind(),
            TokenKind::Identifier(_)
                | TokenKind::TypeIdentifier(_)
                | TokenKind::SelfValue
                | TokenKind::IntLiteral(..)
                | TokenKind::FloatLiteral(..)
                | TokenKind::CharLiteral(_)
                | TokenKind::True
                | TokenKind::False
                | TokenKind::SomeKw
                | TokenKind::NoneKw
                | TokenKind::OkKw
                | TokenKind::ErrKw
                | TokenKind::Amp
                | TokenKind::AmpMut
                | TokenKind::Bang
        )
    }

    /// Check if current token could start an expression.
    pub(crate) fn is_expression_start(&self) -> bool {
        matches!(
            self.current_kind(),
            TokenKind::IntLiteral(..)
                | TokenKind::FloatLiteral(..)
                | TokenKind::StringLiteral(_)
                | TokenKind::InterpolatedString(_)
                | TokenKind::CharLiteral(_)
                | TokenKind::True
                | TokenKind::False
                | TokenKind::Identifier(_)
                | TokenKind::TypeIdentifier(_)
                | TokenKind::SelfValue
                | TokenKind::SelfType
                | TokenKind::SomeKw
                | TokenKind::NoneKw
                | TokenKind::OkKw
                | TokenKind::ErrKw
                | TokenKind::LParen
                | TokenKind::LBracket
                | TokenKind::LBrace
                | TokenKind::Minus
                | TokenKind::Bang
                | TokenKind::Amp
                | TokenKind::AmpMut
                | TokenKind::If
                | TokenKind::Match
                | TokenKind::While
                | TokenKind::For
                | TokenKind::Loop
                | TokenKind::Do
                | TokenKind::Move
                | TokenKind::Async
                | TokenKind::Return
                | TokenKind::Break
                | TokenKind::Continue
                | TokenKind::Yield
                | TokenKind::Super
                | TokenKind::Unsafe
                | TokenKind::Null
        )
    }

    // ─── Parenthesized / Tuple ─────────────────────────────────────────

    fn parse_paren_or_tuple(&mut self) -> Expr {
        let start = self.current_span();
        self.advance(); // consume (
        self.skip_newlines();

        // Unit literal: ()
        if self.at(TokenKind::RParen) {
            self.advance();
            return Expr {
                kind: ExprKind::UnitLiteral,
                span: self.span_from(&start),
            };
        }

        let first = self.parse_expression();
        self.skip_newlines();

        if self.eat(TokenKind::Comma) {
            // Tuple
            self.skip_newlines();
            let mut elements = vec![first];
            if !self.at(TokenKind::RParen) {
                elements.push(self.parse_expression());
                while self.eat(TokenKind::Comma) {
                    self.skip_newlines();
                    if self.at(TokenKind::RParen) {
                        break;
                    }
                    elements.push(self.parse_expression());
                }
            }
            self.skip_newlines();
            self.expect(TokenKind::RParen);
            let span = self.span_from(&start);
            Expr {
                kind: ExprKind::TupleLiteral(elements),
                span,
            }
        } else {
            // Parenthesized expression
            self.expect(TokenKind::RParen);
            first
        }
    }

    // ─── Array Literal ─────────────────────────────────────────────────

    fn parse_array_literal(&mut self) -> Expr {
        let start = self.current_span();
        self.advance(); // consume [
        self.skip_newlines();

        if self.at(TokenKind::RBracket) {
            self.advance();
            return Expr {
                kind: ExprKind::ArrayLiteral(vec![]),
                span: self.span_from(&start),
            };
        }

        let first = self.parse_expression();
        self.skip_newlines();

        // Array fill: [value; count]
        if self.eat(TokenKind::Semicolon) {
            self.skip_newlines();
            let count = self.parse_expression();
            self.skip_newlines();
            self.expect(TokenKind::RBracket);
            let span = self.span_from(&start);
            return Expr {
                kind: ExprKind::ArrayFill {
                    value: Box::new(first),
                    count: Box::new(count),
                },
                span,
            };
        }

        let mut elements = vec![first];
        while self.eat(TokenKind::Comma) {
            self.skip_newlines();
            if self.at(TokenKind::RBracket) {
                break;
            }
            elements.push(self.parse_expression());
            self.skip_newlines();
        }
        self.skip_newlines();
        self.expect(TokenKind::RBracket);
        let span = self.span_from(&start);
        Expr {
            kind: ExprKind::ArrayLiteral(elements),
            span,
        }
    }

    // ─── Closures ──────────────────────────────────────────────────────

    fn parse_brace_closure(&mut self, is_async: bool, is_move: bool) -> Expr {
        let start = self.current_span();
        self.advance(); // consume {
        self.skip_newlines();

        // Check for closure params: { |x, y| ... } or empty `||`
        let params = if self.at(TokenKind::Pipe) {
            self.parse_closure_params()
        } else if self.at(TokenKind::PipePipe) {
            // `||` is an empty parameter list (two pipes fused by the lexer).
            self.advance();
            vec![]
        } else {
            vec![]
        };
        self.skip_newlines();

        // Single expression closure: { |x| expr } or multi-statement
        // If we see a newline after params and more than one statement, it's a block
        // Otherwise single expr
        let body = if self.at(TokenKind::RBrace) {
            // Empty closure
            ClosureBody::Expr(Box::new(Expr {
                kind: ExprKind::UnitLiteral,
                span: self.current_span(),
            }))
        } else {
            // Try to parse as single expression, but may have newlines
            let expr = self.parse_expression();
            self.skip_newlines();
            if self.at(TokenKind::RBrace) {
                ClosureBody::Expr(Box::new(expr))
            } else {
                // Multiple statements — parse as block
                let mut stmts = vec![Statement::Expression(expr)];
                self.skip_newlines();
                while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
                    self.skip_newlines();
                    if self.at(TokenKind::RBrace) {
                        break;
                    }
                    stmts.push(self.parse_statement());
                    self.skip_newlines();
                }
                ClosureBody::Block(Block {
                    statements: stmts,
                    span: self.current_span(),
                })
            }
        };

        self.expect(TokenKind::RBrace);
        let span = self.span_from(&start);
        Expr {
            kind: ExprKind::Closure(ClosureExpr {
                is_async,
                is_move,
                params,
                body,
                span: span.clone(),
            }),
            span,
        }
    }

    /// Parse `do NL statements NL end` as a block expression.
    /// The value of the block is the value of its last expression,
    /// following the same tail-expression rule used by `resolve_block_as_expr`.
    fn parse_do_block_expr(&mut self) -> Expr {
        let start = self.current_span();
        self.advance(); // consume `do`
        self.skip_newlines();
        let body = self.parse_body();
        self.expect(TokenKind::End);
        let span = self.span_from(&start);
        Expr {
            kind: ExprKind::Block(body),
            span,
        }
    }

    fn parse_do_closure(&mut self, is_async: bool, is_move: bool) -> Expr {
        let start = self.current_span();
        self.advance(); // consume do
        self.skip_newlines();

        let params = if self.at(TokenKind::Pipe) {
            self.parse_closure_params()
        } else if self.at(TokenKind::PipePipe) {
            // `||` = empty closure params.
            self.advance();
            vec![]
        } else {
            vec![]
        };
        self.skip_newlines();

        let block = self.parse_body();
        self.expect(TokenKind::End);
        let span = self.span_from(&start);
        Expr {
            kind: ExprKind::Closure(ClosureExpr {
                is_async,
                is_move,
                params,
                body: ClosureBody::Block(block),
                span: span.clone(),
            }),
            span,
        }
    }

    fn parse_closure_params(&mut self) -> Vec<ClosureParam> {
        self.expect(TokenKind::Pipe);
        let mut params = Vec::new();
        if !self.at(TokenKind::Pipe) {
            params.push(self.parse_closure_param());
            while self.eat(TokenKind::Comma) {
                self.skip_newlines();
                if self.at(TokenKind::Pipe) {
                    break;
                }
                params.push(self.parse_closure_param());
            }
        }
        self.expect(TokenKind::Pipe);
        params
    }

    fn parse_closure_param(&mut self) -> ClosureParam {
        let start = self.current_span();
        let name = self.expect_any_identifier();
        let type_expr = if self.eat(TokenKind::Colon) {
            Some(self.parse_type())
        } else {
            None
        };
        let span = self.span_from(&start);
        ClosureParam {
            name,
            type_expr,
            span,
        }
    }

    /// True when the next tokens look like a trailing block argument
    /// (`do |params| ... end` or `{ |params| ... }`).  Used to recognize
    /// bare function calls whose only argument is a block, such as
    /// `with_x do |n| ... end` where `with_x` takes an implicit block.
    pub(crate) fn is_trailing_block_start(&self) -> bool {
        if self.at(TokenKind::Do) {
            // `do |...|` — unambiguously a block closure.  Plain `do ... end`
            // would be a standalone block expression, but bare identifiers
            // followed by `do` are otherwise meaningless, so treat both as
            // trailing blocks.
            return true;
        }
        if self.at(TokenKind::LBrace) {
            // Only treat `{ |` as a trailing block to avoid swallowing
            // struct-initializer or block-expression literals.
            let mut i = 1;
            while matches!(self.peek_at_kind(i), TokenKind::Newline) {
                i += 1;
            }
            return matches!(self.peek_at_kind(i), TokenKind::Pipe | TokenKind::PipePipe);
        }
        false
    }

    /// Try to parse a trailing block argument after a method call.
    /// Returns Some if { |params| ... } or do |params| ... end follows.
    pub(crate) fn maybe_parse_block_arg(&mut self) -> Option<Expr> {
        if self.at(TokenKind::LBrace) {
            Some(self.parse_brace_closure(false, false))
        } else if self.at(TokenKind::Do) {
            Some(self.parse_do_closure(false, false))
        } else {
            None
        }
    }

    // ─── Control Flow ──────────────────────────────────────────────────

    fn parse_if_expr(&mut self) -> Expr {
        let start = self.current_span();
        self.advance(); // consume if
        self.skip_newlines();

        // Check for if let
        if self.at(TokenKind::Let) {
            return self.parse_if_let_expr(start);
        }

        let condition = self.parse_expression();
        self.skip_newlines();
        let then_body = self.parse_body();

        let mut elsif_clauses = Vec::new();
        while self.at(TokenKind::Elsif) {
            let elsif_start = self.current_span();
            self.advance(); // consume elsif
            self.skip_newlines();
            let elsif_cond = self.parse_expression();
            self.skip_newlines();
            let elsif_body = self.parse_body();
            let elsif_span = self.span_from(&elsif_start);
            elsif_clauses.push(ElsifClause {
                condition: Box::new(elsif_cond),
                body: elsif_body,
                span: elsif_span,
            });
        }

        let else_body = if self.eat(TokenKind::Else) {
            self.skip_newlines();
            Some(self.parse_body())
        } else {
            None
        };

        self.expect(TokenKind::End);
        let span = self.span_from(&start);
        Expr {
            kind: ExprKind::If(IfExpr {
                condition: Box::new(condition),
                then_body,
                elsif_clauses,
                else_body,
                span: span.clone(),
            }),
            span,
        }
    }

    fn parse_if_let_expr(&mut self, start: Span) -> Expr {
        self.advance(); // consume let
        let pattern = self.parse_pattern();
        self.expect(TokenKind::Eq);
        self.skip_newlines();
        let value = self.parse_expression();
        self.skip_newlines();
        let then_body = self.parse_body();

        let else_body = if self.eat(TokenKind::Else) {
            self.skip_newlines();
            Some(self.parse_body())
        } else {
            None
        };

        self.expect(TokenKind::End);
        let span = self.span_from(&start);
        Expr {
            kind: ExprKind::IfLet(IfLetExpr {
                pattern,
                value: Box::new(value),
                then_body,
                else_body,
                span: span.clone(),
            }),
            span,
        }
    }

    fn parse_match_expr(&mut self) -> Expr {
        let start = self.current_span();
        self.advance(); // consume match
        self.skip_newlines();
        let subject = self.parse_expression();
        self.skip_newlines();

        let mut arms = Vec::new();
        while !self.at(TokenKind::End) && !self.at(TokenKind::Eof) {
            self.skip_newlines();
            if self.at(TokenKind::End) {
                break;
            }
            arms.push(self.parse_match_arm());
            self.skip_newlines();
        }

        self.expect(TokenKind::End);
        let span = self.span_from(&start);
        Expr {
            kind: ExprKind::Match(MatchExpr {
                subject: Box::new(subject),
                arms,
                span: span.clone(),
            }),
            span,
        }
    }

    fn parse_match_arm(&mut self) -> MatchArm {
        let start = self.current_span();
        let pattern = self.parse_pattern();

        let guard = if self.at(TokenKind::If) {
            self.advance();
            self.skip_newlines();
            Some(Box::new(self.parse_expression()))
        } else {
            None
        };

        self.expect(TokenKind::Arrow);
        self.skip_newlines();

        // Arm body: single expression or block (multiple statements until next arm / end)
        let body = if self.is_expression_start() {
            let expr = self.parse_expression();
            MatchArmBody::Expr(expr)
        } else {
            let block = self.parse_body();
            MatchArmBody::Block(block)
        };

        let span = self.span_from(&start);
        MatchArm {
            pattern,
            guard,
            body,
            span,
        }
    }

    fn parse_while_expr(&mut self) -> Expr {
        let start = self.current_span();
        self.advance(); // consume while
        self.skip_newlines();

        // Check for while let
        if self.at(TokenKind::Let) {
            self.advance();
            let pattern = self.parse_pattern();
            self.expect(TokenKind::Eq);
            self.skip_newlines();
            let value = self.parse_expression();
            self.skip_newlines();
            let body = self.parse_body();
            self.expect(TokenKind::End);
            let span = self.span_from(&start);
            return Expr {
                kind: ExprKind::WhileLet(WhileLetExpr {
                    pattern,
                    value: Box::new(value),
                    body,
                    span: span.clone(),
                }),
                span,
            };
        }

        let condition = self.parse_expression();
        self.skip_newlines();
        let body = self.parse_body();
        self.expect(TokenKind::End);
        let span = self.span_from(&start);
        Expr {
            kind: ExprKind::While(WhileExpr {
                condition: Box::new(condition),
                body,
                span: span.clone(),
            }),
            span,
        }
    }

    fn parse_for_expr(&mut self) -> Expr {
        let start = self.current_span();
        self.advance(); // consume for
        self.skip_newlines();
        let pattern = self.parse_pattern();
        self.expect(TokenKind::In);
        self.skip_newlines();
        let iterable = self.parse_expression();
        self.skip_newlines();
        let body = self.parse_body();
        self.expect(TokenKind::End);
        let span = self.span_from(&start);
        Expr {
            kind: ExprKind::For(ForExpr {
                pattern,
                iterable: Box::new(iterable),
                body,
                span: span.clone(),
            }),
            span,
        }
    }

    fn parse_loop_expr(&mut self) -> Expr {
        let start = self.current_span();
        self.advance(); // consume loop
        self.skip_newlines();
        let body = self.parse_body();
        self.expect(TokenKind::End);
        let span = self.span_from(&start);
        Expr {
            kind: ExprKind::Loop(LoopExpr {
                body,
                span: span.clone(),
            }),
            span,
        }
    }
}

fn token_to_binop(kind: &TokenKind) -> BinOp {
    match kind {
        TokenKind::Plus => BinOp::Add,
        TokenKind::Minus => BinOp::Sub,
        TokenKind::Star => BinOp::Mul,
        TokenKind::Slash => BinOp::Div,
        TokenKind::Percent => BinOp::Mod,
        TokenKind::EqEq => BinOp::Eq,
        TokenKind::NotEq => BinOp::NotEq,
        TokenKind::Lt => BinOp::Lt,
        TokenKind::Gt => BinOp::Gt,
        TokenKind::LtEq => BinOp::LtEq,
        TokenKind::GtEq => BinOp::GtEq,
        TokenKind::AmpAmp => BinOp::And,
        TokenKind::PipePipe => BinOp::Or,
        TokenKind::Amp => BinOp::BitAnd,
        TokenKind::Pipe => BinOp::BitOr,
        TokenKind::Caret => BinOp::BitXor,
        TokenKind::Shl => BinOp::Shl,
        TokenKind::Shr => BinOp::Shr,
        _ => unreachable!("not a binary operator: {:?}", kind),
    }
}
