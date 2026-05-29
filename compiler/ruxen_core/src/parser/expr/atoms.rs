//! Primary expression parsing: literals, identifiers, type expressions,
//! enum constructors, macro calls, parenthesised / tuple / array / map
//! literals, and closures (`do…end` and `{…}`).

use super::*;

impl Parser {
    pub(super) fn parse_primary(&mut self) -> Expr {
        self.skip_newlines();
        let start = self.current_span();
        let kind = self.current_kind().clone();

        match kind {
            // Literals
            TokenKind::IntLiteral(val, suffix) => {
                self.advance();
                Expr {
                    kind: ExprKind::IntLiteral(val, suffix),
                    span: start,
                }
            }
            TokenKind::FloatLiteral(val, suffix) => {
                self.advance();
                Expr {
                    kind: ExprKind::FloatLiteral(val, suffix),
                    span: start,
                }
            }
            TokenKind::StringLiteral(ref val) => {
                let val = val.clone();
                self.advance();
                Expr {
                    kind: ExprKind::StringLiteral(val),
                    span: start,
                }
            }
            TokenKind::InterpolatedString(ref parts) => {
                let parts = parts.clone();
                self.advance();
                Expr {
                    kind: ExprKind::InterpolatedString(parts),
                    span: start,
                }
            }
            TokenKind::CharLiteral(val) => {
                self.advance();
                Expr {
                    kind: ExprKind::CharLiteral(val),
                    span: start,
                }
            }
            TokenKind::RegexLiteral {
                ref pattern,
                ref flags,
            } => {
                let pattern = pattern.clone();
                let flags = flags.clone();
                self.advance();
                Expr {
                    kind: ExprKind::RegexLiteral { pattern, flags },
                    span: start,
                }
            }
            TokenKind::True => {
                self.advance();
                Expr {
                    kind: ExprKind::BoolLiteral(true),
                    span: start,
                }
            }
            TokenKind::False => {
                self.advance();
                Expr {
                    kind: ExprKind::BoolLiteral(false),
                    span: start,
                }
            }

            // self
            TokenKind::SelfValue => {
                self.advance();
                Expr {
                    kind: ExprKind::SelfRef,
                    span: start,
                }
            }

            // Self
            TokenKind::SelfType => {
                self.advance();
                Expr {
                    kind: ExprKind::SelfType,
                    span: start,
                }
            }

            // Some, Ok, Err — used as enum constructors. `None` is spelled
            // `nil` after ruby-naming.spec.md §3.10, lexed as `TokenKind::Nil`
            // and handled in the nil-literal arm below.
            TokenKind::SomeKw => {
                self.advance();
                self.parse_constructor_args("Some", vec![], start)
            }
            TokenKind::OkKw => {
                self.advance();
                self.parse_constructor_args("Ok", vec![], start)
            }
            TokenKind::ErrKw => {
                self.advance();
                self.parse_constructor_args("Err", vec![], start)
            }

            // Type identifier — could be enum constructor, type path, etc.
            TokenKind::TypeIdentifier(ref name) => {
                let name = name.clone();
                self.advance();
                self.parse_type_expr_primary(name, start)
            }

            // Plain identifier
            TokenKind::Identifier(ref name) => {
                let name = name.clone();
                self.advance();
                // Check for macro call: name!(...) — also handle ident ending with !
                if name.ends_with('!') {
                    // Already has !, parse macro args
                    let trimmed = name.trim_end_matches('!').to_string();
                    self.parse_macro_call_args(trimmed, start)
                } else if self.at(TokenKind::Bang) {
                    // name followed by !
                    self.advance(); // consume !
                    self.parse_macro_call_args(name, start)
                } else if self.at(TokenKind::LParen) {
                    // Function call
                    let args = self.parse_call_args();
                    let block = self.maybe_parse_block_arg();
                    let span = self.span_from(&start);
                    Expr {
                        kind: ExprKind::Call {
                            callee: Box::new(Expr {
                                kind: ExprKind::Identifier(name),
                                span: start,
                            }),
                            args,
                            block: block.map(Box::new),
                        },
                        span,
                    }
                } else if self.is_bare_call_arg_start(&name) {
                    // Bare function call without parens: `puts "hello"`, `puts msg`
                    let arg = self.parse_expression();
                    let span = self.span_from(&start);
                    Expr {
                        kind: ExprKind::Call {
                            callee: Box::new(Expr {
                                kind: ExprKind::Identifier(name),
                                span: start,
                            }),
                            args: vec![arg],
                            block: None,
                        },
                        span,
                    }
                } else if self.is_trailing_block_start() {
                    // Bare function call with a trailing block only, no parens
                    // and no other arguments: `with_x do |n| ... end` or
                    // `with_x { |n| ... }`.
                    let block = self.maybe_parse_block_arg();
                    let span = self.span_from(&start);
                    Expr {
                        kind: ExprKind::Call {
                            callee: Box::new(Expr {
                                kind: ExprKind::Identifier(name),
                                span: start,
                            }),
                            args: vec![],
                            block: block.map(Box::new),
                        },
                        span,
                    }
                } else {
                    Expr {
                        kind: ExprKind::Identifier(name),
                        span: start,
                    }
                }
            }

            // Parenthesized expression or tuple
            TokenKind::LParen => self.parse_paren_or_tuple(),

            // Array literal
            TokenKind::LBracket => self.parse_array_literal(),

            // `{ k => v, ... }` Map literal or `{ |x| ... }` brace closure.
            TokenKind::LBrace => self.parse_brace_map_or_closure(false, false),

            // do ... end — either a closure (if `|params|` follow) or a
            // block expression whose value is the last expression.
            TokenKind::Do => {
                // Peek past newlines for closure-param markers.
                let mut look = 1;
                while matches!(self.peek_at_kind(look), TokenKind::Newline) {
                    look += 1;
                }
                if matches!(
                    self.peek_at_kind(look),
                    TokenKind::Pipe | TokenKind::PipePipe
                ) {
                    self.parse_do_closure(false, false)
                } else {
                    self.parse_do_block_expr()
                }
            }

            // move closure
            TokenKind::Move => {
                self.advance();
                if self.at(TokenKind::LBrace) {
                    self.parse_brace_closure(false, true)
                } else if self.at(TokenKind::Do) {
                    self.parse_do_closure(false, true)
                } else {
                    self.error("expected `{` or `do` after `move`");
                    Expr {
                        kind: ExprKind::Identifier("_error".to_string()),
                        span: start,
                    }
                }
            }

            TokenKind::Async => {
                self.advance();
                let is_move = self.eat(TokenKind::Move);
                if self.at(TokenKind::LBrace) {
                    self.parse_brace_closure(true, is_move)
                } else if self.at(TokenKind::Do) {
                    self.parse_do_closure(true, is_move)
                } else {
                    self.error("expected `def`, `do`, or `{` after `async`");
                    Expr {
                        kind: ExprKind::Identifier("_error".to_string()),
                        span: start,
                    }
                }
            }

            TokenKind::Await => {
                self.advance();
                self.error("prefix `await` is not supported; use postfix `.await`");
                Expr {
                    kind: ExprKind::Identifier("_error".to_string()),
                    span: start,
                }
            }

            // Unsafe block: `unsafe ... end`
            TokenKind::Unsafe => {
                self.advance(); // consume `unsafe`
                self.skip_newlines();
                let body = self.parse_body();
                self.expect(TokenKind::End);
                let span = self.span_from(&start);
                Expr {
                    kind: ExprKind::UnsafeBlock(body),
                    span,
                }
            }

            // `nil` literal — replaces legacy `null` (raw pointer) and the
            // `None` constructor under ruby-naming. Lowers to NullLiteral;
            // the type checker reconciles `nil` against the expected type
            // (raw pointer types take it as null; `Option[T]` takes it as
            // `None`).
            TokenKind::Nil => {
                self.advance();
                Expr {
                    kind: ExprKind::NullLiteral,
                    span: start,
                }
            }

            // Control flow expressions
            TokenKind::If => self.parse_if_expr(),
            TokenKind::Match => self.parse_match_expr(),
            TokenKind::While => self.parse_while_expr(),
            TokenKind::For => self.parse_for_expr(),
            TokenKind::Loop => self.parse_loop_expr(),

            // Return, break, continue
            TokenKind::Return => {
                self.advance();
                let value = if self.is_expression_start() {
                    Some(Box::new(self.parse_expression()))
                } else {
                    None
                };
                let span = self.span_from(&start);
                Expr {
                    kind: ExprKind::Return(value),
                    span,
                }
            }
            TokenKind::Break => {
                self.advance();
                let value = if self.is_expression_start() {
                    Some(Box::new(self.parse_expression()))
                } else {
                    None
                };
                let span = self.span_from(&start);
                Expr {
                    kind: ExprKind::Break(value),
                    span,
                }
            }
            TokenKind::Continue => {
                self.advance();
                Expr {
                    kind: ExprKind::Continue,
                    span: start,
                }
            }

            // Yield
            TokenKind::Yield => {
                self.advance();
                let mut args = Vec::new();
                if self.is_expression_start() {
                    args.push(self.parse_expression());
                    while self.eat(TokenKind::Comma) {
                        self.skip_newlines();
                        args.push(self.parse_expression());
                    }
                }
                let span = self.span_from(&start);
                Expr {
                    kind: ExprKind::Yield(args),
                    span,
                }
            }

            // Super (for constructor calls like super(...))
            TokenKind::Super => {
                self.advance();
                if self.at(TokenKind::LParen) {
                    let args = self.parse_call_args();
                    let span = self.span_from(&start);
                    Expr {
                        kind: ExprKind::Call {
                            callee: Box::new(Expr {
                                kind: ExprKind::Identifier("super".to_string()),
                                span: start,
                            }),
                            args,
                            block: None,
                        },
                        span,
                    }
                } else {
                    Expr {
                        kind: ExprKind::Identifier("super".to_string()),
                        span: start,
                    }
                }
            }

            _ => {
                self.error(&format!(
                    "expected expression, found {:?}",
                    self.current_kind()
                ));
                self.advance();
                Expr {
                    kind: ExprKind::Identifier("_error".to_string()),
                    span: start,
                }
            }
        }
    }

