//! Type expression parsing for the Riven language.

use crate::lexer::token::TokenKind;
use crate::parser::ast::*;
use crate::parser::Parser;

impl Parser {
    /// Parse a type expression.
    pub(crate) fn parse_type(&mut self) -> TypeExpr {
        self.skip_newlines();
        let start = self.current_span();

        match self.current_kind() {
            // Raw pointer type: *T, *mut T
            TokenKind::Star => self.parse_raw_pointer_type(),

            // Reference type: &[lifetime] [mut] Type
            TokenKind::Amp => self.parse_reference_type(false),
            TokenKind::AmpMut => self.parse_reference_type(true),

            // Double reference: && (lexed as AmpAmp)
            TokenKind::AmpAmp => {
                let start = self.current_span();
                self.advance(); // consume &&
                let inner = self.parse_type();
                let inner_span = start.clone();
                let outer_span = self.span_from(&start);
                TypeExpr::Reference {
                    lifetime: None,
                    mutable: false,
                    inner: Box::new(TypeExpr::Reference {
                        lifetime: None,
                        mutable: false,
                        inner: Box::new(inner),
                        span: inner_span,
                    }),
                    span: outer_span,
                }
            }

            // Tuple type or unit: (Type, Type, ...)
            TokenKind::LParen => self.parse_tuple_or_unit_type(),

            // Array type: [Type; size]
            TokenKind::LBracket => self.parse_array_type(),

            // impl Trait
            TokenKind::Impl => self.parse_impl_trait_type(),

            // dyn Trait
            TokenKind::Dyn => self.parse_dyn_trait_type(),

            // Fn type: Fn(T1, T2) -> R
            TokenKind::TypeIdentifier(ref name) if name == "Fn" => self.parse_fn_type(),

            // Never type
            TokenKind::TypeIdentifier(ref name) if name == "Never" => {
                let span = self.current_span();
                self.advance();
                TypeExpr::Never { span }
            }

            // Named type: Path[GenericArgs]
            TokenKind::TypeIdentifier(_) | TokenKind::SelfType => self.parse_named_type(),

            // Lifetime in a type position (e.g., in trait bounds)
            TokenKind::Lifetime(_) => {
                // This shouldn't happen in normal type position, but handle gracefully
                self.error("unexpected lifetime in type position");
                TypeExpr::Inferred { span: start }
            }

            // Lowercase identifiers that can be used as types (e.g., `str`)
            TokenKind::Identifier(ref name) if is_primitive_type_name(name) => {
                self.parse_named_type_from_identifier()
            }

            _ => {
                self.error(&format!("expected type, found {:?}", self.current_kind()));
                TypeExpr::Inferred { span: start }
            }
        }
    }

    /// Parse a raw pointer type: `*T`, `*mut T`, `*Void`, `*mut Void`
    fn parse_raw_pointer_type(&mut self) -> TypeExpr {
        let start = self.current_span();
        self.advance(); // consume *

        let mutable = self.eat(TokenKind::Mut);
        let inner = self.parse_type();
        let span = self.span_from(&start);

        TypeExpr::RawPointer {
            mutable,
            inner: Box::new(inner),
            span,
        }
    }

    fn parse_reference_type(&mut self, is_amp_mut: bool) -> TypeExpr {
        let start = self.current_span();
        self.advance(); // consume & or &mut

        let mut lifetime = None;
        let mutable;

        if is_amp_mut {
            // &mut was a single token
            // Check for lifetime after &mut
            if let TokenKind::Lifetime(ref lt) = self.current_kind().clone() {
                lifetime = Some(lt.clone());
                self.advance();
            }
            mutable = true;
        } else {
            // & — check for lifetime
            if let TokenKind::Lifetime(ref lt) = self.current_kind().clone() {
                lifetime = Some(lt.clone());
                self.advance();
            }
            // Check for mut after & [lifetime]
            if self.at(TokenKind::Mut) {
                self.advance();
                mutable = true;
            } else {
                mutable = false;
            }
        }

        let inner = self.parse_type();
        let span = self.span_from(&start);

        TypeExpr::Reference {
            lifetime,
            mutable,
            inner: Box::new(inner),
            span,
        }
    }

