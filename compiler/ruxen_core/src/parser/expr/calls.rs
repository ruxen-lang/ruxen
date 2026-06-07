//! Postfix call/index/field/safe-nav handling plus the helpers used by
//! identifiers and methods to recognise bare-call and trailing-block forms.

use super::*;

impl Parser {
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

    pub(super) fn parse_postfix(&mut self, lhs: Expr) -> Expr {
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
                    // Operator-symbol method calls (`a.+(b)`, `a.[](i)`,
                    // `a.-@()`) parse here. The desugar pass (Step 3)
                    // produces these same `MethodCall` names, and a user
                    // may also write the explicit form directly.
                    self.try_parse_operator_name()
                        .unwrap_or_else(|| self.expect_any_identifier())
                };

                let generic_args = if self.at(TokenKind::LBracket) {
                    self.parse_generic_args()
                } else {
                    vec![]
                };

                if self.at(TokenKind::LParen) {
                    let args = self.parse_call_args();
                    let block = self.maybe_parse_block_arg();
                    let span = self.span_from(&start_span);
                    Expr {
                        kind: ExprKind::MethodCall {
                            object: Box::new(lhs),
                            method: field,
                            generic_args,
                            args,
                            block: block.map(Box::new),
                        },
                        span,
                    }
                } else {
                    // Check for block arg after field access (method call with no parens but with block)
                    let block = self.maybe_parse_block_arg();
                    if block.is_some() || !generic_args.is_empty() {
                        let span = self.span_from(&start_span);
                        Expr {
                            kind: ExprKind::MethodCall {
                                object: Box::new(lhs),
                                method: field,
                                generic_args,
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
            TokenKind::AmpDot => {
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
    pub(super) fn is_bare_call_arg_start(&self, callee_name: &str) -> bool {
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
                | TokenKind::OkKw
                | TokenKind::ErrKw
                | TokenKind::Amp
                | TokenKind::AmpMut
                | TokenKind::Bang
        )
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
}
