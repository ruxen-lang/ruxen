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