    fn parse_tuple_or_unit_type(&mut self) -> TypeExpr {
        let start = self.current_span();
        self.advance(); // consume (
        self.skip_newlines();

        if self.at(TokenKind::RParen) {
            let span = self.span_from(&start);
            self.advance();
            return TypeExpr::Tuple {
                elements: vec![],
                span,
            };
        }

        let first = self.parse_type();
        self.skip_newlines();

        if self.at(TokenKind::Comma) {
            // Tuple type
            let mut elements = vec![first];
            while self.eat(TokenKind::Comma) {
                self.skip_newlines();
                if self.at(TokenKind::RParen) {
                    break;
                }
                elements.push(self.parse_type());
                self.skip_newlines();
            }
            self.expect(TokenKind::RParen);
            let span = self.span_from(&start);
            TypeExpr::Tuple { elements, span }
        } else {
            // Parenthesized single type — treat as that type
            self.expect(TokenKind::RParen);
            first
        }
    }

    fn parse_array_type(&mut self) -> TypeExpr {
        let start = self.current_span();
        self.advance(); // consume [
        self.skip_newlines();

        let element = self.parse_type();
        self.skip_newlines();

        let size = if self.eat(TokenKind::Semicolon) {
            self.skip_newlines();
            Some(Box::new(self.parse_expression()))
        } else {
            None
        };
        self.skip_newlines();
        self.expect(TokenKind::RBracket);
        let span = self.span_from(&start);

        TypeExpr::Array {
            element: Box::new(element),
            size,
            span,
        }
    }

    fn parse_impl_trait_type(&mut self) -> TypeExpr {
        let start = self.current_span();
        self.advance(); // consume impl
        let bounds = self.parse_trait_bounds();
        let span = self.span_from(&start);

        // Sugar: `impl Fn(Args) -> R` is a single-bound `impl Fn` over a
        // function signature. Collapse it straight into `TypeExpr::Function`
        // so downstream (type-check + codegen) handles it as an ordinary
        // closure type, exactly like a bare `Fn(Args) -> R` return type.
        if bounds.len() == 1 {
            let b = &bounds[0];
            let is_fn_trait = b
                .path
                .segments
                .last()
                .map(|s| matches!(s.as_str(), "Fn" | "FnMut" | "FnOnce"))
                .unwrap_or(false);
            if is_fn_trait {
                if let Some(args) = &b.path.generic_args {
                    if args.len() == 1 {
                        if let TypeExpr::Function { .. } = &args[0] {
                            return args[0].clone();
                        }
                    }
                }
            }
        }

        TypeExpr::ImplTrait { bounds, span }
    }

    fn parse_dyn_trait_type(&mut self) -> TypeExpr {
        let start = self.current_span();
        self.advance(); // consume dyn
        let bounds = self.parse_trait_bounds();
        let span = self.span_from(&start);
        TypeExpr::DynTrait { bounds, span }
    }

    fn parse_fn_type(&mut self) -> TypeExpr {
        let start = self.current_span();
        self.advance(); // consume Fn
        self.expect(TokenKind::LParen);
        self.skip_newlines();

        let mut params = Vec::new();
        if !self.at(TokenKind::RParen) {
            params.push(self.parse_type());
            while self.eat(TokenKind::Comma) {
                self.skip_newlines();
                if self.at(TokenKind::RParen) {
                    break;
                }
                params.push(self.parse_type());
            }
        }
        self.skip_newlines();
        self.expect(TokenKind::RParen);

        let return_type = if self.eat(TokenKind::Arrow) {
            self.skip_newlines();
            self.parse_type()
        } else {
            TypeExpr::Tuple {
                elements: vec![],
                span: self.current_span(),
            }
        };

        let span = self.span_from(&start);
        TypeExpr::Function {
            params,
            return_type: Box::new(return_type),
            span,
        }
    }

    pub(crate) fn parse_named_type(&mut self) -> TypeExpr {
        let path = self.parse_type_path();
        TypeExpr::Named(path)
    }

