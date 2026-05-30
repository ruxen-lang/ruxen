//! Expression parsing for the Ruxen language using Pratt-style precedence climbing.
//!
//! The dispatcher (`parse_expression`, `parse_expr_bp`, prefix handling, and the
//! binding-power tables) lives in this file. Larger surfaces are split out:
//!
//! * [`atoms`] — primaries: literals, identifiers, type/enum constructors,
//!   macro calls, parenthesised/tuple/array/map literals, and closures.
//! * [`calls`] — postfix `.method`, `?.field`, `(args)`, `[idx]`, `?` plus the
//!   bare-call / trailing-block helpers.
//! * [`operators`] — infix operator handling (binary, compound assign, range,
//!   cast).
//! * [`match_expr`] — `match ... end` expressions.
//! * [`control_flow`] — `if`/`while`/`for`/`loop` (return/break/continue/yield
//!   still produced in `atoms::parse_primary`).

use crate::lexer::token::{Span, TokenKind};
use crate::parser::ast::*;
use crate::parser::Parser;

mod atoms;
mod calls;
mod control_flow;
mod match_expr;
mod operators;

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

        // Comparison + regex-match: non-associative (7, 8). `~=` sits
        // at the same precedence as `==`/`!=` per the std.regex spec.
        TokenKind::EqEq
        | TokenKind::NotEq
        | TokenKind::Lt
        | TokenKind::Gt
        | TokenKind::LtEq
        | TokenKind::GtEq
        | TokenKind::TildeEq => Some((7, 8)),

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
    pub(super) fn parse_expr_bp(&mut self, min_bp: u8) -> Expr {
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

    /// Check if the current token starts a statement that is NOT
    /// also a valid expression starter (`let` / `var`). Used by the
    /// closure-body parser to switch directly to block mode when
    /// the body opens with a let-binding — `parse_expression` can't
    /// handle a leading `let`, so without this gate we'd fail with
    /// "expected expression, found Let" on every multi-statement
    /// closure that declares a local on its first line.
    pub(crate) fn is_statement_keyword_start(&self) -> bool {
        matches!(self.current_kind(), TokenKind::Let | TokenKind::Var)
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
                | TokenKind::RegexLiteral { .. }
                | TokenKind::True
                | TokenKind::False
                | TokenKind::Identifier(_)
                | TokenKind::TypeIdentifier(_)
                | TokenKind::SelfValue
                | TokenKind::SelfType
                | TokenKind::SomeKw
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
                | TokenKind::Nil
        )
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
        TokenKind::TildeEq => BinOp::MatchOp,
        _ => unreachable!("not a binary operator: {:?}", kind),
    }
}