    /// Heuristic: the current `[` opens a list of type arguments, not an
    /// indexing expression. True when the token immediately after `[` is a
    /// TypeIdentifier, Self, or a lifetime and the matching `]` is followed
    /// by `.` or `(` (a method call or constructor).
    fn looks_like_type_args(&self) -> bool {
        if !matches!(self.current_kind(), TokenKind::LBracket) {
            return false;
        }
        // Peek past any newlines after `[`.
        let mut idx = 1;
        while matches!(self.peek_at_kind(idx), TokenKind::Newline) {
            idx += 1;
        }
        // First token after `[` should look like a type or a const-arg.
        //
        // T2.02 S8 follow-up: integer literals are accepted as
        // const-generic arguments at use sites (`Counter[10].new(...)`).
        // Without this lookahead arm the parser misreads such forms as
        // an index expression, which downstream produces a `<error>`
        // receiver and `_<error>_init` link failures.
        let first = self.peek_at_kind(idx);
        if !matches!(
            first,
            TokenKind::TypeIdentifier(_)
                | TokenKind::SelfType
                | TokenKind::Lifetime(_)
                | TokenKind::Amp
                | TokenKind::AmpMut
                | TokenKind::IntLiteral(_, _)
        ) {
            return false;
        }
        // Scan for the matching `]`, tracking bracket depth.
        let mut depth: i32 = 1;
        let mut j = idx;
        while depth > 0 {
            match self.peek_at_kind(j) {
                TokenKind::LBracket => depth += 1,
                TokenKind::RBracket => depth -= 1,
                TokenKind::Eof => return false,
                _ => {}
            }
            j += 1;
            if j > 256 {
                return false; // safety bound
            }
        }
        // After the matching `]` (j is one past it), check for `.` or `(`.
        let after = self.peek_at_kind(j);
        matches!(after, TokenKind::Dot | TokenKind::LParen)
    }

