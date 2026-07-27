//! Infix operator parsing: binary/compound-assign/range/cast dispatch.
//! Binding-power tables and prefix handling live in `mod.rs`.

use super::*;

impl Parser {
    pub(super) fn parse_infix(&mut self, lhs: Expr, op_kind: &TokenKind, r_bp: u8) -> Expr {
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
            TokenKind::ShlEq => self.make_compound_assign(lhs, BinOp::Shl, r_bp, &start_span),
            TokenKind::ShrEq => self.make_compound_assign(lhs, BinOp::Shr, r_bp, &start_span),
            TokenKind::AmpEq => self.make_compound_assign(lhs, BinOp::BitAnd, r_bp, &start_span),
            TokenKind::PipeEq => self.make_compound_assign(lhs, BinOp::BitOr, r_bp, &start_span),
            TokenKind::CaretEq => self.make_compound_assign(lhs, BinOp::BitXor, r_bp, &start_span),

            // Conditional assignment: `a ||= b` / `a &&= b`. Unlike `+=`, these
            // short-circuit AND only assign when needed, so they desugar to
            // `a || (a = b)` / `a && (a = b)` rather than `a = a op b`.
            TokenKind::PipePipeEq => {
                self.make_conditional_assign(lhs, BinOp::Or, r_bp, &start_span)
            }
            TokenKind::AmpAmpEq => self.make_conditional_assign(lhs, BinOp::And, r_bp, &start_span),

            // Range — Ruby semantics (ruby-naming.spec.md §3.10b):
            // `..` is INCLUSIVE, `...` is EXCLUSIVE. The Rust `..=` form
            // is retired and rejected with a fix-it.
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
                        inclusive: true,
                    },
                    span,
                }
            }
            TokenKind::DotDotDot => {
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

    /// `a ||= b` desugars to `a || (a = b)` and `a &&= b` to `a && (a = b)`.
    /// The logical operator short-circuits (see `lower_short_circuit`), so the
    /// inner `a = b` runs only when the value is actually replaced: `a` false
    /// for `||=`, `a` true for `&&=`. Operands are Bool (Ruxen is strict-Bool).
    /// Option fallback uses the separate `??` operator; `??=` is not yet
    /// available. The target is evaluated twice (the read and the assign) as in
    /// Ruby, so keep side-effecting targets in mind.
    fn make_conditional_assign(&mut self, lhs: Expr, op: BinOp, r_bp: u8, start: &Span) -> Expr {
        let rhs = self.parse_expr_bp(r_bp);
        let span = self.span_from(start);
        let assign = Expr {
            kind: ExprKind::Assign {
                target: Box::new(lhs.clone()),
                value: Box::new(rhs),
            },
            span: span.clone(),
        };
        Expr {
            kind: ExprKind::BinaryOp {
                left: Box::new(lhs),
                op,
                right: Box::new(assign),
            },
            span,
        }
    }
}