    pub(crate) fn parse_type_path(&mut self) -> TypePath {
        let start = self.current_span();
        let mut segments = Vec::new();

        // First segment
        match self.current_kind().clone() {
            TokenKind::TypeIdentifier(name) => {
                segments.push(name);
                self.advance();
            }
            TokenKind::SelfType => {
                segments.push("Self".to_string());
                self.advance();
            }
            _ => {
                self.error("expected type name");
                return TypePath {
                    segments: vec!["_Error".to_string()],
                    generic_args: None,
                    span: start,
                };
            }
        }

        // Additional segments via .
        while self.at(TokenKind::Dot) {
            if let TokenKind::TypeIdentifier(_) = self.peek_kind() {
                self.advance(); // consume .
                if let TokenKind::TypeIdentifier(name) = self.current_kind().clone() {
                    segments.push(name);
                    self.advance();
                }
            } else {
                break;
            }
        }

        // Generic args: [T, U]
        let generic_args = if self.at(TokenKind::LBracket) {
            Some(self.parse_generic_args())
        } else {
            None
        };

        let span = self.span_from(&start);
        TypePath {
            segments,
            generic_args,
            span,
        }
    }

    /// Parse generic arguments: [Type, Type, ...]
    ///
    /// Tier-2 const generics (stage 2): if the next token is an
    /// integer literal we emit `TypeExpr::ConstLit` instead of
    /// calling `parse_type()`.  Resolve disambiguates whether that
    /// landed against a type param (→ E0700) or a const param
    /// (→ `ConstExpr::Lit`) in S3.  S8 will extend the lookahead to
    /// accept arithmetic in generic-arg position.
    pub(crate) fn parse_generic_args(&mut self) -> Vec<TypeExpr> {
        self.expect(TokenKind::LBracket);
        self.skip_newlines();
        let mut args = Vec::new();

        if !self.at(TokenKind::RBracket) {
            args.push(self.parse_generic_arg());
            while self.eat(TokenKind::Comma) {
                self.skip_newlines();
                if self.at(TokenKind::RBracket) {
                    break;
                }
                args.push(self.parse_generic_arg());
            }
        }
        self.skip_newlines();
        self.expect(TokenKind::RBracket);
        args
    }

    /// Parse a single generic argument — a type expression, an
    /// integer literal that lowers to `TypeExpr::ConstLit`, or an
    /// arithmetic const expression that lowers to
    /// `TypeExpr::ConstExprArg`.
    ///
    /// Tier-2 const generics S8.S3: when an `IntLiteral` is followed
    /// by a binary arithmetic op (`+ - * /`), the whole expression
    /// is parsed via `parse_expression` and emitted as
    /// `ConstExprArg`.  Bare literals (no arithmetic follow-up)
    /// continue to emit `ConstLit` so existing call sites stay
    /// byte-identical.
    fn parse_generic_arg(&mut self) -> TypeExpr {
        if let TokenKind::IntLiteral(v, _suffix) = self.current_kind().clone() {
            // Peek one token past the literal.  If it's an arithmetic
            // op, fall through to expression parsing so `2 + 3` is
            // captured as a single `ConstExprArg`.  Otherwise the
            // historic `ConstLit` fast path applies.
            if matches!(
                self.peek_at_kind(1),
                TokenKind::Plus | TokenKind::Minus | TokenKind::Star | TokenKind::Slash
            ) {
                let start = self.current_span();
                let expr = self.parse_expression();
                let span = self.span_from(&start);
                return TypeExpr::ConstExprArg {
                    expr: Box::new(expr),
                    span,
                };
            }
            let span = self.current_span();
            self.advance();
            return TypeExpr::ConstLit { value: v, span };
        }
        self.parse_type()
    }

