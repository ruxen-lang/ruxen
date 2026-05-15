//! Pattern parsing for the Riven language.

use crate::lexer::token::TokenKind;
use crate::parser::ast::*;
use crate::parser::Parser;

impl Parser {
    /// Parse a pattern. Handles or-patterns at the top level.
    pub(crate) fn parse_pattern(&mut self) -> Pattern {
        let first = self.parse_single_pattern();
        self.maybe_parse_or_pattern(first)
    }

    /// If we see `|`, parse an or-pattern wrapping the first pattern.
    fn maybe_parse_or_pattern(&mut self, first: Pattern) -> Pattern {
        if !self.at(TokenKind::Pipe) {
            return first;
        }
        let start_span = self.span_of_pattern(&first);
        let mut patterns = vec![first];
        while self.eat(TokenKind::Pipe) {
            self.skip_newlines();
            patterns.push(self.parse_single_pattern());
        }
        let span = self.span_from(&start_span);
        Pattern::Or { patterns, span }
    }

    /// Parse a single pattern (no or).
    fn parse_single_pattern(&mut self) -> Pattern {
        self.skip_newlines();
        let start = self.current_span();

        match self.current_kind().clone() {
            // Wildcard: _
            TokenKind::Identifier(ref name) if name == "_" => {
                let span = self.current_span();
                self.advance();
                Pattern::Wildcard { span }
            }

            // Rest: ..
            TokenKind::DotDot => {
                let span = self.current_span();
                self.advance();
                Pattern::Rest { span }
            }

            // Ref pattern: ref [mut] name
            TokenKind::Ref => {
                self.advance();
                let mutable = self.eat(TokenKind::Mut);
                let name = self.expect_identifier();
                let span = self.span_from(&start);
                Pattern::Ref {
                    mutable,
                    name,
                    span,
                }
            }

            // mut name (mutable identifier binding)
            TokenKind::Mut => {
                self.advance();
                let name = self.expect_identifier();
                let span = self.span_from(&start);
                Pattern::Identifier {
                    mutable: true,
                    name,
                    span,
                }
            }

            // Tuple pattern: (pat, pat, ...)
            TokenKind::LParen => self.parse_tuple_pattern(),

            // Boolean literals
            TokenKind::True => {
                let span = self.current_span();
                self.advance();
                Pattern::Literal {
                    expr: Box::new(Expr {
                        kind: ExprKind::BoolLiteral(true),
                        span: span.clone(),
                    }),
                    span,
                }
            }
            TokenKind::False => {
                let span = self.current_span();
                self.advance();
                Pattern::Literal {
                    expr: Box::new(Expr {
                        kind: ExprKind::BoolLiteral(false),
                        span: span.clone(),
                    }),
                    span,
                }
            }

            // Numeric literals
            TokenKind::IntLiteral(val, suffix) => {
                let span = self.current_span();
                self.advance();
                Pattern::Literal {
                    expr: Box::new(Expr {
                        kind: ExprKind::IntLiteral(val, suffix),
                        span: span.clone(),
                    }),
                    span,
                }
            }
            TokenKind::FloatLiteral(val, suffix) => {
                let span = self.current_span();
                self.advance();
                Pattern::Literal {
                    expr: Box::new(Expr {
                        kind: ExprKind::FloatLiteral(val, suffix),
                        span: span.clone(),
                    }),
                    span,
                }
            }

            // String literal
            TokenKind::StringLiteral(ref val) => {
                let val = val.clone();
                let span = self.current_span();
                self.advance();
                Pattern::Literal {
                    expr: Box::new(Expr {
                        kind: ExprKind::StringLiteral(val),
                        span: span.clone(),
                    }),
                    span,
                }
            }

            // Negative numeric literal
            TokenKind::Minus => {
                self.advance();
                match self.current_kind().clone() {
                    TokenKind::IntLiteral(val, suffix) => {
                        self.advance();
                        let span = self.span_from(&start);
                        Pattern::Literal {
                            expr: Box::new(Expr {
                                kind: ExprKind::IntLiteral(-val, suffix),
                                span: span.clone(),
                            }),
                            span,
                        }
                    }
                    TokenKind::FloatLiteral(val, suffix) => {
                        self.advance();
                        let span = self.span_from(&start);
                        Pattern::Literal {
                            expr: Box::new(Expr {
                                kind: ExprKind::FloatLiteral(-val, suffix),
                                span: span.clone(),
                            }),
                            span,
                        }
                    }
                    _ => {
                        self.error("expected numeric literal after `-` in pattern");
                        Pattern::Wildcard { span: start }
                    }
                }
            }

            // Type identifier: could be enum pattern or struct pattern
            // e.g., Status.InProgress(who), TaskError.NotFound(id: id), Some(x), None, Ok(x), Err(e)
            TokenKind::TypeIdentifier(ref name) => {
                let name = name.clone();
                self.parse_type_pattern(name)
            }

            // Some, None, Ok, Err — special keywords that act like type identifiers in patterns
            TokenKind::SomeKw => {
                self.advance();
                if self.at(TokenKind::LParen) {
                    self.advance();
                    let inner = self.parse_pattern();
                    self.expect(TokenKind::RParen);
                    let span = self.span_from(&start);
                    Pattern::Enum {
                        path: vec![],
                        variant: "Some".to_string(),
                        fields: vec![inner],
                        span,
                    }
                } else {
                    let span = self.span_from(&start);
                    Pattern::Enum {
                        path: vec![],
                        variant: "Some".to_string(),
                        fields: vec![],
                        span,
                    }
                }
            }
            // `nil` (ruby-naming.spec.md §3.10) binds to `Option::None`
            // in pattern position. The same token is also used for the
            // raw-pointer null; the resolver disambiguates by the
            // scrutinee's type.
            TokenKind::Nil => {
                let span = self.current_span();
                self.advance();
                Pattern::Enum {
                    path: vec![],
                    variant: "None".to_string(),
                    fields: vec![],
                    span,
                }
            }
            TokenKind::OkKw => {
                self.advance();
                if self.at(TokenKind::LParen) {
                    self.advance();
                    let inner = self.parse_pattern();
                    self.expect(TokenKind::RParen);
                    let span = self.span_from(&start);
                    Pattern::Enum {
                        path: vec![],
                        variant: "Ok".to_string(),
                        fields: vec![inner],
                        span,
                    }
                } else {
                    let span = self.span_from(&start);
                    Pattern::Enum {
                        path: vec![],
                        variant: "Ok".to_string(),
                        fields: vec![],
                        span,
                    }
                }
            }
            TokenKind::ErrKw => {
                self.advance();
                if self.at(TokenKind::LParen) {
                    self.advance();
                    let inner = self.parse_pattern();
                    self.expect(TokenKind::RParen);
                    let span = self.span_from(&start);
                    Pattern::Enum {
                        path: vec![],
                        variant: "Err".to_string(),
                        fields: vec![inner],
                        span,
                    }
                } else {
                    let span = self.span_from(&start);
                    Pattern::Enum {
                        path: vec![],
                        variant: "Err".to_string(),
                        fields: vec![],
                        span,
                    }
                }
            }

            // Plain identifier: variable binding
            TokenKind::Identifier(ref name) => {
                let name = name.clone();
                let span = self.current_span();
                self.advance();
                Pattern::Identifier {
                    mutable: false,
                    name,
                    span,
                }
            }

            _ => {
                self.error(&format!(
                    "expected pattern, found {:?}",
                    self.current_kind()
                ));
                Pattern::Wildcard { span: start }
            }
        }
    }

