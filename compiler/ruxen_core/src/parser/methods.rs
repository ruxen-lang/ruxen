//! Function/method definitions, parameter lists, statements, and let-bindings.
//!
//! This module groups the syntactic surface that revolves around `def` bodies:
//! parameter parsing, method signatures (for mixin items), block-body / single
//! expression-body parsing, statements, and `let` / `var` bindings.

use crate::lexer::token::TokenKind;
use crate::parser::ast::*;
use crate::parser::Parser;

/// Internal helper for parsing method signatures before deciding if body follows.
pub(super) struct ParsedMethodSig {
    pub(super) vis: Visibility,
    pub(super) is_async: bool,
    pub(super) self_mode: Option<SelfMode>,
    pub(super) is_class_method: bool,
    pub(super) name: String,
    pub(super) generic_params: Option<GenericParams>,
    pub(super) params: Vec<Param>,
    pub(super) return_type: Option<TypeExpr>,
}

impl Parser {
    /// Parse the Ruby-style name list that follows a section marker:
    /// `private :method_a, :method_b`. The leading marker has already
    /// been consumed; the cursor sits on the first `:`.
    pub(super) fn parse_visibility_name_list(&mut self) -> Vec<String> {
        let mut names = Vec::new();
        // First name: `: ident`
        if !self.eat(TokenKind::Colon) {
            return names;
        }
        match self.current_kind().clone() {
            TokenKind::Identifier(n) => {
                self.advance();
                names.push(n);
            }
            _ => {
                self.error(&format!(
                    "expected method name after `:` in visibility name list, found {:?}",
                    self.current_kind()
                ));
                return names;
            }
        }
        while self.eat(TokenKind::Comma) {
            self.skip_newlines();
            if !self.eat(TokenKind::Colon) {
                self.error("expected `:method_name` after `,` in visibility name list");
                break;
            }
            match self.current_kind().clone() {
                TokenKind::Identifier(n) => {
                    self.advance();
                    names.push(n);
                }
                _ => {
                    self.error(&format!(
                        "expected method name after `:` in visibility name list, found {:?}",
                        self.current_kind()
                    ));
                    break;
                }
            }
        }
        names
    }

    /// Parse an `include Mixin` directive inside a class body. Produces an
    /// InnerImpl with no items — the contract is satisfied by methods
    /// scattered at the class-body level (resolver/typecker enforce).
    pub(super) fn parse_include_directive(&mut self, is_unsafe: bool) -> InnerImpl {
        let start = self.current_span();
        self.advance(); // consume `include`
        let negative_trait = self.eat(TokenKind::Bang);
        let trait_name = self.parse_type_path();
        let span = self.span_from(&start);
        InnerImpl {
            is_unsafe,
            negative_trait,
            trait_name,
            items: Vec::new(),
            span,
        }
    }

    // ─── Field Declarations ──────────────────────────────────────────

    pub(super) fn parse_field_decl_with_vis(&mut self, visibility: Visibility) -> FieldDecl {
        let start = self.current_span();
        let name = self.expect_identifier();
        self.expect(TokenKind::Colon);
        let type_expr = self.parse_type();
        let span = self.span_from(&start);
        FieldDecl {
            visibility,
            name,
            type_expr,
            span,
        }
    }