    /// After seeing a TypeIdentifier, parse the rest of the primary.
    /// This handles: TypeName.method/field, TypeName.Variant(...), TypeName[GenericArgs](...), TypeName.new(...)
    fn parse_type_expr_primary(&mut self, name: String, start: Span) -> Expr {
        // Check for generic type arguments: Name[T, U].method(...)
        // Distinguish type-application from indexing: a type-application has
        // one or more type-like tokens (TypeIdentifier/Self) inside. Generic
        // args are erased — they're inferred from constructor/method args.
        if self.at(TokenKind::LBracket) && self.looks_like_type_args() {
            let _generic_args = self.parse_generic_args();
            // Fall through to normal handling as if we had just the TypeIdentifier.
        }

        // Check if followed by . — could be enum variant construction or static method
        if self.at(TokenKind::Dot) {
            // Peek what follows the dot
            let after_dot = self.peek_at_kind(1);
            // The stdlib variant keywords (Some/None/Ok/Err) also serve as
            // variant names for user-defined generic enums that re-use
            // those identifiers (e.g. `enum MyOpt[T] { Some(T), None }`),
            // so treat them like TypeIdentifiers for `Type.Variant(...)`
            // and `Type.Variant` parsing.
            let is_variant_kw = matches!(
                after_dot,
                TokenKind::SomeKw | TokenKind::OkKw | TokenKind::ErrKw
            );
            if is_variant_kw {
                // `Type.Some` / `Type.Ok` / `Type.Err` — user-defined
                // generic enums that re-use a stdlib keyword as a variant
                // name. `Type.None` is spelled `Type.nil` (§3.10).
                self.advance(); // consume .
                let variant_name = match self.current_kind() {
                    TokenKind::SomeKw => "Some".to_string(),
                    TokenKind::OkKw => "Ok".to_string(),
                    TokenKind::ErrKw => "Err".to_string(),
                    _ => {
                        self.error("expected variant name");
                        "_error".to_string()
                    }
                };
                self.advance(); // consume the keyword
                let path = vec![name.clone()];
                if self.at(TokenKind::LParen) {
                    let args = self.parse_field_args();
                    let span = self.span_from(&start);
                    Expr {
                        kind: ExprKind::EnumVariant {
                            type_path: path,
                            variant: variant_name,
                            args,
                        },
                        span,
                    }
                } else {
                    let span = self.span_from(&start);
                    Expr {
                        kind: ExprKind::EnumVariant {
                            type_path: path,
                            variant: variant_name,
                            args: vec![],
                        },
                        span,
                    }
                }
            } else {
                match after_dot {
                    // TypeName.Variant or TypeName.method
                    TokenKind::TypeIdentifier(_) => {
                        // Enum variant: Status.InProgress(...)
                        self.advance(); // consume .
                        let mut path = vec![name.clone()];
                        let variant_name;
                        // Collect path: A.B.C — all TypeIdentifiers
                        loop {
                            if let TokenKind::TypeIdentifier(ref vname) =
                                self.current_kind().clone()
                            {
                                let vname = vname.clone();
                                self.advance();
                                if self.at(TokenKind::Dot) {
                                    if let TokenKind::TypeIdentifier(_) = self.peek_kind() {
                                        path.push(vname);
                                        self.advance(); // consume .
                                        continue;
                                    }
                                }
                                variant_name = vname;
                                break;
                            } else {
                                self.error("expected variant name");
                                variant_name = "_error".to_string();
                                break;
                            }
                        }

                        // #06.93 Phase 3: detect the qualified-type-with-
                        // method-call shape. If after `Outer.Inner` we are
                        // sitting on `.<lowercase>`, the user wrote
                        // `Outer.Inner.method(args)` — a static method
                        // call on a module-qualified class, NOT an enum
                        // variant. Push `variant_name` onto the path and
                        // emit a dotted-Identifier so the postfix layer
                        // picks up `.method(args)` as a MethodCall on the
                        // qualified receiver. The resolver recognises
                        // dotted identifiers as qualified type
                        // references (see `resolve_expr`'s MethodCall
                        // arm).
                        let next_is_lower_method = self.at(TokenKind::Dot)
                            && matches!(self.peek_kind(), TokenKind::Identifier(_));
                        if next_is_lower_method {
                            path.push(variant_name);
                            let span = self.span_from(&start);
                            Expr {
                                kind: ExprKind::Identifier(path.join(".")),
                                span,
                            }
                        } else if self.at(TokenKind::LParen) {
                            let args = self.parse_field_args();
                            let span = self.span_from(&start);
                            Expr {
                                kind: ExprKind::EnumVariant {
                                    type_path: path,
                                    variant: variant_name,
                                    args,
                                },
                                span,
                            }
                        } else {
                            // Unit variant
                            let span = self.span_from(&start);
                            Expr {
                                kind: ExprKind::EnumVariant {
                                    type_path: path,
                                    variant: variant_name,
                                    args: vec![],
                                },
                                span,
                            }
                        }
                    }
                    _ => {
                        // TypeName.method(...) or TypeName.field
                        // Return the type as an identifier, postfix will handle .method
                        Expr {
                            kind: ExprKind::Identifier(name),
                            span: start,
                        }
                    }
                }
            }
        } else if self.at(TokenKind::LParen) {
            // TypeName(...) — constructor call
            let args = self.parse_call_args();
            let block = self.maybe_parse_block_arg();
            let span = self.span_from(&start);
            Expr {
                kind: ExprKind::Call {
                    callee: Box::new(Expr {
                        kind: ExprKind::Identifier(name),
                        span: start,
                    }),
                    args,
                    block: block.map(Box::new),
                },
                span,
            }
        } else {
            Expr {
                kind: ExprKind::Identifier(name),
                span: start,
            }
        }
    }