    /// Parse generic parameters: [T, U: Trait, 'a]
    pub(crate) fn parse_generic_params(&mut self) -> GenericParams {
        let start = self.current_span();
        self.expect(TokenKind::LBracket);
        self.skip_newlines();

        let mut params = Vec::new();
        if !self.at(TokenKind::RBracket) {
            params.push(self.parse_generic_param());
            while self.eat(TokenKind::Comma) {
                self.skip_newlines();
                if self.at(TokenKind::RBracket) {
                    break;
                }
                params.push(self.parse_generic_param());
            }
        }
        self.skip_newlines();
        self.expect(TokenKind::RBracket);
        let span = self.span_from(&start);
        GenericParams { params, span }
    }

    fn parse_generic_param(&mut self) -> GenericParam {
        self.skip_newlines();
        let start = self.current_span();

        // Tier-2 const generics: `const NAME: Type`.  Stage 1 — parser
        // only.  Resolve validates the type in S3.
        if self.eat(TokenKind::Const) {
            // `const` consumed.  Expect identifier or type-identifier
            // (Riven convention: `N`, `CAP`, `LEN` — all live in the
            // `Identifier` lexer slot, not `TypeIdentifier`, because
            // they're value-level names).
            let name = match self.current_kind().clone() {
                TokenKind::Identifier(n) | TokenKind::TypeIdentifier(n) => {
                    self.advance();
                    n
                }
                _ => {
                    self.error("expected const generic parameter name after `const`");
                    "_".to_string()
                }
            };
            self.expect(TokenKind::Colon);
            let ty = self.parse_type();
            let span = self.span_from(&start);
            return GenericParam::Const { name, ty, span };
        }

        if let TokenKind::Lifetime(ref name) = self.current_kind().clone() {
            let name = name.clone();
            self.advance();
            let span = self.span_from(&start);
            GenericParam::Lifetime { name, span }
        } else if let TokenKind::TypeIdentifier(ref name) = self.current_kind().clone() {
            let name = name.clone();
            self.advance();
            let bounds = if self.eat(TokenKind::Colon) {
                self.parse_trait_bounds()
            } else {
                vec![]
            };
            let span = self.span_from(&start);
            GenericParam::Type { name, bounds, span }
        } else {
            self.error("expected generic parameter");
            GenericParam::Type {
                name: "_".to_string(),
                bounds: vec![],
                span: start,
            }
        }
    }

    /// Parse trait bounds: Trait1 + Trait2 + ...
    pub(crate) fn parse_trait_bounds(&mut self) -> Vec<TraitBound> {
        let mut bounds = Vec::new();
        bounds.push(self.parse_single_trait_bound());
        while self.eat(TokenKind::Plus) {
            self.skip_newlines();
            bounds.push(self.parse_single_trait_bound());
        }
        bounds
    }

    fn parse_single_trait_bound(&mut self) -> TraitBound {
        self.skip_newlines();
        let mut path = self.parse_type_path();

        // Fn-trait sugar inside a bound: `Fn(A, B) -> R` / `FnMut(...)` /
        // `FnOnce(...)`. The type-path parser stops at `(`, so we pick up
        // the parenthesized arg list and optional return type here and
        // stash them as a synthetic `Function` generic arg so downstream
        // code can recover the signature.
        let is_fn_trait = path
            .segments
            .last()
            .map(|s| matches!(s.as_str(), "Fn" | "FnMut" | "FnOnce"))
            .unwrap_or(false);
        if is_fn_trait && self.at(TokenKind::LParen) {
            let fn_start = self.current_span();
            self.advance(); // consume (
            self.skip_newlines();
            let mut params = Vec::new();
            if !self.at(TokenKind::RParen) {
                params.push(self.parse_type());
                while self.eat(TokenKind::Comma) {
                    self.skip_newlines();
                    if self.at(TokenKind::RParen) {
                        break;
                    }
                    params.push(self.parse_type());
                }
            }
            self.skip_newlines();
            self.expect(TokenKind::RParen);

            let return_type = if self.eat(TokenKind::Arrow) {
                self.skip_newlines();
                self.parse_type()
            } else {
                TypeExpr::Tuple {
                    elements: vec![],
                    span: self.current_span(),
                }
            };

            let fn_span = self.span_from(&fn_start);
            let fn_ty = TypeExpr::Function {
                params,
                return_type: Box::new(return_type),
                span: fn_span,
            };
            path.generic_args = Some(vec![fn_ty]);
            path.span = self.span_from(&path.span.clone());
        }

        let span = path.span.clone();
        TraitBound { path, span }
    }

