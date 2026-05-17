//! Recursive-descent parser for the Riven programming language.
//!
//! Produces an AST from a token stream. Handles error recovery by
//! skipping to synchronization points and recording diagnostics.

pub mod ast;
pub mod attributes;
pub mod classes;
pub mod expr;
pub mod ffi;
pub mod methods;
pub mod patterns;
pub mod printer;
pub mod types;

#[cfg(test)]
mod tests;

use crate::diagnostics::Diagnostic;
use crate::lexer::token::{Span, Token, TokenKind};
use ast::*;

// ─── Parser Struct ──────────────────────────────────────────────────

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    diagnostics: Vec<Diagnostic>,
    /// Doc comments (`## ...`) accumulated while skipping leading trivia.
    /// Drained by `take_pending_docs()` when a definition is built.
    pending_doc_comments: Vec<String>,
}

// ─── Token Navigation ───────────────────────────────────────────────

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            pos: 0,
            diagnostics: Vec::new(),
            pending_doc_comments: Vec::new(),
        }
    }

    /// Consume any leading `##` doc-comment tokens, returning their bodies.
    /// The lexer has already stripped the `##` prefix and one optional
    /// space, so the captured strings are the doc body verbatim.
    fn collect_doc_comments(&mut self) -> Vec<String> {
        let mut docs = Vec::new();
        loop {
            // Allow blank newlines between consecutive `##` lines.
            self.skip_newlines();
            let body = match self.current_kind() {
                TokenKind::DocComment(body) => body.clone(),
                _ => break,
            };
            docs.push(body);
            self.advance();
        }
        docs
    }

    /// Drain any pending doc comments accumulated by ambient skipping into
    /// a fresh `Vec` for attachment to the next definition.
    fn take_pending_docs(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_doc_comments)
    }

    /// Return a reference to accumulated diagnostics.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Parse a single REPL input — may be an expression, statement, or top-level item.
    ///
    /// Returns `ReplParseResult::Incomplete` if the input has unclosed delimiters
    /// (e.g., `def` without `end`), signaling the REPL to request continuation lines.
    pub fn parse_repl_input(&mut self) -> ReplParseResult {
        self.skip_newlines();

        if self.at_eof() {
            return ReplParseResult::Incomplete;
        }

        // Save position for backtracking
        let saved_pos = self.pos;
        let saved_diags = self.diagnostics.len();

        // Try top-level item first (def, class, struct, enum, mixin, extension,
        // module, use, const). Legacy `trait`/`impl`/`extern` no longer lex.
        match self.current_kind().clone() {
            TokenKind::Def
            | TokenKind::Async
            | TokenKind::Class
            | TokenKind::Struct
            | TokenKind::Enum
            | TokenKind::Mixin
            | TokenKind::Extension
            | TokenKind::Module
            | TokenKind::Use
            | TokenKind::Const
            | TokenKind::Type
            | TokenKind::Newtype
            | TokenKind::Lib => {
                let result = self.parse_top_level_item();
                if self.diagnostics.len() > saved_diags {
                    // Check if the errors indicate incomplete input
                    let has_eof_error = self.diagnostics[saved_diags..].iter().any(|d| {
                        d.message.contains("expected End")
                            || d.message.contains("expected {:?}, found Eof")
                            || d.message.contains("found Eof")
                    });
                    if has_eof_error || self.at_eof() {
                        // Could be incomplete — check delimiter balance
                        self.pos = saved_pos;
                        self.diagnostics.truncate(saved_diags);
                        if self.check_incomplete() {
                            return ReplParseResult::Incomplete;
                        }
                        // Re-parse to get proper diagnostics
                        let _ = self.parse_top_level_item();
                    }
                    let diags = self.diagnostics[saved_diags..].to_vec();
                    return ReplParseResult::Error(diags);
                }
                match result {
                    Some(item) => ReplParseResult::Complete(ReplInput::TopLevel(item)),
                    None => {
                        let diags = self.diagnostics[saved_diags..].to_vec();
                        if diags.is_empty() {
                            ReplParseResult::Error(vec![Diagnostic::error(
                                "failed to parse top-level item",
                                self.current_span(),
                            )])
                        } else {
                            ReplParseResult::Error(diags)
                        }
                    }
                }
            }
            TokenKind::Protected => {
                let result = self.parse_top_level_item();
                if self.diagnostics.len() > saved_diags {
                    let has_eof_error = self.diagnostics[saved_diags..]
                        .iter()
                        .any(|d| d.message.contains("found Eof"));
                    if has_eof_error {
                        self.pos = saved_pos;
                        self.diagnostics.truncate(saved_diags);
                        if self.check_incomplete() {
                            return ReplParseResult::Incomplete;
                        }
                    }
                    let diags = self.diagnostics[saved_diags..].to_vec();
                    return ReplParseResult::Error(diags);
                }
                match result {
                    Some(item) => ReplParseResult::Complete(ReplInput::TopLevel(item)),
                    None => {
                        let diags = self.diagnostics[saved_diags..].to_vec();
                        ReplParseResult::Error(diags)
                    }
                }
            }
            // Let / var binding → Statement
            TokenKind::Let | TokenKind::Var => {
                let stmt = self.parse_statement();
                if self.diagnostics.len() > saved_diags {
                    let diags = self.diagnostics[saved_diags..].to_vec();
                    return ReplParseResult::Error(diags);
                }
                ReplParseResult::Complete(ReplInput::Statement(stmt))
            }
            // Everything else → try as expression
            _ => {
                let expr = self.parse_expression();
                if self.diagnostics.len() > saved_diags {
                    // Check if it's an incomplete expression (unclosed brackets etc)
                    self.pos = saved_pos;
                    self.diagnostics.truncate(saved_diags);
                    if self.check_incomplete() {
                        return ReplParseResult::Incomplete;
                    }
                    // Re-parse to get diagnostics
                    let _ = self.parse_expression();
                    let diags = self.diagnostics[saved_diags..].to_vec();
                    return ReplParseResult::Error(diags);
                }
                ReplParseResult::Complete(ReplInput::Expression(expr))
            }
        }
    }

    /// Check if the remaining tokens indicate an incomplete input
    /// (unclosed delimiters that need continuation lines).
    fn check_incomplete(&self) -> bool {
        let mut depth: i32 = 0;
        let mut paren_depth: i32 = 0;
        let mut bracket_depth: i32 = 0;
        let mut brace_depth: i32 = 0;

        for tok in &self.tokens {
            match &tok.kind {
                // Block openers
                TokenKind::Def
                | TokenKind::Async
                | TokenKind::Class
                | TokenKind::Struct
                | TokenKind::Enum
                | TokenKind::Mixin
                | TokenKind::Extension
                | TokenKind::Module
                | TokenKind::If
                | TokenKind::While
                | TokenKind::For
                | TokenKind::Loop
                | TokenKind::Match => depth += 1,
                // Do blocks also need end
                TokenKind::Do => depth += 1,
                TokenKind::End => depth -= 1,
                TokenKind::LParen => paren_depth += 1,
                TokenKind::RParen => paren_depth -= 1,
                TokenKind::LBracket => bracket_depth += 1,
                TokenKind::RBracket => bracket_depth -= 1,
                TokenKind::LBrace => brace_depth += 1,
                TokenKind::RBrace => brace_depth -= 1,
                TokenKind::Eof => break,
                _ => {}
            }
        }

        depth > 0 || paren_depth > 0 || bracket_depth > 0 || brace_depth > 0
    }

    /// Main entry point: parse a complete Riven program.
    pub fn parse(&mut self) -> Result<Program, Vec<Diagnostic>> {
        let start = self.current_span();
        let mut items = Vec::new();

        while !self.at_eof() {
            self.skip_newlines();
            if self.at_eof() {
                break;
            }
            let before = self.pos;
            match self.parse_top_level_item() {
                Some(item) => items.push(item),
                None => {
                    // Error recovery: skip to next sync point.
                    self.synchronize();
                    // If we landed on `end` at top level, skip it.
                    if self.at(TokenKind::End) {
                        self.advance();
                    }
                    // `synchronize()` is allowed to stop on any block-opening
                    // keyword (For, If, While, …). When `parse_top_level_item`
                    // doesn't accept that keyword either, we'd otherwise spin
                    // forever on a single token; force one tick of progress.
                    if self.pos == before && !self.at_eof() {
                        self.advance();
                    }
                }
            }
            self.skip_newlines();
        }

        let span = self.span_from(&start);
        let program = Program { items, span };

        if self
            .diagnostics
            .iter()
            .any(|d| d.level == crate::diagnostics::DiagnosticLevel::Error)
        {
            Err(self.diagnostics.clone())
        } else {
            Ok(program)
        }
    }

    // ─── Current / Peek / Advance ────────────────────────────────────

    pub(crate) fn current(&self) -> &Token {
        self.tokens
            .get(self.pos)
            .unwrap_or_else(|| self.tokens.last().unwrap())
    }

    pub(crate) fn current_kind(&self) -> &TokenKind {
        &self.current().kind
    }

    pub(crate) fn current_span(&self) -> Span {
        self.current().span.clone()
    }

    pub(crate) fn peek(&self) -> &Token {
        self.tokens
            .get(self.pos + 1)
            .unwrap_or_else(|| self.tokens.last().unwrap())
    }

    pub(crate) fn peek_kind(&self) -> TokenKind {
        self.peek().kind.clone()
    }

    pub(crate) fn peek_at(&self, offset: usize) -> &Token {
        self.tokens
            .get(self.pos + offset)
            .unwrap_or_else(|| self.tokens.last().unwrap())
    }

    pub(crate) fn peek_at_kind(&self, offset: usize) -> TokenKind {
        self.peek_at(offset).kind.clone()
    }

    pub(crate) fn advance(&mut self) -> &Token {
        let tok = self
            .tokens
            .get(self.pos)
            .unwrap_or_else(|| self.tokens.last().unwrap());
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    /// Universal anti-OOM guard for parser body-loops.
    ///
    /// Many `parse_X_def` methods iterate
    /// `while !self.at(End) && !self.at(Eof) { self.parse_X_item(); }`.
    /// If `parse_X_item` doesn't advance the cursor — typically because
    /// `expect_identifier` / `expect(kind)` reported a missing terminal
    /// without advancing on mismatch — the outer loop spins on the same
    /// token forever and unbounded-allocates placeholder AST nodes
    /// (≈1.25 GiB observed on `struct ... impl Display ... end ... end`
    /// before the targeted fix in `parse_struct_def`).
    ///
    /// Caveat: `Parser::synchronize()` stops at top-level keywords
    /// (`Impl`, `Def`, `Class`, ...) so it is a no-op when the
    /// offending token is itself a sync point. The body-loop *must*
    /// either consume the offending block via the appropriate
    /// sub-parser (`parse_inner_impl` / `parse_func_def`) OR call this
    /// helper to force one token of forward progress.
    ///
    /// Call at the BOTTOM of every body-loop iteration with the cursor
    /// position captured at the TOP. Returns whether progress was
    /// natural (`true`) or had to be forced (`false`); the boolean is
    /// useful for tests but most callers can ignore it.
    pub(crate) fn ensure_loop_progress(&mut self, before_pos: usize) -> bool {
        if self.pos > before_pos {
            return true;
        }
        if self.at(TokenKind::Eof) {
            // The outer loop's `!self.at(Eof)` guard will terminate
            // on the next check; nothing to advance past.
            return true;
        }
        // The body-loop iteration consumed zero tokens despite not
        // being at EOF. Emit a structured diagnostic and force-advance
        // so the loop cannot spin.
        self.error(&format!(
            "parser made no progress at {:?} — forcing advance to avoid an OOM loop \
             (this is a parser bug; please report)",
            self.current_kind()
        ));
        self.advance();
        false
    }

    pub(crate) fn at(&self, kind: TokenKind) -> bool {
        std::mem::discriminant(self.current_kind()) == std::mem::discriminant(&kind)
    }

    pub(crate) fn at_eof(&self) -> bool {
        matches!(self.current_kind(), TokenKind::Eof)
    }

    /// Consume the current token if it matches `kind`. Returns true if consumed.
    pub(crate) fn eat(&mut self, kind: TokenKind) -> bool {
        if self.at(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    /// Expect and consume a token of the given kind; emit error if mismatch.
    pub(crate) fn expect(&mut self, kind: TokenKind) -> bool {
        if self.at(kind.clone()) {
            self.advance();
            true
        } else {
            self.error(&format!(
                "expected {:?}, found {:?}",
                kind,
                self.current_kind()
            ));
            false
        }
    }

    pub(crate) fn skip_newlines(&mut self) {
        while self.at(TokenKind::Newline) {
            self.advance();
        }
    }

    /// Expect a statement terminator: newline, `;`, or EOF.
    pub(crate) fn expect_terminator(&mut self) {
        if self.at(TokenKind::Newline) || self.at(TokenKind::Semicolon) {
            self.advance();
        }
        // Also fine if we're at Eof, End, Else, Elsif, RBrace, RParen, RBracket
        // (contextual terminators)
    }

    // ─── Identifier Helpers ──────────────────────────────────────────

    /// Expect a lowercase identifier.
    pub(crate) fn expect_identifier(&mut self) -> String {
        match self.current_kind().clone() {
            TokenKind::Identifier(name) => {
                self.advance();
                name
            }
            _ => {
                self.error(&format!(
                    "expected identifier, found {:?}",
                    self.current_kind()
                ));
                "_error".to_string()
            }
        }
    }

    /// Expect a type identifier (uppercase).
    pub(crate) fn expect_type_identifier(&mut self) -> String {
        match self.current_kind().clone() {
            TokenKind::TypeIdentifier(name) => {
                self.advance();
                name
            }
            _ => {
                self.error(&format!(
                    "expected type identifier, found {:?}",
                    self.current_kind()
                ));
                "_Error".to_string()
            }
        }
    }

    /// Expect any kind of identifier (lowercase, type, or keyword that can be used as ident).
    pub(crate) fn expect_any_identifier(&mut self) -> String {
        match self.current_kind().clone() {
            TokenKind::Identifier(name) => {
                self.advance();
                name
            }
            TokenKind::TypeIdentifier(name) => {
                self.advance();
                name
            }
            TokenKind::Init => {
                self.advance();
                "init".to_string()
            }
            TokenKind::SelfValue => {
                self.advance();
                "self".to_string()
            }
            // ruby-naming.spec.md introduced `var` / `some` / `any`
            // as keywords. Path-segment and field/method-name positions
            // are unambiguous, so accept them as plain identifiers here
            // — e.g. `std.env.var`, `iter.any { … }`, `iter.some { … }`.
            TokenKind::Var => {
                self.advance();
                "var".to_string()
            }
            TokenKind::AnyBound => {
                self.advance();
                "any".to_string()
            }
            TokenKind::SomeBound => {
                self.advance();
                "some".to_string()
            }
            _ => {
                self.error(&format!(
                    "expected identifier, found {:?}",
                    self.current_kind()
                ));
                "_error".to_string()
            }
        }
    }

    // ─── Span Helpers ────────────────────────────────────────────────

    pub(crate) fn span_from(&self, start: &Span) -> Span {
        let end = if self.pos > 0 {
            &self.tokens[self.pos - 1].span
        } else {
            start
        };
        Span {
            start: start.start,
            end: end.end,
            line: start.line,
            column: start.column,
        }
    }

    // ─── Error Reporting ─────────────────────────────────────────────

    pub(crate) fn error(&mut self, message: &str) {
        let span = self.current_span();
        self.diagnostics.push(Diagnostic::error(message, span));
    }

    #[allow(dead_code)]
    pub(crate) fn error_at(&mut self, message: &str, span: Span) {
        self.diagnostics.push(Diagnostic::error(message, span));
    }

    pub(crate) fn error_at_with_code(&mut self, message: &str, span: Span, code: &str) {
        self.diagnostics
            .push(Diagnostic::error_with_code(message, span, code));
    }

    // ─── Error Recovery ──────────────────────────────────────────────

    pub(crate) fn synchronize(&mut self) {
        loop {
            match self.current_kind() {
                TokenKind::Eof => return,
                TokenKind::Let
                | TokenKind::Var
                | TokenKind::Def
                | TokenKind::Async
                | TokenKind::Class
                | TokenKind::Struct
                | TokenKind::Enum
                | TokenKind::Mixin
                | TokenKind::Extension
                | TokenKind::Module
                | TokenKind::Use
                | TokenKind::If
                | TokenKind::Match
                | TokenKind::While
                | TokenKind::For
                | TokenKind::Loop
                | TokenKind::End
                | TokenKind::Const
                | TokenKind::Type
                | TokenKind::Newtype
                | TokenKind::Public
                | TokenKind::Private
                | TokenKind::Protected => return,
                _ => {
                    self.advance();
                }
            }
        }
    }
}

// ─── Top-Level Item Parsing ─────────────────────────────────────────

impl Parser {
    fn parse_top_level_item(&mut self) -> Option<TopLevelItem> {
        self.skip_newlines();
        // Capture any leading `##` doc comments so the next item can pick
        // them up via `take_pending_docs()`.
        let docs = self.collect_doc_comments();
        if !docs.is_empty() {
            self.pending_doc_comments.extend(docs);
        }

        match self.current_kind().clone() {
            TokenKind::Module => Some(TopLevelItem::Module(self.parse_module_def())),
            TokenKind::Class => Some(TopLevelItem::Class(self.parse_class_def())),
            TokenKind::Struct => Some(TopLevelItem::Struct(self.parse_struct_def())),
            TokenKind::Enum => Some(TopLevelItem::Enum(self.parse_enum_def())),
            TokenKind::Mixin => Some(TopLevelItem::Mixin(self.parse_trait_def())),
            TokenKind::Extension => Some(TopLevelItem::Impl(self.parse_impl_block(false))),
            TokenKind::Unsafe if matches!(self.peek_kind(), TokenKind::Extension) => {
                self.advance(); // consume `unsafe`
                Some(TopLevelItem::Impl(self.parse_impl_block(true)))
            }
            TokenKind::Use => Some(TopLevelItem::Use(self.parse_use_decl())),
            TokenKind::Type => Some(TopLevelItem::TypeAlias(self.parse_type_alias())),
            TokenKind::Newtype => Some(TopLevelItem::Newtype(self.parse_newtype_def())),
            TokenKind::Const => Some(TopLevelItem::Const(self.parse_const_def())),
            TokenKind::Lib => Some(TopLevelItem::Lib(self.parse_lib_decl(vec![]))),
            TokenKind::At => {
                // ruby-naming.spec.md §10a: the `@[...]` prefix-attribute
                // surface is retired. `@[derive(...)]` → in-body `include
                // X` (or just rely on implicit-include per §3.6);
                // `@[repr(C)]` → `layout c`; `@[link("X")]` → `lib "X",
                // ...`; etc. Old fixtures that still use the prefix form
                // get a hard parser error.
                self.error_at_with_code(
                    "the `@[...]` prefix attribute is retired (ruby-naming.spec.md §10a) — \
                     use the in-body directive form (`include`, `layout`, `lib` options, \
                     `inline def`, etc.) instead",
                    self.current_span(),
                    "E0607",
                );
                self.advance(); // consume `@` to make progress; remainder errors normally
                None
            }
            TokenKind::Async => Some(TopLevelItem::Function(
                self.parse_func_def(Visibility::Private),
            )),
            TokenKind::Def => Some(TopLevelItem::Function(
                self.parse_func_def(Visibility::Private),
            )),
            TokenKind::Protected => {
                let vis = self.parse_visibility();
                match self.current_kind() {
                    TokenKind::Def | TokenKind::Async => {
                        Some(TopLevelItem::Function(self.parse_func_def(vis)))
                    }
                    _ => {
                        self.error("expected `def` after visibility modifier at top level");
                        None
                    }
                }
            }
            _ => {
                self.error(&format!(
                    "expected top-level declaration, found {:?}",
                    self.current_kind()
                ));
                None
            }
        }
    }

    fn parse_visibility(&mut self) -> Visibility {
        match self.current_kind() {
            TokenKind::Protected => {
                self.advance();
                Visibility::Protected
            }
            _ => Visibility::Private,
        }
    }

    // ─── Module ──────────────────────────────────────────────────────

    fn parse_module_def(&mut self) -> ModuleDef {
        let start = self.current_span();
        self.advance(); // consume module
        let name = self.expect_any_identifier();
        self.skip_newlines();

        let mut items = Vec::new();
        // ruby-naming.spec.md §3.2: module bodies honour section markers.
        // Module items inherit `current_vis` only when they expose a
        // `visibility` field (Function, Const). Nested types
        // (Class/Struct/Module/...) carry their own visibility model.
        let mut current_vis = Visibility::Public;
        let mut name_list_overrides: Vec<(Visibility, Vec<String>)> = Vec::new();
        while !self.at(TokenKind::End) && !self.at(TokenKind::Eof) {
            let __progress = self.pos;
            self.skip_newlines();
            if self.at(TokenKind::End) {
                break;
            }
            // Handle bare section markers BEFORE delegating to
            // `parse_top_level_item`. `parse_top_level_item` doesn't
            // know about Public/Private as section markers.
            match self.current_kind().clone() {
                TokenKind::Public => {
                    self.advance();
                    if self.at(TokenKind::Colon) {
                        let names = self.parse_visibility_name_list();
                        name_list_overrides.push((Visibility::Public, names));
                    } else {
                        current_vis = Visibility::Public;
                    }
                    self.skip_newlines();
                    self.ensure_loop_progress(__progress);
                    continue;
                }
                TokenKind::Private => {
                    self.advance();
                    if self.at(TokenKind::Colon) {
                        let names = self.parse_visibility_name_list();
                        name_list_overrides.push((Visibility::Private, names));
                    } else {
                        current_vis = Visibility::Private;
                    }
                    self.skip_newlines();
                    self.ensure_loop_progress(__progress);
                    continue;
                }
                TokenKind::Protected => {
                    // Only treat as bare-marker if not followed by a decl.
                    let next = self.peek_kind();
                    let bare = !matches!(next, TokenKind::Def | TokenKind::Async);
                    if bare {
                        self.advance();
                        if self.at(TokenKind::Colon) {
                            let names = self.parse_visibility_name_list();
                            name_list_overrides.push((Visibility::Protected, names));
                        } else {
                            current_vis = Visibility::Protected;
                        }
                        self.skip_newlines();
                        self.ensure_loop_progress(__progress);
                        continue;
                    }
                    // else: fall through and let parse_top_level_item handle it.
                }
                _ => {}
            }
            if let Some(item) = self.parse_top_level_item() {
                // Stamp section visibility onto items that did not carry
                // an explicit prefix. Items without a visibility field
                // (e.g. nested types) are unaffected here — their own
                // body parsers apply section markers internally.
                let stamped = match item {
                    TopLevelItem::Function(mut f) => {
                        if f.visibility == Visibility::Private {
                            f.visibility = current_vis;
                        }
                        TopLevelItem::Function(f)
                    }
                    other => other,
                };
                items.push(stamped);
            } else {
                self.synchronize();
            }
            self.ensure_loop_progress(__progress);
        }
        self.expect(TokenKind::End);
        // Apply name-list overrides on functions (top-level methods).
        for (vis, names) in &name_list_overrides {
            for n in names {
                for item in items.iter_mut() {
                    if let TopLevelItem::Function(f) = item {
                        if &f.name == n {
                            f.visibility = *vis;
                        }
                    }
                }
            }
        }
        let span = self.span_from(&start);
        ModuleDef { name, items, span }
    }

    // ─── Use Declaration ─────────────────────────────────────────────

    fn parse_use_decl(&mut self) -> UseDecl {
        let start = self.current_span();
        self.advance(); // consume use

        let mut path = Vec::new();
        // Module path: segments separated by .
        path.push(self.expect_any_identifier());
        while self.at(TokenKind::Dot) {
            // Continue while the next token is an identifier segment.
            // `Var` is whitelisted as a path segment because the stdlib
            // uses `std.env.var` (ruby-naming.spec.md introduced `var` as
            // a binding keyword but the path slot is unambiguous).
            if matches!(
                self.peek_kind(),
                TokenKind::Identifier(_) | TokenKind::TypeIdentifier(_) | TokenKind::Var
            ) {
                self.advance(); // consume .
                path.push(self.expect_any_identifier());
            } else if self.peek_kind() == TokenKind::LBrace {
                // use Path.{A, B}
                self.advance(); // consume .
                break;
            } else {
                break;
            }
        }

        let kind = if self.at(TokenKind::LBrace) {
            // Group import: use Path.{A, B, C}
            self.advance(); // consume {
            self.skip_newlines();
            let mut names = Vec::new();
            if !self.at(TokenKind::RBrace) {
                names.push(self.expect_any_identifier());
                while self.eat(TokenKind::Comma) {
                    self.skip_newlines();
                    if self.at(TokenKind::RBrace) {
                        break;
                    }
                    names.push(self.expect_any_identifier());
                }
            }
            self.skip_newlines();
            self.expect(TokenKind::RBrace);
            UseKind::Group(names)
        } else if self.eat(TokenKind::As) {
            let alias = self.expect_any_identifier();
            UseKind::Alias(alias)
        } else {
            UseKind::Simple
        };

        let span = self.span_from(&start);
        UseDecl { path, kind, span }
    }

    // ─── Type Alias ──────────────────────────────────────────────────

    fn parse_type_alias(&mut self) -> TypeAliasDef {
        let start = self.current_span();
        self.advance(); // consume type
        let name = self.expect_type_identifier();

        let generic_params = if self.at(TokenKind::LBracket) {
            Some(self.parse_generic_params())
        } else {
            None
        };

        self.expect(TokenKind::Eq);
        self.skip_newlines();
        let type_expr = self.parse_type();
        let span = self.span_from(&start);
        TypeAliasDef {
            name,
            generic_params,
            type_expr,
            span,
        }
    }

    // ─── Newtype ─────────────────────────────────────────────────────

    fn parse_newtype_def(&mut self) -> NewtypeDef {
        let start = self.current_span();
        self.advance(); // consume newtype
        let name = self.expect_type_identifier();
        self.expect(TokenKind::LParen);
        let inner_type = self.parse_type();
        self.expect(TokenKind::RParen);
        let span = self.span_from(&start);
        NewtypeDef {
            name,
            inner_type,
            span,
        }
    }

    // ─── Const ───────────────────────────────────────────────────────

    fn parse_const_def(&mut self) -> ConstDef {
        let doc_comments = self.take_pending_docs();
        let start = self.current_span();
        self.advance(); // consume const
        let name = self.expect_type_identifier();
        // Type annotation is optional: `const NAME = val` infers the type from
        // the RHS. `const NAME: Type = val` still works.
        let type_expr = if self.eat(TokenKind::Colon) {
            self.parse_type()
        } else {
            TypeExpr::Inferred {
                span: self.current_span(),
            }
        };
        self.expect(TokenKind::Eq);
        self.skip_newlines();
        let value = self.parse_expression();
        let span = self.span_from(&start);
        ConstDef {
            name,
            type_expr,
            value,
            doc_comments,
            span,
        }
    }
}