    /// Parse constructor/enum variant arguments: `(expr, name: expr, ...)`
    fn parse_field_args(&mut self) -> Vec<FieldArg> {
        self.expect(TokenKind::LParen);
        self.skip_newlines();
        let mut args = Vec::new();

        if !self.at(TokenKind::RParen) {
            args.push(self.parse_field_arg());
            while self.eat(TokenKind::Comma) {
                self.skip_newlines();
                if self.at(TokenKind::RParen) {
                    break;
                }
                args.push(self.parse_field_arg());
            }
        }
        self.skip_newlines();
        self.expect(TokenKind::RParen);
        args
    }

    fn parse_field_arg(&mut self) -> FieldArg {
        let start = self.current_span();
        self.skip_newlines();

        // Check for named field: name: expr
        if let TokenKind::Identifier(ref name) = self.current_kind().clone() {
            let name_val = name.clone();
            if self.peek_kind() == TokenKind::Colon {
                self.advance(); // consume name
                self.advance(); // consume :
                self.skip_newlines();
                let value = self.parse_expression();
                let span = self.span_from(&start);
                return FieldArg {
                    name: Some(name_val),
                    value,
                    span,
                };
            }
        }

        let value = self.parse_expression();
        let span = self.span_from(&start);
        FieldArg {
            name: None,
            value,
            span,
        }
    }