    /// Parse a type-prefixed pattern: TypeName.Variant(...) or TypeName(fields)
    fn parse_type_pattern(&mut self, first_name: String) -> Pattern {
        let start = self.current_span();
        self.advance(); // consume the TypeIdentifier

        if self.at(TokenKind::Dot) {
            // Enum pattern: Path.Variant(...)
            let mut path = vec![first_name];

            // Collect path segments and final variant. Accept the stdlib
            // variant keywords (Some/None/Ok/Err) as variant names so that
            // a user enum which re-uses those names (e.g.
            // `enum MyOpt[T] { Some(T), None }`) can be matched with
            // `MyOpt.Some(n)` / `MyOpt.None`.
            while self.eat(TokenKind::Dot) {
                match self.current_kind().clone() {
                    TokenKind::TypeIdentifier(name) => {
                        path.push(name);
                        self.advance();
                    }
                    TokenKind::SomeKw => {
                        path.push("Some".to_string());
                        self.advance();
                    }
                    TokenKind::OkKw => {
                        path.push("Ok".to_string());
                        self.advance();
                    }
                    TokenKind::ErrKw => {
                        path.push("Err".to_string());
                        self.advance();
                    }
                    _ => {
                        self.error("expected variant name after `.`");
                        break;
                    }
                }
            }

            let variant = path.pop().unwrap_or_default();

            let fields = if self.at(TokenKind::LParen) {
                self.parse_pattern_list()
            } else {
                vec![]
            };

            let span = self.span_from(&start);
            Pattern::Enum {
                path,
                variant,
                fields,
                span,
            }
        } else if self.at(TokenKind::LParen) {
            // Could be struct pattern: TypeName(field: pat, ...) or enum variant without path
            // Disambiguate: if the first element has `name:` then struct, else enum
            let fields = self.parse_struct_or_enum_pattern_fields();
            let span = self.span_from(&start);

            // Check if any field has a name — if so, treat as struct pattern
            let has_named = fields.iter().any(|f| f.name.is_some());
            let has_rest = fields
                .iter()
                .any(|f| matches!(f.pattern, Pattern::Rest { .. }));

            if has_named || has_rest {
                Pattern::Struct {
                    path: vec![first_name],
                    fields,
                    rest: has_rest,
                    span,
                }
            } else {
                // Treat as enum variant
                let pats: Vec<Pattern> = fields.into_iter().map(|f| f.pattern).collect();
                Pattern::Enum {
                    path: vec![],
                    variant: first_name,
                    fields: pats,
                    span,
                }
            }
        } else {
            // Just a unit variant or type name used as pattern
            let span = self.span_from(&start);
            Pattern::Enum {
                path: vec![],
                variant: first_name,
                fields: vec![],
                span,
            }
        }
    }

