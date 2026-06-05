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
}