    /// Parse where clause: `where T: Trait, U: Trait`, and (T2.02 S9
    /// parser cut) const predicates `where N > 0, N == M, N + M == 8`.
    pub(crate) fn parse_where_clause(&mut self) -> WhereClause {
        let start = self.current_span();
        self.expect(TokenKind::Where);
        self.skip_newlines();

        let mut predicates = Vec::new();
        let mut const_predicates = Vec::new();
        self.parse_where_item(&mut predicates, &mut const_predicates);
        while self.eat(TokenKind::Comma) {
            self.skip_newlines();
            // Stop if we hit something that's not a where item
            if self.at(TokenKind::Newline)
                || self.at(TokenKind::Eof)
                || self.at(TokenKind::LBrace)
                || self.at(TokenKind::End)
            {
                break;
            }
            self.parse_where_item(&mut predicates, &mut const_predicates);
        }

        let span = self.span_from(&start);
        WhereClause {
            predicates,
            const_predicates,
            span,
        }
    }

    /// Decide whether the next where-clause item is a trait bound
    /// (`T: TraitName`) or a const predicate (`N > 0`, `N + M == 8`).
    ///
    /// Heuristic: peek one token past the leading identifier.  If it
    /// is a comparison op (`> < >= <= == !=`) or an arithmetic op
    /// (`+ - * /`), parse as a const predicate.  Otherwise fall back
    /// to the trait-bound path (which expects `:`).  This correctly
    /// disambiguates every spec §B9 shape without needing to
    /// speculatively parse a full expression and backtrack.
    fn parse_where_item(
        &mut self,
        predicates: &mut Vec<WherePredicate>,
        const_predicates: &mut Vec<ConstPredicate>,
    ) {
        // Lookahead: the second token must be a comparison or
        // arithmetic op for this to be a const predicate.  Anything
        // else (notably `:`) falls through to the trait-bound path.
        let is_const_pred = matches!(
            self.peek_at_kind(1),
            TokenKind::Lt
                | TokenKind::Gt
                | TokenKind::LtEq
                | TokenKind::GtEq
                | TokenKind::EqEq
                | TokenKind::NotEq
                | TokenKind::Plus
                | TokenKind::Minus
                | TokenKind::Star
                | TokenKind::Slash
        );
        if is_const_pred {
            let start = self.current_span();
            let expr = self.parse_expression();
            let span = self.span_from(&start);
            const_predicates.push(ConstPredicate {
                expr: Box::new(expr),
                span,
            });
        } else {
            predicates.push(self.parse_where_predicate());
        }
    }

    fn parse_where_predicate(&mut self) -> WherePredicate {
        let start = self.current_span();
        let type_expr = self.parse_type();
        self.expect(TokenKind::Colon);
        let bounds = self.parse_trait_bounds();
        let span = self.span_from(&start);
        WherePredicate {
            type_expr,
            bounds,
            span,
        }
    }

    /// Parse a named type from a lowercase identifier (e.g., `str`).
    fn parse_named_type_from_identifier(&mut self) -> TypeExpr {
        let start = self.current_span();
        let name = match self.current_kind().clone() {
            TokenKind::Identifier(name) => {
                self.advance();
                name
            }
            _ => unreachable!(),
        };

        // Generic args: [T, U]
        let generic_args = if self.at(TokenKind::LBracket) {
            Some(self.parse_generic_args())
        } else {
            None
        };

        let span = self.span_from(&start);
        TypeExpr::Named(TypePath {
            segments: vec![name],
            generic_args,
            span,
        })
    }
}

/// Check if a lowercase identifier is a known primitive type name.
fn is_primitive_type_name(name: &str) -> bool {
    matches!(
        name,
        "str" | "bool" | "int" | "float" | "char" | "uint" | "usize"
    )
}