    /// Parse a method signature (everything except the body).
    /// Returns the parsed signature components.
    pub(super) fn parse_method_signature(&mut self, visibility: Visibility) -> ParsedMethodSig {
        let is_async = self.eat(TokenKind::Async);
        self.expect(TokenKind::Def);

        // Self mode: var (writing) or consume
        let self_mode = if self.at(TokenKind::Var) {
            let peek = self.peek_kind();
            // `def var name` / `def var init` / `def var self.m` is the
            // mutable-self form; `def var []=(…)` is a mutable-self
            // operator method (index-assign), so an operator name after
            // `var` also signals self-mode.
            if matches!(
                peek,
                TokenKind::Identifier(_) | TokenKind::Init | TokenKind::SelfValue
            ) || Self::is_operator_name_start(&peek)
            {
                self.advance();
                Some(SelfMode::Mutable)
            } else {
                None
            }
        } else if self.eat(TokenKind::Consume) {
            Some(SelfMode::Consuming)
        } else {
            None
        };

        // Function name
        let (is_class_method, name) = if self.at(TokenKind::SelfValue) {
            self.advance();
            self.expect(TokenKind::Dot);
            let method_name = self.expect_identifier();
            (true, method_name)
        } else if self.at(TokenKind::Init) {
            self.advance();
            (false, "init".to_string())
        } else {
            // Operator-symbol method names (`def +`, `def []`, `def -@`)
            // resolve here alongside plain identifiers. See
            // `Parser::parse_def_name`.
            let name = self.parse_def_name();
            (false, name)
        };

        let generic_params = if self.at(TokenKind::LBracket) {
            Some(self.parse_generic_params())
        } else {
            None
        };

        let params = if self.at(TokenKind::LParen) {
            self.parse_params()
        } else {
            vec![]
        };

        let return_type = if self.eat(TokenKind::Arrow) {
            self.skip_newlines();
            Some(self.parse_type())
        } else {
            None
        };

        ParsedMethodSig {
            vis: visibility,
            is_async,
            self_mode,
            is_class_method,
            name,
            generic_params,
            params,
            return_type,
        }
    }

    // ─── Function Definition ─────────────────────────────────────────

    pub(super) fn parse_func_def(&mut self, visibility: Visibility) -> FuncDef {
        let doc_comments = self.take_pending_docs();
        let start = self.current_span();
        let is_async = self.eat(TokenKind::Async);
        self.expect(TokenKind::Def);

        // Self mode: var (writing) or consume
        let self_mode = if self.at(TokenKind::Var) {
            // Check if this is `def var name` (self mode) vs something else
            // It's a self mode if followed by an identifier or self.ident
            let peek = self.peek_kind();
            // `def var name` / `def var init` / `def var self.m` is the
            // mutable-self form; `def var []=(…)` is a mutable-self
            // operator method (index-assign), so an operator name after
            // `var` also signals self-mode.
            if matches!(
                peek,
                TokenKind::Identifier(_) | TokenKind::Init | TokenKind::SelfValue
            ) || Self::is_operator_name_start(&peek)
            {
                self.advance();
                Some(SelfMode::Mutable)
            } else {
                None
            }
        } else if self.eat(TokenKind::Consume) {
            Some(SelfMode::Consuming)
        } else {
            None
        };

        // Function name — could be:
        // - regular identifier
        // - init
        // - self.method_name (class method)
        let (is_class_method, name) = if self.at(TokenKind::SelfValue) {
            // self.method_name — class method
            self.advance(); // consume self
            self.expect(TokenKind::Dot);
            let method_name = self.expect_identifier();
            (true, method_name)
        } else if self.at(TokenKind::Init) {
            self.advance();
            (false, "init".to_string())
        } else {
            // Operator-symbol method names (`def +`, `def []`, `def -@`)
            // resolve here alongside plain identifiers. See
            // `Parser::parse_def_name`.
            let name = self.parse_def_name();
            (false, name)
        };

        // Generic params
        let generic_params = if self.at(TokenKind::LBracket) {
            Some(self.parse_generic_params())
        } else {
            None
        };

        // Parameters
        let params = if self.at(TokenKind::LParen) {
            self.parse_params()
        } else {
            vec![]
        };

        // Return type
        let return_type = if self.eat(TokenKind::Arrow) {
            self.skip_newlines();
            Some(self.parse_type())
        } else {
            None
        };

        // Where clause — may appear on a new line after `-> Ret`
        let where_clause = {
            // Peek past newlines to see if `where` follows
            let mut look = 0;
            while matches!(self.peek_at_kind(look), TokenKind::Newline) {
                look += 1;
            }
            if self.at(TokenKind::Where) || matches!(self.peek_at_kind(look), TokenKind::Where) {
                self.skip_newlines();
                Some(self.parse_where_clause())
            } else {
                None
            }
        };

        self.skip_newlines();

        // Body: either { expr } for single-expression methods or multi-line body ... end
        let body = if self.at(TokenKind::LBrace) {
            // Single expression body: { expr }
            self.advance(); // consume {
            self.skip_newlines();
            let expr = self.parse_expression();
            self.skip_newlines();
            self.expect(TokenKind::RBrace);
            let span = self.span_from(&start);
            Block {
                statements: vec![Statement::Expression(expr)],
                span,
            }
        } else {
            // Multi-line body ... end
            let body = self.parse_body();
            self.expect(TokenKind::End);
            body
        };

        let span = self.span_from(&start);

        // Determine self_mode: if no explicit mode but method has body referencing self,
        // default to Immutable for methods (non-class methods).
        let final_self_mode = self_mode.or({
            // If it's not a class method and not init and doesn't have explicit self_mode,
            // we don't add one — it means no self param.
            None
        });

        FuncDef {
            visibility,
            is_async,
            self_mode: final_self_mode,
            is_class_method,
            name,
            generic_params,
            params,
            return_type,
            where_clause,
            body,
            doc_comments,
            span,
        }
    }