    fn parse_constructor_args(&mut self, name: &str, type_path: Vec<String>, start: Span) -> Expr {
        if self.at(TokenKind::LParen) {
            let args = self.parse_field_args();
            let span = self.span_from(&start);
            Expr {
                kind: ExprKind::EnumVariant {
                    type_path,
                    variant: name.to_string(),
                    args,
                },
                span,
            }
        } else {
            Expr {
                kind: ExprKind::Identifier(name.to_string()),
                span: start,
            }
        }
    }

    fn parse_macro_call_args(&mut self, name: String, start: Span) -> Expr {
        let (args, delimiter) = match self.current_kind() {
            TokenKind::LParen => {
                let args = self.parse_call_args();
                (args, MacroDelimiter::Paren)
            }
            TokenKind::LBracket => {
                self.advance();
                self.skip_newlines();
                let mut args = Vec::new();
                if !self.at(TokenKind::RBracket) {
                    args.push(self.parse_expression());
                    while self.eat(TokenKind::Comma) {
                        self.skip_newlines();
                        if self.at(TokenKind::RBracket) {
                            break;
                        }
                        args.push(self.parse_expression());
                    }
                }
                self.skip_newlines();
                self.expect(TokenKind::RBracket);
                (args, MacroDelimiter::Bracket)
            }
            TokenKind::LBrace => {
                self.advance();
                self.skip_newlines();
                let mut args = Vec::new();
                // ruby-naming.spec.md §10a: the `map!{…}` / `hash!{…}`
                // macros are retired — Map literals are spelled bare
                // `{ k => v, … }` directly. Any `name!{ k => v }` form
                // still reaching the parser is an unrecognised macro and
                // should not get the special key/value flattening.
                let is_hash_macro = false;
                if !self.at(TokenKind::RBrace) {
                    let first = self.parse_expression();
                    if is_hash_macro && self.at(TokenKind::FatArrow) {
                        self.advance(); // =>
                        self.skip_newlines();
                        let value = self.parse_expression();
                        args.push(first);
                        args.push(value);
                        while self.eat(TokenKind::Comma) {
                            self.skip_newlines();
                            if self.at(TokenKind::RBrace) {
                                break;
                            }
                            let k = self.parse_expression();
                            self.expect(TokenKind::FatArrow);
                            self.skip_newlines();
                            let v = self.parse_expression();
                            args.push(k);
                            args.push(v);
                        }
                    } else {
                        args.push(first);
                        while self.eat(TokenKind::Comma) {
                            self.skip_newlines();
                            if self.at(TokenKind::RBrace) {
                                break;
                            }
                            args.push(self.parse_expression());
                        }
                    }
                }
                self.skip_newlines();
                self.expect(TokenKind::RBrace);
                (args, MacroDelimiter::Brace)
            }
            _ => {
                // Macro with no args
                (vec![], MacroDelimiter::Paren)
            }
        };

        let span = self.span_from(&start);
        Expr {
            kind: ExprKind::MacroCall {
                name,
                args,
                delimiter,
            },
            span,
        }
    }

