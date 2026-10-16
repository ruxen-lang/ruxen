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

/// Section-marker state of a body (class, struct, enum, mixin, module): the
/// running `public` / `private` / `protected` visibility
/// (ruby-naming.spec.md §3.2) and the `private :a, :b` name lists, applied
/// once the body is parsed.
pub(super) struct BodyState {
    pub(super) current_vis: Visibility,
    name_lists: Vec<(Visibility, Vec<String>)>,
}

impl BodyState {
    pub(super) fn new() -> Self {
        Self {
            current_vis: Visibility::Public,
            name_lists: Vec::new(),
        }
    }

    /// Re-mark every method a name list names. A name list wins over the
    /// section the method sat in, and a later list over an earlier one.
    pub(super) fn apply_name_lists<'f>(&self, methods: impl Iterator<Item = &'f mut FuncDef>) {
        for method in methods {
            for (vis, names) in &self.name_lists {
                if names.contains(&method.name) {
                    method.visibility = *vis;
                }
            }
        }
    }
}

/// The members every type body (class, struct, enum) shares.
#[derive(Default)]
pub(super) struct TypeMembers {
    pub(super) methods: Vec<FuncDef>,
    pub(super) inner_impls: Vec<InnerImpl>,
    pub(super) aliases: Vec<AliasDef>,
}

impl Parser {
    /// Move to the next member of an `end`-terminated body: skip blank
    /// lines and hold any `##` docs for the member. False at the body's
    /// `end` (or end of input).
    pub(super) fn at_body_member(&mut self) -> bool {
        self.skip_newlines();
        let docs = self.collect_doc_comments();
        self.pending_doc_comments.extend(docs);
        !self.at(TokenKind::End) && !self.at(TokenKind::Eof)
    }

    /// Parse the section marker at the cursor (`public`, `private`, or a
    /// bare `protected`): a section switch, or a `private :a, :b` name list.
    pub(super) fn parse_section_marker(&mut self, vis: Visibility, state: &mut BodyState) {
        self.advance();
        if self.at(TokenKind::Colon) {
            let names = self.parse_visibility_name_list();
            state.name_lists.push((vis, names));
        } else {
            state.current_vis = vis;
        }
    }

    /// Parse a member every type body accepts: a section marker, a `def`
    /// (also `protected def` and `inline def`), an `include` or an `alias`.
    /// Returns false, consuming nothing, at any other member. With
    /// `has_fields`, `protected name: Type` is left to the caller as a field.
    pub(super) fn parse_type_body_member(
        &mut self,
        body: &str,
        has_fields: bool,
        state: &mut BodyState,
        members: &mut TypeMembers,
    ) -> bool {
        match self.current_kind().clone() {
            TokenKind::Public => self.parse_section_marker(Visibility::Public, state),
            TokenKind::Private => self.parse_section_marker(Visibility::Private, state),
            TokenKind::Protected
                if matches!(self.peek_kind(), TokenKind::Def | TokenKind::Async) =>
            {
                self.advance();
                members
                    .methods
                    .push(self.parse_func_def(Visibility::Protected));
            }
            TokenKind::Protected
                if has_fields && matches!(self.peek_kind(), TokenKind::Identifier(_)) =>
            {
                return false;
            }
            TokenKind::Protected => self.parse_section_marker(Visibility::Protected, state),
            TokenKind::Def | TokenKind::Async => {
                members.methods.push(self.parse_func_def(state.current_vis));
            }
            // `include Mixin` declares mixin participation; required methods
            // are the body's own, default methods are synthesized in resolve.
            TokenKind::Include => {
                members
                    .inner_impls
                    .push(self.parse_include_directive(false));
            }
            TokenKind::Unsafe if matches!(self.peek_kind(), TokenKind::Include) => {
                self.advance(); // consume `unsafe`
                members.inner_impls.push(self.parse_include_directive(true));
            }
            // `inline` is a hint only; codegen ignores it.
            TokenKind::Inline => {
                self.advance();
                if matches!(self.current_kind(), TokenKind::Def | TokenKind::Async) {
                    members.methods.push(self.parse_func_def(state.current_vis));
                } else {
                    self.error(&format!(
                        "expected `def` after `inline` in {body} body, found {:?}",
                        self.current_kind()
                    ));
                    if !self.at(TokenKind::Eof) {
                        self.advance();
                    }
                    self.synchronize();
                }
            }
            // `alias new old` (docs/decisions/alias-keyword.md). `alias`
            // lexes as an Identifier; a field named `alias` is `alias: Type`,
            // which `is_alias_item_start` rejects.
            TokenKind::Identifier(_) if self.is_alias_item_start() => {
                members.aliases.push(self.parse_alias_item());
            }
            _ => return false,
        }
        true
    }

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