    pub(super) fn parse_params(&mut self) -> Vec<Param> {
        self.expect(TokenKind::LParen);
        self.skip_newlines();
        let mut params = Vec::new();
        if !self.at(TokenKind::RParen) {
            params.push(self.parse_param());
            while self.eat(TokenKind::Comma) {
                self.skip_newlines();
                if self.at(TokenKind::RParen) {
                    break;
                }
                params.push(self.parse_param());
            }
        }
        self.skip_newlines();
        self.expect(TokenKind::RParen);
        params
    }

    fn parse_param(&mut self) -> Param {
        let start = self.current_span();
        self.skip_newlines();

        // Check for auto-assign: @name
        let auto_assign = self.eat(TokenKind::At);

        // Check for &block parameter
        if self.at(TokenKind::Amp) {
            if let TokenKind::Identifier(ref name) = self.peek_kind() {
                if name == "block" {
                    self.advance(); // consume &
                    let name = self.expect_identifier(); // consume "block"
                    self.expect(TokenKind::Colon);
                    let type_expr = self.parse_block_type();
                    let span = self.span_from(&start);
                    return Param {
                        auto_assign: false,
                        name: format!("&{}", name),
                        type_expr,
                        default: None,
                        span,
                    };
                }
            }
        }

        let name = self.expect_identifier();
        self.expect(TokenKind::Colon);
        let type_expr = self.parse_type();
        let default = if self.eat(TokenKind::Eq) {
            Some(Box::new(self.parse_expression()))
        } else {
            None
        };
        let span = self.span_from(&start);
        Param {
            auto_assign,
            name,
            type_expr,
            default,
            span,
        }
    }

    /// Parse a Block type: Block(T1, T2) -> R or Block -> R or Block
    fn parse_block_type(&mut self) -> TypeExpr {
        let start = self.current_span();
        // Expect "Block" type identifier
        if let TokenKind::TypeIdentifier(ref name) = self.current_kind().clone() {
            if name == "Block" {
                self.advance();
                let params = if self.at(TokenKind::LParen) {
                    self.advance();
                    self.skip_newlines();
                    let mut params = Vec::new();
                    if !self.at(TokenKind::RParen) {
                        params.push(self.parse_type());
                        while self.eat(TokenKind::Comma) {
                            self.skip_newlines();
                            params.push(self.parse_type());
                        }
                    }
                    self.expect(TokenKind::RParen);
                    params
                } else {
                    vec![]
                };

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
                return TypeExpr::Function {
                    params,
                    return_type: Box::new(return_type),
                    span,
                };
            }
        }

        // Fallback to regular type parsing
        self.parse_type()
    }

    // ─── Statement & Block Parsing ──────────────────────────────────

    /// Parse a body (sequence of statements) up to `end`, `else`, `elsif`, `end`.
    pub(crate) fn parse_body(&mut self) -> Block {
        self.parse_body_with_options(false)
    }