    // ─── Parenthesized / Tuple ─────────────────────────────────────────

    fn parse_paren_or_tuple(&mut self) -> Expr {
        let start = self.current_span();
        self.advance(); // consume (
        self.skip_newlines();

        // Per ruby-naming.spec.md: Ruxen is Ruby-faithful, not Rust-
        // shaped. The Rust unit literal `()` is NOT part of the user
        // surface — Ruby uses `nil` or an empty block in every
        // position a Rustacean would write `()`. Refuse `()` at parse
        // time with a hint pointing at the canonical Ruby spelling.
        // The internal `ExprKind::UnitLiteral` variant stays for
        // synthesised expressions (empty closure body, async-lowering
        // fillers); only the user-written form is removed.
        if self.at(TokenKind::RParen) {
            let span = self.span_from(&start);
            self.error_at(
                "`()` is not valid Ruxen syntax (Ruby uses `nil` or an empty block — \
                 use `nil` here)",
                span.clone(),
            );
            self.advance();
            return Expr {
                kind: ExprKind::NullLiteral,
                span,
            };
        }

        let first = self.parse_expression();
        self.skip_newlines();

        if self.eat(TokenKind::Comma) {
            // Tuple
            self.skip_newlines();
            let mut elements = vec![first];
            if !self.at(TokenKind::RParen) {
                elements.push(self.parse_expression());
                while self.eat(TokenKind::Comma) {
                    self.skip_newlines();
                    if self.at(TokenKind::RParen) {
                        break;
                    }
                    elements.push(self.parse_expression());
                }
            }
            self.skip_newlines();
            self.expect(TokenKind::RParen);
            let span = self.span_from(&start);
            Expr {
                kind: ExprKind::TupleLiteral(elements),
                span,
            }
        } else {
            // Parenthesized expression
            self.expect(TokenKind::RParen);
            first
        }
    }

    // ─── Array Literal ─────────────────────────────────────────────────

    fn parse_array_literal(&mut self) -> Expr {
        let start = self.current_span();
        self.advance(); // consume [
        self.skip_newlines();

        if self.at(TokenKind::RBracket) {
            self.advance();
            return Expr {
                kind: ExprKind::ArrayLiteral(vec![]),
                span: self.span_from(&start),
            };
        }

        let first = self.parse_expression();
        self.skip_newlines();

        // Array fill: [value; count]
        if self.eat(TokenKind::Semicolon) {
            self.skip_newlines();
            let count = self.parse_expression();
            self.skip_newlines();
            self.expect(TokenKind::RBracket);
            let span = self.span_from(&start);
            return Expr {
                kind: ExprKind::ArrayFill {
                    value: Box::new(first),
                    count: Box::new(count),
                },
                span,
            };
        }

        let mut elements = vec![first];
        while self.eat(TokenKind::Comma) {
            self.skip_newlines();
            if self.at(TokenKind::RBracket) {
                break;
            }
            elements.push(self.parse_expression());
            self.skip_newlines();
        }
        self.skip_newlines();
        self.expect(TokenKind::RBracket);
        let span = self.span_from(&start);
        Expr {
            kind: ExprKind::ArrayLiteral(elements),
            span,
        }
    }

    // ─── Closures ──────────────────────────────────────────────────────

