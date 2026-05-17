//! `match ... end` expression parsing. Patterns themselves live in
//! `parser/patterns.rs`; this module only handles arm structure.

use super::*;

impl Parser {
    pub(super) fn parse_match_expr(&mut self) -> Expr {
        let start = self.current_span();
        self.advance(); // consume match
        self.skip_newlines();
        let subject = self.parse_expression();
        self.skip_newlines();

        let mut arms = Vec::new();
        while !self.at(TokenKind::End) && !self.at(TokenKind::Eof) {
            let __progress = self.pos;
            self.skip_newlines();
            if self.at(TokenKind::End) {
                break;
            }
            arms.push(self.parse_match_arm());
            self.skip_newlines();
            self.ensure_loop_progress(__progress);
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
}
