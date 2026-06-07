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

        // Q7: `-> do … end` is the explicit multi-statement BLOCK arm form
        // (decision: `{ expr }` stays a single-expression arm; `do … end` is
        // the block). Parse the block directly so the expression parser
        // doesn't take `do` as a closure literal (which produced a spurious
        // `Fn() -> T` arm type). A block arm is also not a closure, so the
        // arm's body sees the surrounding bindings live (no stale-capture).
        if self.at(TokenKind::Do) {
            self.advance(); // consume `do`
            self.skip_newlines();
            let block = self.parse_body();
            self.expect(TokenKind::End);
            let span = self.span_from(&start);
            return MatchArm {
                pattern,
                guard,
                body: MatchArmBody::Block(block),
                span,
            };
        }

        // Arm body: always go through `parse_match_arm_body`, which
        // collects statements until the next sibling arm header,
        // closing `end`, or EOF (using the `looks_like_sibling_match_arm`
        // lookahead to avoid greedily consuming the next arm's
        // pattern). A single-expression arm naturally produces a
        // one-statement block; multi-statement arms also parse
        // correctly without needing a leading `let`/`var` to flip
        // the parser into block mode.
        //
        // Pre-fix: this branched on `is_expression_start` and called
        // `parse_expression` for the common single-expr case. That
        // worked for `Some(x) -> x` but silently dropped subsequent
        // statements in shapes like
        //   Some(stream) ->
        //     Thread.spawn({ || handle(stream) })
        //     count = count + 1
        // because `parse_expression` returned after `Thread.spawn(...)`
        // and the trailing `count = count + 1` was misread as a
        // sibling arm header (failing on the next pattern's arrow).
        // Pin: `docs/rondo_v1_blockers.md` B10.
        let block = self.parse_match_arm_body();
        let body = if block.statements.len() == 1 {
            // Single-statement arms keep the historical
            // `MatchArmBody::Expr` shape so the formatter / pretty-
            // printer don't switch to block-style indentation on
            // every `Some(x) -> x` arm in existing fixtures.
            let mut stmts = block.statements;
            match stmts.pop().unwrap() {
                Statement::Expression(e) => MatchArmBody::Expr(e),
                other => MatchArmBody::Block(Block {
                    statements: vec![other],
                    span: block.span,
                }),
            }
        } else {
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