    fn parse_pattern_list(&mut self) -> Vec<Pattern> {
        self.expect(TokenKind::LParen);
        self.skip_newlines();
        let mut patterns = Vec::new();
        if !self.at(TokenKind::RParen) {
            patterns.push(self.parse_pattern());
            while self.eat(TokenKind::Comma) {
                self.skip_newlines();
                if self.at(TokenKind::RParen) {
                    break;
                }
                patterns.push(self.parse_pattern());
            }
        }
        self.skip_newlines();
        self.expect(TokenKind::RParen);
        patterns
    }

    fn parse_struct_or_enum_pattern_fields(&mut self) -> Vec<PatternField> {
        self.expect(TokenKind::LParen);
        self.skip_newlines();
        let mut fields = Vec::new();
        if !self.at(TokenKind::RParen) {
            fields.push(self.parse_pattern_field());
            while self.eat(TokenKind::Comma) {
                self.skip_newlines();
                if self.at(TokenKind::RParen) {
                    break;
                }
                fields.push(self.parse_pattern_field());
            }
        }
        self.skip_newlines();
        self.expect(TokenKind::RParen);
        fields
    }

    fn parse_pattern_field(&mut self) -> PatternField {
        let start = self.current_span();
        self.skip_newlines();

        // Check for rest: ..
        if self.at(TokenKind::DotDot) {
            let span = self.current_span();
            self.advance();
            return PatternField {
                name: None,
                pattern: Pattern::Rest { span: span.clone() },
                span,
            };
        }

        // Check for named field: name: pattern
        if let TokenKind::Identifier(ref name) = self.current_kind().clone() {
            let name_val = name.clone();
            if self.peek_kind() == TokenKind::Colon {
                self.advance(); // consume name
                self.advance(); // consume :
                self.skip_newlines();
                let pattern = self.parse_pattern();
                let span = self.span_from(&start);
                return PatternField {
                    name: Some(name_val),
                    pattern,
                    span,
                };
            }
        }

        // Just a pattern
        let pattern = self.parse_pattern();
        let span = self.span_from(&start);
        PatternField {
            name: None,
            pattern,
            span,
        }
    }

    fn parse_tuple_pattern(&mut self) -> Pattern {
        let start = self.current_span();
        self.advance(); // consume (
        self.skip_newlines();

        // Check for unit pattern ()
        if self.at(TokenKind::RParen) {
            let span = self.span_from(&start);
            self.advance();
            return Pattern::Tuple {
                elements: vec![],
                span,
            };
        }

        let mut elements = vec![self.parse_pattern()];
        while self.eat(TokenKind::Comma) {
            self.skip_newlines();
            if self.at(TokenKind::RParen) {
                break;
            }
            elements.push(self.parse_pattern());
        }
        self.skip_newlines();
        self.expect(TokenKind::RParen);
        let span = self.span_from(&start);
        Pattern::Tuple { elements, span }
    }

    /// Helper to get the span of a pattern.
    fn span_of_pattern(&self, pat: &Pattern) -> crate::lexer::token::Span {
        match pat {
            Pattern::Literal { span, .. }
            | Pattern::Identifier { span, .. }
            | Pattern::Wildcard { span }
            | Pattern::Tuple { span, .. }
            | Pattern::Enum { span, .. }
            | Pattern::Struct { span, .. }
            | Pattern::Or { span, .. }
            | Pattern::Ref { span, .. }
            | Pattern::Rest { span } => span.clone(),
        }
    }
}