    /// Parse a method signature (everything except the body): the header
    /// shared by `def`s and mixin signatures.
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
        let ParsedMethodSig {
            vis: visibility,
            is_async,
            self_mode,
            is_class_method,
            name,
            generic_params,
            params,
            return_type,
        } = self.parse_method_signature(visibility);

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

        // Body: an endless `= expr`, a `{ expr }` single-expression body, or a
        // multi-line `... end` body.
        let body = if self.at(TokenKind::Eq) {
            // Endless method definition: `def f(x) = expr` (no `end`).
            self.advance(); // consume =
            self.skip_newlines();
            let expr = self.parse_expression();
            let span = self.span_from(&start);
            Block {
                statements: vec![Statement::Expression(expr)],
                span,
            }
        } else if self.at(TokenKind::LBrace) {
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

        FuncDef {
            visibility,
            is_async,
            self_mode,
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

    /// Parse the type annotation of a `&block:` parameter.
    ///
    /// Per the Ruby-block-semantics ADR (`docs/decisions/ruby-block-semantics.md`)
    /// the CANONICAL spelling is the square-bracket generic form
    /// `Fn[(T1, T2) -> R]` / `Fn[() -> R]` / `Fn[-> R]`, matching Ruxen's
    /// `Array[Int]` / `State[T]` convention. The following back-compat
    /// spellings are also accepted (and `ruxen fmt` preserves whichever
    /// was written — no normalization):
    ///   - `Fn(T1, T2) -> R`              (the existing closure-param spelling)
    ///   - `Block(T1, T2) -> R` / `Block` (the legacy `&block: Block(...)` form)
    ///   - any other `parse_type` shape (`any Fn[...]`, etc.) as a fallback.
    ///
    /// All of these normalize to a single `TypeExpr::Function { params, return_type }`
    /// so resolve/typeck see one shape regardless of surface spelling.
    fn parse_block_type(&mut self) -> TypeExpr {
        let start = self.current_span();
        if let TokenKind::TypeIdentifier(ref name) = self.current_kind().clone() {
            // Canonical: `Fn[(T…) -> R]`. Only treat `Fn[` as the block
            // signature form — `Fn(...)` falls through to `parse_fn_type`.
            if name == "Fn" && self.peek_kind() == TokenKind::LBracket {
                self.advance(); // consume `Fn`
                self.advance(); // consume `[`
                self.skip_newlines();
                let (params, return_type) = self.parse_fn_signature_tail();
                self.skip_newlines();
                self.expect(TokenKind::RBracket);
                let span = self.span_from(&start);
                return TypeExpr::Function {
                    params,
                    return_type: Box::new(return_type),
                    bracketed: true, // canonical `Fn[(T…) -> R]`
                    span,
                };
            }
            // Legacy: `Block(T…) -> R` / `Block`.
            if name == "Block" {
                self.advance(); // consume `Block`
                let (params, return_type) = self.parse_fn_signature_tail();
                let span = self.span_from(&start);
                return TypeExpr::Function {
                    params,
                    return_type: Box::new(return_type),
                    bracketed: false, // legacy `Block(...)` form
                    span,
                };
            }
        }

        // Back-compat `Fn(...) -> R` and every other shape (`any Fn[...]`,
        // a bare named type, …) go through ordinary type parsing.
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
            _ => {
                let expr = self.parse_expression();
                // Ruby parallel assignment: `a, b = e1, e2` (and swap
                // `a, b = b, a`). `parse_expression` stopped at the first `,`
                // (it is not an infix operator), so a trailing comma here starts
                // a multi-assignment target list.
                if self.at(TokenKind::Comma) {
                    return Statement::Expression(self.parse_multiple_assignment(expr));
                }
                Statement::Expression(self.apply_statement_modifier(expr))
            }
        }
    }

    /// Parse the tail of a parallel assignment `t1, t2, ... = v1, v2, ...`,
    /// given the already-parsed first target. A trailing target may be an
    /// assignment splat (`a, *rest = xs`). Targets and values are parsed at a
    /// binding power above assignment so a target does not swallow the `=`.
    fn parse_multiple_assignment(&mut self, first: Expr) -> Expr {
        let start = first.span.clone();
        let mut targets = vec![first];
        let mut splat_target = None;
        while self.eat(TokenKind::Comma) {
            self.skip_newlines();
            let is_splat = self.eat(TokenKind::Star);
            if is_splat && splat_target.is_some() {
                self.error("parallel assignment permits only one splat target");
            }
            targets.push(self.parse_expr_bp(2));
            if is_splat && splat_target.is_none() {
                splat_target = Some(targets.len() - 1);
            }
        }
        if !self.eat(TokenKind::Eq) {
            // A comma list at statement level with no `=` is not a parallel
            // assignment. Ruxen has no bare comma expressions and bare calls
            // take a single argument, so this is a mistake (usually a missing
            // pair of parentheses). Report it clearly instead of cascading a
            // misleading "parallel assignment" arity error.
            self.error(
                "unexpected `,`: a comma list at statement level must be a \
                 parallel assignment `a, b = ...`; a multi-argument call needs \
                 parentheses, e.g. `f(a, b)`",
            );
            let span = self.span_from(&start);
            return Expr {
                kind: ExprKind::MultiAssign {
                    targets,
                    values: Vec::new(),
                    splat_target,
                },
                span,
            };
        }
        self.skip_newlines();
        let mut values = vec![self.parse_expr_bp(2)];
        while self.eat(TokenKind::Comma) {
            self.skip_newlines();
            values.push(self.parse_expr_bp(2));
        }
        let span = self.span_from(&start);
        if let Some(index) = splat_target {
            if index + 1 != targets.len() {
                self.error("assignment splat must be the final target; use `a, *rest = values`");
            }
        } else if targets.len() != values.len() {
            self.error(&format!(
                "parallel assignment needs equal counts: {} targets but {} values",
                targets.len(),
                values.len()
            ));
        }
        Expr {
            kind: ExprKind::MultiAssign {
                targets,
                values,
                splat_target,
            },
            span,
        }
    }

    /// Ruby trailing statement modifiers: `EXPR if C` / `EXPR unless C` /
    /// `EXPR while C` / `EXPR until C`. Each desugars to the matching block
    /// form wrapping the single statement. `if`/`while` etc. are not infix
    /// operators, so `parse_expression` already stopped before the keyword;
    /// here we consume it (if present) and wrap. No modifier keyword → the
    /// expression is returned unchanged.
    fn apply_statement_modifier(&mut self, expr: Expr) -> Expr {
        if !self.at_statement_modifier() {
            return expr;
        }
        let start = expr.span.clone();
        let modifier = self.current_kind().clone();
        self.advance(); // consume the modifier keyword
        self.skip_newlines();
        let condition = self.parse_expression();
        let span = self.span_from(&start);
        let body = Block {
            statements: vec![Statement::Expression(expr)],
            span: span.clone(),
        };
        let kind = match modifier {
            TokenKind::If => ExprKind::If(IfExpr {
                condition: Box::new(condition),
                then_body: body,
                elsif_clauses: Vec::new(),
                else_body: None,
                span: span.clone(),
            }),
            TokenKind::Unless => ExprKind::If(IfExpr {
                condition: Box::new(crate::parser::expr::control_flow::negate_condition(
                    condition,
                )),
                then_body: body,
                elsif_clauses: Vec::new(),
                else_body: None,
                span: span.clone(),
            }),
            TokenKind::While => ExprKind::While(WhileExpr {
                condition: Box::new(condition),
                body,
                span: span.clone(),
            }),
            TokenKind::Until => ExprKind::While(WhileExpr {
                condition: Box::new(crate::parser::expr::control_flow::negate_condition(
                    condition,
                )),
                body,
                span: span.clone(),
            }),
            _ => unreachable!("guarded by the matches! above"),
        };
        Expr { kind, span }
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