    /// Variant of `parse_body` for match-arm bodies. Same as
    /// `parse_body` except it also stops when the upcoming tokens
    /// look like a sibling arm header (`<pattern> -> ...`).
    /// Without this signal, a multi-statement arm body greedily
    /// consumes the next sibling's pattern as a statement, blowing
    /// up later with "expected expression, found Arrow".
    pub(crate) fn parse_match_arm_body(&mut self) -> Block {
        self.parse_body_with_options(true)
    }

    fn parse_body_with_options(&mut self, in_match_arm: bool) -> Block {
        let start = self.current_span();
        let mut statements = Vec::new();

        loop {
            self.skip_terminators();
            match self.current_kind() {
                TokenKind::End | TokenKind::Else | TokenKind::Elsif | TokenKind::Eof => break,
                _ => {
                    if in_match_arm && self.looks_like_sibling_match_arm() {
                        break;
                    }
                    let before = self.pos;
                    statements.push(self.parse_statement());
                    self.expect_terminator();
                    // Safety: if we made no progress, force advance to avoid infinite loop
                    if self.pos == before {
                        self.advance();
                    }
                }
            }
        }

        let span = self.span_from(&start);
        Block { statements, span }
    }

    /// Heuristic lookahead: are we at the start of a sibling match
    /// arm header?
    ///
    /// An arm header is a short pattern token sequence followed by
    /// `->`. So we scan forward at the current bracket depth and
    /// return true only if we hit `->` BEFORE any token that
    /// disqualifies the prefix as a pattern (statement-starting
    /// keyword, operator that can't appear in a pattern, newline at
    /// depth 0 — patterns don't span newlines outside of brackets).
    ///
    /// Used by `parse_match_arm_body` to terminate a multi-statement
    /// arm body before it eats the next arm's pattern.
    fn looks_like_sibling_match_arm(&self) -> bool {
        let mut i = self.pos;
        let end = (i + 16).min(self.tokens.len());
        let mut depth: i32 = 0;
        while i < end {
            match &self.tokens[i].kind {
                TokenKind::Arrow if depth == 0 => return true,
                TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => depth += 1,
                TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                    if depth == 0 {
                        return false;
                    }
                    depth -= 1;
                }
                // Hard stops at depth 0: a real statement starts
                // here, not a pattern. The keyword check rules out
                // `let z = ...` being mis-detected as a pattern just
                // because there's an Arrow at the next sibling arm.
                TokenKind::End
                | TokenKind::Eof
                | TokenKind::Let
                | TokenKind::Var
                | TokenKind::While
                | TokenKind::For
                | TokenKind::Match
                | TokenKind::Return
                | TokenKind::Break
                | TokenKind::Continue
                | TokenKind::Newline
                    if depth == 0 =>
                {
                    return false;
                }
                TokenKind::If if depth == 0 => {
                    // `if` at the current position starts a statement
                    // body, but `pat if guard ->` is a sibling match arm.
                    if i == self.pos {
                        return false;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        false
    }

    pub(crate) fn parse_statement(&mut self) -> Statement {
        self.skip_newlines();

        match self.current_kind() {
            TokenKind::Let | TokenKind::Var => Statement::Let(self.parse_let_binding()),
            _ => Statement::Expression(self.parse_expression()),
        }
    }

    fn parse_let_binding(&mut self) -> LetBinding {
        let start = self.current_span();
        // `var x = ...` is the mutable form; `let x = ...` is immutable.
        // (`let mut x = ...` is retired — `mut` is no longer a keyword.)
        let var_form = matches!(self.current_kind(), TokenKind::Var);
        self.advance(); // consume let | var

        let mutable = var_form;
        let pattern = self.parse_pattern();

        let type_annotation = if self.eat(TokenKind::Colon) {
            Some(self.parse_type())
        } else {
            None
        };

        let value = if self.eat(TokenKind::Eq) {
            self.skip_newlines();
            Some(Box::new(self.parse_expression()))
        } else {
            None
        };

        let span = self.span_from(&start);
        LetBinding {
            mutable,
            pattern,
            type_annotation,
            value,
            span,
        }
    }
}