    /// Dispatch `{ ... }` between a Map literal and a brace closure.
    /// `|`-led or `||`-led contents always mean a closure; an arrow-free
    /// body is a closure; otherwise we parse the first key, look for
    /// `=>`, and if found commit to a Map literal.
    fn parse_brace_map_or_closure(&mut self, is_async: bool, is_move: bool) -> Expr {
        // Peek past the `{` and any newlines to decide.
        let mut look = 1;
        while matches!(self.peek_at_kind(look), TokenKind::Newline) {
            look += 1;
        }
        let first = self.peek_at_kind(look);
        if matches!(first, TokenKind::Pipe | TokenKind::PipePipe) {
            return self.parse_brace_closure(is_async, is_move);
        }
        if matches!(first, TokenKind::RBrace) {
            // `{}` — empty closure (preserves legacy meaning; the empty
            // Map is spelled `Map.new`).
            return self.parse_brace_closure(is_async, is_move);
        }
        // Not a closure-param marker — scan for a top-level `=>` to decide
        // between Map literal and brace block / single-expr closure.
        if self.brace_body_is_map_literal(look) {
            return self.parse_map_literal();
        }
        self.parse_brace_closure(is_async, is_move)
    }

    /// Look ahead from the first content token inside `{ ... }` and return
    /// `true` when we find a `=>` at brace-depth zero before the closing
    /// `}`. Tracks paren / bracket / brace nesting so `{ "x" => f(a => b) }`
    /// still parses as a Map even though `a => b` appears inside the call.
    fn brace_body_is_map_literal(&self, mut at: usize) -> bool {
        let mut depth_paren = 0;
        let mut depth_bracket = 0;
        let mut depth_brace = 0;
        loop {
            match self.peek_at_kind(at) {
                TokenKind::FatArrow
                    if depth_paren == 0 && depth_bracket == 0 && depth_brace == 0 =>
                {
                    return true;
                }
                TokenKind::LParen => depth_paren += 1,
                TokenKind::RParen => {
                    if depth_paren == 0 {
                        return false;
                    }
                    depth_paren -= 1;
                }
                TokenKind::LBracket => depth_bracket += 1,
                TokenKind::RBracket => {
                    if depth_bracket == 0 {
                        return false;
                    }
                    depth_bracket -= 1;
                }
                TokenKind::LBrace => depth_brace += 1,
                TokenKind::RBrace => {
                    if depth_brace == 0 {
                        return false;
                    }
                    depth_brace -= 1;
                }
                TokenKind::Eof => return false,
                _ => {}
            }
            at += 1;
        }
    }

    /// Parse `{ k => v, k => v, ... }` as a `MapLiteral`. Assumes the
    /// caller has decided this is a Map literal (via lookahead).
    fn parse_map_literal(&mut self) -> Expr {
        let start = self.current_span();
        self.advance(); // {
        self.skip_newlines();
        let mut entries = Vec::new();
        if !self.at(TokenKind::RBrace) {
            let k = self.parse_expression();
            self.expect(TokenKind::FatArrow);
            self.skip_newlines();
            let v = self.parse_expression();
            entries.push((k, v));
            while self.eat(TokenKind::Comma) {
                self.skip_newlines();
                if self.at(TokenKind::RBrace) {
                    break;
                }
                let k = self.parse_expression();
                self.expect(TokenKind::FatArrow);
                self.skip_newlines();
                let v = self.parse_expression();
                entries.push((k, v));
                self.skip_newlines();
            }
        }
        self.skip_newlines();
        self.expect(TokenKind::RBrace);
        let span = self.span_from(&start);
        Expr {
            kind: ExprKind::MapLiteral(entries),
            span,
        }
    }

