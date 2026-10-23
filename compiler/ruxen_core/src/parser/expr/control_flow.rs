//! Control-flow expressions: if/elsif/else, if-let, while/while-let, for, loop.
//! Return/break/continue/yield literals are produced inside `atoms::parse_primary`
//! since they share its dispatch.

use super::*;

impl Parser {
    pub(super) fn parse_if_expr(&mut self) -> Expr {
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

    /// `cond ? then_expr : else_expr` ternary, desugared to an `if`/`else`
    /// expression. The leading `?` (already confirmed to be ternary by the
    /// caller's whitespace-adjacency check) is consumed here. Right-associative:
    /// `a ? b : c ? d : e` parses as `a ? b : (c ? d : e)`. Kept as a parser
    /// desugar; a surface-preserving `Ternary` node for `ruxen fmt` is a
    /// follow-up (today fmt renders the `if`/`else` block form).
    pub(super) fn parse_ternary(&mut self, cond: Expr) -> Expr {
        let start = cond.span.clone();
        self.advance(); // consume `?`
        self.skip_newlines();
        let then_expr = self.parse_expression(); // stops at `:` (not an infix op)
        self.skip_newlines();
        self.expect(TokenKind::Colon);
        self.skip_newlines();
        // Parse the else branch at the ternary tier so a chained ternary nests
        // to the right.
        let else_expr = self.parse_expr_bp(3);
        let span = self.span_from(&start);
        let then_body = Block {
            statements: vec![Statement::Expression(then_expr)],
            span: span.clone(),
        };
        let else_body = Block {
            statements: vec![Statement::Expression(else_expr)],
            span: span.clone(),
        };
        Expr {
            kind: ExprKind::If(IfExpr {
                condition: Box::new(cond),
                then_body,
                elsif_clauses: Vec::new(),
                else_body: Some(else_body),
                span: span.clone(),
            }),
            span,
        }
    }

    /// `unless COND ... [else ...] end` desugars to `if !COND ... [else ...] end`.
    /// Ruby forbids `elsif` after `unless`, so none is parsed.
    pub(super) fn parse_unless_expr(&mut self) -> Expr {
        let start = self.current_span();
        self.advance(); // consume unless
        self.skip_newlines();

        let condition = self.parse_expression();
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
            kind: ExprKind::If(IfExpr {
                condition: Box::new(negate_condition(condition)),
                then_body,
                elsif_clauses: Vec::new(),
                else_body,
                span: span.clone(),
            }),
            span,
        }
    }

    /// `until COND ... end` desugars to `while !COND ... end`.
    pub(super) fn parse_until_expr(&mut self) -> Expr {
        let start = self.current_span();
        self.advance(); // consume until
        self.skip_newlines();

        let condition = self.parse_expression();
        self.skip_newlines();
        let body = self.parse_body();
        self.expect(TokenKind::End);
        let span = self.span_from(&start);
        Expr {
            kind: ExprKind::While(WhileExpr {
                condition: Box::new(negate_condition(condition)),
                body,
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

    pub(super) fn parse_while_expr(&mut self) -> Expr {
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

    pub(super) fn parse_for_expr(&mut self) -> Expr {
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

    pub(super) fn parse_loop_expr(&mut self) -> Expr {
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

/// Wrap a condition in a logical `!` for the `unless`/`until` desugars (and the
/// `unless`/`until` statement modifiers). Reuses the same span so diagnostics
/// point at the original condition.
pub(in crate::parser) fn negate_condition(condition: Expr) -> Expr {
    let span = condition.span.clone();
    Expr {
        kind: ExprKind::UnaryOp {
            op: UnaryOp::Not,
            operand: Box::new(condition),
        },
        span,
    }
}
