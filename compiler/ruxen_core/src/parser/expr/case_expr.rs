//! `case … when/in … end` parsing.
//!
//! `case/when` is Ruby's `===` dispatch, desugared here to an `if`/`elsif`
//! chain (`when v` → `subject == v`, `when a, b` → `subject == a || subject ==
//! b`, `when lo..hi` → a range-membership test). `case/in` is Ruby's pattern
//! matching, desugared to the same `Match` AST the `match` keyword produces.
//!
//! Both are parser-level desugars, so resolve / typeck / MIR are unchanged.
//! `ruxen fmt` therefore renders the desugared `if`/`match` form; a
//! surface-preserving `Case` node is a follow-up.

use super::*;

impl Parser {
    pub(super) fn parse_case_expr(&mut self) -> Expr {
        let start = self.current_span();
        self.advance(); // consume `case`
        self.skip_newlines();
        let subject = self.parse_expression();
        self.skip_newlines();
        // `in` clauses select pattern matching; `when` clauses select the
        // `===`-style value/range dispatch.
        if self.at(TokenKind::In) {
            self.parse_case_in(subject, start)
        } else {
            self.parse_case_when(subject, start)
        }
    }

    /// `case s / when a, b then A / when lo..hi then B / else C / end`
    /// desugars to `if s == a || s == b then A elsif <lo..hi contains s> then B
    /// else C end`. The subject is re-evaluated per branch (v1; bind it to a
    /// variable first if it is expensive or side-effecting).
    fn parse_case_when(&mut self, subject: Expr, start: Span) -> Expr {
        let mut branches: Vec<(Expr, Block)> = Vec::new();
        while self.at(TokenKind::When) {
            self.advance(); // `when`
            self.skip_newlines();
            let mut values = vec![self.parse_expr_bp(2)];
            while self.eat(TokenKind::Comma) {
                self.skip_newlines();
                values.push(self.parse_expr_bp(2));
            }
            self.eat(TokenKind::Then); // optional `then`
            self.skip_newlines();
            let body = self.parse_case_body();
            let condition = build_when_condition(&subject, values);
            branches.push((condition, body));
        }
        let else_body = if self.eat(TokenKind::Else) {
            self.skip_newlines();
            Some(self.parse_case_body())
        } else {
            None
        };
        self.expect(TokenKind::End);
        let span = self.span_from(&start);

        if branches.is_empty() {
            self.error("`case` expression needs at least one `when` clause");
        }

        let mut iter = branches.into_iter();
        let (first_cond, first_body) = iter.next().unwrap_or_else(|| {
            (
                Expr {
                    kind: ExprKind::BoolLiteral(false),
                    span: span.clone(),
                },
                Block {
                    statements: Vec::new(),
                    span: span.clone(),
                },
            )
        });
        let elsif_clauses: Vec<ElsifClause> = iter
            .map(|(cond, body)| ElsifClause {
                condition: Box::new(cond),
                body,
                span: span.clone(),
            })
            .collect();
        Expr {
            kind: ExprKind::If(IfExpr {
                condition: Box::new(first_cond),
                then_body: first_body,
                elsif_clauses,
                else_body,
                span: span.clone(),
            }),
            span,
        }
    }

    /// `case s / in pat [if guard] then body / … / else body / end` desugars to
    /// the same `Match` AST as `match s / pat -> body / … / end`. `else`
    /// becomes a wildcard arm.
    fn parse_case_in(&mut self, subject: Expr, start: Span) -> Expr {
        let mut arms = Vec::new();
        while self.at(TokenKind::In) {
            let arm_start = self.current_span();
            self.advance(); // `in`
            self.skip_newlines();
            let pattern = self.parse_pattern();
            let guard = if self.at(TokenKind::If) {
                self.advance();
                self.skip_newlines();
                Some(Box::new(self.parse_expression()))
            } else {
                None
            };
            // Separator: `then`, `->`, or just a newline.
            if self.at(TokenKind::Then) || self.at(TokenKind::Arrow) {
                self.advance();
            }
            self.skip_newlines();
            let block = self.parse_case_body();
            let body = arm_body_from_block(block);
            let span = self.span_from(&arm_start);
            arms.push(MatchArm {
                pattern,
                guard,
                body,
                span,
            });
        }
        if self.eat(TokenKind::Else) {
            let arm_start = self.current_span();
            self.skip_newlines();
            let block = self.parse_case_body();
            let body = arm_body_from_block(block);
            let span = self.span_from(&arm_start);
            arms.push(MatchArm {
                pattern: Pattern::Wildcard { span: span.clone() },
                guard: None,
                body,
                span,
            });
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

    /// Parse one clause body: statements up to the next `when` / `in` / `else`
    /// / `end`.
    fn parse_case_body(&mut self) -> Block {
        let start = self.current_span();
        let mut statements = Vec::new();
        self.skip_newlines();
        while !self.at(TokenKind::When)
            && !self.at(TokenKind::In)
            && !self.at(TokenKind::Else)
            && !self.at(TokenKind::End)
            && !self.at(TokenKind::Eof)
        {
            let progress = self.pos;
            statements.push(self.parse_statement());
            self.skip_newlines();
            self.ensure_loop_progress(progress);
        }
        let span = self.span_from(&start);
        Block { statements, span }
    }
}

/// A one-statement clause body keeps the `Expr` arm shape (matching how the
/// `match` parser renders `pat -> expr`); multi-statement bodies stay blocks.
fn arm_body_from_block(block: Block) -> MatchArmBody {
    if block.statements.len() == 1 {
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
    }
}

fn make_binop(left: Expr, op: BinOp, right: Expr) -> Expr {
    let span = left.span.clone();
    Expr {
        kind: ExprKind::BinaryOp {
            left: Box::new(left),
            op,
            right: Box::new(right),
        },
        span,
    }
}

/// OR together each value's test to form one `when` clause's condition.
fn build_when_condition(subject: &Expr, values: Vec<Expr>) -> Expr {
    let mut cond: Option<Expr> = None;
    for v in values {
        let c = when_value_condition(subject, v);
        cond = Some(match cond {
            None => c,
            Some(prev) => make_binop(prev, BinOp::Or, c),
        });
    }
    cond.unwrap_or_else(|| Expr {
        kind: ExprKind::BoolLiteral(false),
        span: subject.span.clone(),
    })
}

/// The test for one `when` value: a range literal becomes a membership test
/// (`subject >= lo && subject <= hi`, half-open for `...`), any other value
/// becomes `subject == value`.
fn when_value_condition(subject: &Expr, value: Expr) -> Expr {
    if let ExprKind::Range {
        start,
        end,
        inclusive,
    } = value.kind
    {
        let mut parts: Vec<Expr> = Vec::new();
        if let Some(lo) = start {
            parts.push(make_binop(subject.clone(), BinOp::GtEq, *lo));
        }
        if let Some(hi) = end {
            let op = if inclusive { BinOp::LtEq } else { BinOp::Lt };
            parts.push(make_binop(subject.clone(), op, *hi));
        }
        let mut cond: Option<Expr> = None;
        for p in parts {
            cond = Some(match cond {
                None => p,
                Some(prev) => make_binop(prev, BinOp::And, p),
            });
        }
        cond.unwrap_or_else(|| Expr {
            kind: ExprKind::BoolLiteral(true),
            span: subject.span.clone(),
        })
    } else {
        make_binop(subject.clone(), BinOp::Eq, value)
    }
}