    pub(super) fn parse_brace_closure(&mut self, is_async: bool, is_move: bool) -> Expr {
        let start = self.current_span();
        self.advance(); // consume {
        self.skip_newlines();

        // Check for closure params: { |x, y| ... } or empty `||`
        let params = if self.at(TokenKind::Pipe) {
            self.parse_closure_params()
        } else if self.at(TokenKind::PipePipe) {
            // `||` is an empty parameter list (two pipes fused by the lexer).
            self.advance();
            vec![]
        } else {
            vec![]
        };
        self.skip_newlines();

        // Closure body has three shapes:
        //   1. Empty: `{ || }` / `{ |x| }`
        //   2. Single expression: `{ |x| x + 1 }`
        //   3. Multi-statement block: `{ |x| let y = ...; ... ; tail_expr }`
        //
        // Shape 3 is detected when the first token after the param
        // header is a statement-starting keyword (`let`/`var`/`if`/
        // `while`/`for`/`match`/`return`/`break`/`continue`).
        // `parse_expression` doesn't know how to start with those, so
        // we'd otherwise fail with "expected expression, found Let".
        // For non-keyword starts (expression-start), we still try
        // single-expression first and fall through to block mode if
        // additional statements follow before `}`.
        let body = if self.at(TokenKind::RBrace) {
            // Empty closure
            ClosureBody::Expr(Box::new(Expr {
                kind: ExprKind::UnitLiteral,
                span: self.current_span(),
            }))
        } else if self.is_statement_keyword_start() {
            // Multi-statement block — body opens with `let`/`var`/etc.
            // Parse statements until the closing brace.
            let mut stmts = Vec::new();
            while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
                let __progress = self.pos;
                self.skip_newlines();
                if self.at(TokenKind::RBrace) {
                    break;
                }
                stmts.push(self.parse_statement());
                self.skip_newlines();
                self.ensure_loop_progress(__progress);
            }
            ClosureBody::Block(Block {
                statements: stmts,
                span: self.current_span(),
            })
        } else {
            // Try to parse as single expression, but may have newlines
            let expr = self.parse_expression();
            self.skip_newlines();
            if self.at(TokenKind::RBrace) {
                ClosureBody::Expr(Box::new(expr))
            } else {
                // Multiple statements — parse as block
                let mut stmts = vec![Statement::Expression(expr)];
                self.skip_newlines();
                while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
                    let __progress = self.pos;
                    self.skip_newlines();
                    if self.at(TokenKind::RBrace) {
                        break;
                    }
                    stmts.push(self.parse_statement());
                    self.skip_newlines();
                    self.ensure_loop_progress(__progress);
                }
                ClosureBody::Block(Block {
                    statements: stmts,
                    span: self.current_span(),
                })
            }
        };

        self.expect(TokenKind::RBrace);
        let span = self.span_from(&start);
        Expr {
            kind: ExprKind::Closure(ClosureExpr {
                is_async,
                is_move,
                params,
                body,
                span: span.clone(),
            }),
            span,
        }
    }

    /// Parse `do NL statements NL end` as a block expression.
    /// The value of the block is the value of its last expression,
    /// following the same tail-expression rule used by `resolve_block_as_expr`.
    fn parse_do_block_expr(&mut self) -> Expr {
        let start = self.current_span();
        self.advance(); // consume `do`
        self.skip_newlines();
        let body = self.parse_body();
        self.expect(TokenKind::End);
        let span = self.span_from(&start);
        Expr {
            kind: ExprKind::Block(body),
            span,
        }
    }

    pub(super) fn parse_do_closure(&mut self, is_async: bool, is_move: bool) -> Expr {
        let start = self.current_span();
        self.advance(); // consume do
        self.skip_newlines();

        let params = if self.at(TokenKind::Pipe) {
            self.parse_closure_params()
        } else if self.at(TokenKind::PipePipe) {
            // `||` = empty closure params.
            self.advance();
            vec![]
        } else {
            vec![]
        };
        self.skip_newlines();

        let block = self.parse_body();
        self.expect(TokenKind::End);
        let span = self.span_from(&start);
        Expr {
            kind: ExprKind::Closure(ClosureExpr {
                is_async,
                is_move,
                params,
                body: ClosureBody::Block(block),
                span: span.clone(),
            }),
            span,
        }
    }

    fn parse_closure_params(&mut self) -> Vec<ClosureParam> {
        self.expect(TokenKind::Pipe);
        let mut params = Vec::new();
        if !self.at(TokenKind::Pipe) {
            params.push(self.parse_closure_param());
            while self.eat(TokenKind::Comma) {
                self.skip_newlines();
                if self.at(TokenKind::Pipe) {
                    break;
                }
                params.push(self.parse_closure_param());
            }
        }
        self.expect(TokenKind::Pipe);
        params
    }

    fn parse_closure_param(&mut self) -> ClosureParam {
        let start = self.current_span();
        let name = self.expect_any_identifier();
        let type_expr = if self.eat(TokenKind::Colon) {
            Some(self.parse_type())
        } else {
            None
        };
        let span = self.span_from(&start);
        ClosureParam {
            name,
            type_expr,
            span,
        }
    }
}
