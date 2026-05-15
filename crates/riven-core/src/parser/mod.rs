//! Recursive-descent parser for the Riven programming language.
//!
//! Produces an AST from a token stream. Handles error recovery by
//! skipping to synchronization points and recording diagnostics.

pub mod ast;
pub mod expr;
pub mod patterns;
pub mod printer;
pub mod types;

#[cfg(test)]
mod tests;

use crate::diagnostics::Diagnostic;
use crate::lexer::token::{Span, Token, TokenKind};
use ast::*;

/// Internal helper for parsing method signatures before deciding if body follows.
struct ParsedMethodSig {
    vis: Visibility,
    is_async: bool,
    self_mode: Option<SelfMode>,
    is_class_method: bool,
    name: String,
    generic_params: Option<GenericParams>,
    params: Vec<Param>,
    return_type: Option<TypeExpr>,
}

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
            | TokenKind::Impl
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
            TokenKind::Pub | TokenKind::Protected => {
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
                | TokenKind::Impl
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
            // ruby-naming.spec.md introduced `var` as a keyword, but
            // stdlib paths like `std.env.var` still need it to act as
            // a plain identifier in module paths. Accept it here.
            TokenKind::Var => {
                self.advance();
                "var".to_string()
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
                | TokenKind::Impl
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
                | TokenKind::Pub
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
            TokenKind::Extension => {
                Some(TopLevelItem::Impl(self.parse_impl_block(false)))
            }
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
                // Parse @[...] attributes, then the item that follows.
                // Recognised attributes:
                //   @[link("name")]            — applies to `lib`
                //   @[repr(C)] / @[repr(C, packed)] — applies to `struct`
                //   @[derive(Trait1, Trait2)]  — applies to struct/class/enum;
                //                                produces the same HIR as the
                //                                body-form `derive Trait1, Trait2`
                let attrs = self.parse_attributes();
                self.skip_newlines();
                let has_derive_attr = attrs.iter().any(|attr| attr.name == "derive");
                match self.current_kind() {
                    TokenKind::Lib => {
                        let mut lib = self.parse_lib_decl(vec![]);
                        for attr in attrs {
                            if attr.name == "link" {
                                for arg in &attr.args {
                                    lib.link_attrs.push(LinkAttr {
                                        name: arg.as_str().to_string(),
                                        kind: LinkKind::Dynamic,
                                    });
                                }
                            } else {
                                self.error(&format!(
                                    "attribute `{}` is not valid on `lib`",
                                    attr.name
                                ));
                            }
                        }
                        Some(TopLevelItem::Lib(lib))
                    }
                    TokenKind::Struct => {
                        let mut s = self.parse_struct_def();
                        self.apply_struct_attrs(&mut s, &attrs);
                        Some(TopLevelItem::Struct(s))
                    }
                    TokenKind::Class => {
                        let mut c = self.parse_class_def();
                        self.apply_class_attrs(&mut c, &attrs);
                        Some(TopLevelItem::Class(c))
                    }
                    TokenKind::Enum => {
                        let mut e = self.parse_enum_def();
                        self.apply_enum_attrs(&mut e, &attrs);
                        Some(TopLevelItem::Enum(e))
                    }
                    TokenKind::Mixin => {
                        if has_derive_attr {
                            for attr in &attrs {
                                if attr.name == "derive" {
                                    self.error_at_with_code(
                                        "@[derive] cannot be applied to a mixin",
                                        attr.span.clone(),
                                        "E0607",
                                    );
                                }
                            }
                        }
                        Some(TopLevelItem::Mixin(self.parse_trait_def()))
                    }
                    TokenKind::Def => {
                        if has_derive_attr {
                            for attr in &attrs {
                                if attr.name == "derive" {
                                    self.error_at_with_code(
                                        "@[derive] cannot be applied to a function",
                                        attr.span.clone(),
                                        "E0607",
                                    );
                                }
                            }
                        }
                        Some(TopLevelItem::Function(
                            self.parse_func_def(Visibility::Private),
                        ))
                    }
                    TokenKind::Pub | TokenKind::Protected => {
                        if has_derive_attr {
                            for attr in &attrs {
                                if attr.name == "derive" {
                                    self.error_at_with_code(
                                        "@[derive] cannot be applied to a function",
                                        attr.span.clone(),
                                        "E0607",
                                    );
                                }
                            }
                        }
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
                        self.error("expected declaration after attribute");
                        None
                    }
                }
            }
            TokenKind::Async => Some(TopLevelItem::Function(
                self.parse_func_def(Visibility::Private),
            )),
            TokenKind::Def => Some(TopLevelItem::Function(
                self.parse_func_def(Visibility::Private),
            )),
            TokenKind::Pub => {
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
            TokenKind::Pub => {
                self.advance();
                Visibility::Public
            }
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
                    let bare = !matches!(
                        next,
                        TokenKind::Def | TokenKind::Async | TokenKind::Pub
                    );
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

    // ─── Enum ────────────────────────────────────────────────────────

    fn parse_enum_def(&mut self) -> EnumDef {
        let doc_comments = self.take_pending_docs();
        let start = self.current_span();
        self.advance(); // consume enum
        let name = self.expect_type_identifier();

        let generic_params = if self.at(TokenKind::LBracket) {
            Some(self.parse_generic_params())
        } else {
            None
        };
        // T2.02 S9: optional `where ...` clause between header and body.
        let where_clause = if self.at(TokenKind::Where) {
            Some(self.parse_where_clause())
        } else {
            None
        };
        self.skip_newlines();

        let mut variants = Vec::new();
        let mut methods: Vec<FuncDef> = Vec::new();
        let mut inner_impls: Vec<InnerImpl> = Vec::new();
        let mut derive_traits = Vec::new();
        // ruby-naming.spec.md §3.2: section-marker visibility for inline
        // method declarations. Default public, mirrors `parse_class_def`.
        let mut current_vis = Visibility::Public;
        let mut name_list_overrides: Vec<(Visibility, Vec<String>)> = Vec::new();
        while !self.at(TokenKind::End) && !self.at(TokenKind::Eof) {
            let __progress = self.pos;
            self.skip_newlines();
            if self.at(TokenKind::End) {
                break;
            }
            // Capture leading `##` doc comments so the next inner item
            // (variant / method / inner impl) can pick them up.
            let docs = self.collect_doc_comments();
            if !docs.is_empty() {
                self.pending_doc_comments.extend(docs);
            }
            if self.at(TokenKind::End) {
                break;
            }
            match self.current_kind().clone() {
                // ── derive lines ─────────────────────────────────────
                TokenKind::Derive => {
                    self.advance();
                    derive_traits.push(self.expect_type_identifier());
                    while self.eat(TokenKind::Comma) {
                        self.skip_newlines();
                        derive_traits.push(self.expect_type_identifier());
                    }
                }
                // ── Section markers (ruby-naming.spec.md §3.2) ──
                TokenKind::Public => {
                    self.advance();
                    if self.at(TokenKind::Colon) {
                        let names = self.parse_visibility_name_list();
                        name_list_overrides.push((Visibility::Public, names));
                    } else {
                        current_vis = Visibility::Public;
                    }
                }
                TokenKind::Private => {
                    self.advance();
                    if self.at(TokenKind::Colon) {
                        let names = self.parse_visibility_name_list();
                        name_list_overrides.push((Visibility::Private, names));
                    } else {
                        current_vis = Visibility::Private;
                    }
                }
                TokenKind::Protected => {
                    let next = self.peek_kind();
                    let prefix_form = matches!(
                        next,
                        TokenKind::Def | TokenKind::Async | TokenKind::Pub
                    );
                    if prefix_form {
                        let vis = self.parse_visibility();
                        if matches!(self.current_kind(), TokenKind::Def | TokenKind::Async) {
                            methods.push(self.parse_func_def(vis));
                        } else {
                            // Fall back to error: we don't accept fields on enum bodies.
                            self.error(&format!(
                                "expected `def` after explicit visibility in enum body, found {:?}",
                                self.current_kind()
                            ));
                            if !self.at(TokenKind::Eof) {
                                self.advance();
                            }
                            self.synchronize();
                        }
                    } else {
                        self.advance();
                        if self.at(TokenKind::Colon) {
                            let names = self.parse_visibility_name_list();
                            name_list_overrides.push((Visibility::Protected, names));
                        } else {
                            current_vis = Visibility::Protected;
                        }
                    }
                }
                // ── Method definitions ──────────────────────────────
                TokenKind::Def | TokenKind::Async => {
                    methods.push(self.parse_func_def(current_vis));
                }
                TokenKind::Pub => {
                    let vis = self.parse_visibility();
                    if matches!(self.current_kind(), TokenKind::Def | TokenKind::Async) {
                        methods.push(self.parse_func_def(vis));
                    } else {
                        self.error(&format!(
                            "expected `def` after `pub` in enum body, found {:?}",
                            self.current_kind()
                        ));
                        if !self.at(TokenKind::Eof) {
                            self.advance();
                        }
                        self.synchronize();
                    }
                }
                // ── Inner impl / include directives ─────────────────
                TokenKind::Impl => {
                    inner_impls.push(self.parse_inner_impl(false));
                }
                TokenKind::Include => {
                    inner_impls.push(self.parse_include_directive(false));
                }
                TokenKind::Unsafe if matches!(self.peek_kind(), TokenKind::Impl) => {
                    self.advance();
                    inner_impls.push(self.parse_inner_impl(true));
                }
                TokenKind::Unsafe if matches!(self.peek_kind(), TokenKind::Include) => {
                    self.advance();
                    inner_impls.push(self.parse_include_directive(true));
                }
                TokenKind::Inline => {
                    // `inline` modifier on a method — consume and require def
                    // to follow. Parser currently treats `inline` as a hint
                    // only; codegen ignores it. Mirror class-body behavior.
                    self.advance();
                    if matches!(self.current_kind(), TokenKind::Def | TokenKind::Async) {
                        methods.push(self.parse_func_def(current_vis));
                    } else {
                        self.error(&format!(
                            "expected `def` after `inline` in enum body, found {:?}",
                            self.current_kind()
                        ));
                        if !self.at(TokenKind::Eof) {
                            self.advance();
                        }
                        self.synchronize();
                    }
                }
                // ── Default: variant declaration ────────────────────
                _ => {
                    variants.push(self.parse_variant());
                }
            }
            self.skip_newlines();
            self.ensure_loop_progress(__progress);
        }
        self.expect(TokenKind::End);
        // Apply `private :a, :b` style name-list overrides as a final pass.
        for (vis, names) in &name_list_overrides {
            for n in names {
                if let Some(m) = methods.iter_mut().find(|m| &m.name == n) {
                    m.visibility = *vis;
                }
            }
        }
        let span = self.span_from(&start);
        EnumDef {
            name,
            generic_params,
            variants,
            methods,
            inner_impls,
            derive_traits,
            doc_comments,
            where_clause,
            span,
        }
    }

    fn parse_variant(&mut self) -> Variant {
        let start = self.current_span();
        // Variant names are ordinarily `TypeIdentifier`s, but the lexer
        // reserves `Some`, `None`, `Ok`, `Err` as keywords so that the
        // stdlib enum syntax can lower them specially. Inside a user
        // enum definition (e.g. `enum MyOpt[T] { Some(T), None }`), those
        // keywords are the variant names. Accept them here so the parser
        // doesn't spin generating diagnostics on a non-advancing token —
        // that previously OOMed the compiler on any user enum that
        // re-used an Option/Result variant name.
        let name = match self.current_kind() {
            TokenKind::SomeKw => {
                self.advance();
                "Some".to_string()
            }
            // Under ruby-naming, the lexer maps `nil` → `TokenKind::Nil`.
            // The legacy `NoneKw` (from `None`) is no longer produced but the
            // variant remains for backwards-compat with code that pattern-
            // matches it. Both lower to a `"None"`-named variant.
            TokenKind::Nil | TokenKind::NoneKw => {
                self.advance();
                "None".to_string()
            }
            TokenKind::OkKw => {
                self.advance();
                "Ok".to_string()
            }
            TokenKind::ErrKw => {
                self.advance();
                "Err".to_string()
            }
            _ => self.expect_type_identifier(),
        };

        let fields = if self.at(TokenKind::LParen) {
            self.advance(); // consume (
            self.skip_newlines();
            let mut fields = Vec::new();
            if !self.at(TokenKind::RParen) {
                fields.push(self.parse_variant_field());
                while self.eat(TokenKind::Comma) {
                    self.skip_newlines();
                    if self.at(TokenKind::RParen) {
                        break;
                    }
                    fields.push(self.parse_variant_field());
                }
            }
            self.skip_newlines();
            self.expect(TokenKind::RParen);
            // Determine tuple vs struct based on whether fields have names
            if fields.iter().any(|f| f.name.is_some()) {
                VariantKind::Struct(fields)
            } else {
                VariantKind::Tuple(fields)
            }
        } else if self.at(TokenKind::LBrace) {
            self.advance(); // consume {
            self.skip_newlines();
            let mut fields = Vec::new();
            if !self.at(TokenKind::RBrace) {
                fields.push(self.parse_variant_field());
                while self.eat(TokenKind::Comma) {
                    self.skip_newlines();
                    if self.at(TokenKind::RBrace) {
                        break;
                    }
                    fields.push(self.parse_variant_field());
                }
            }
            self.skip_newlines();
            self.expect(TokenKind::RBrace);
            VariantKind::Struct(fields)
        } else {
            VariantKind::Unit
        };

        let span = self.span_from(&start);
        Variant { name, fields, span }
    }

    fn parse_variant_field(&mut self) -> VariantField {
        let start = self.current_span();
        // Check for named field: name: Type
        if let TokenKind::Identifier(ref name) = self.current_kind().clone() {
            let name_val = name.clone();
            if self.peek_kind() == TokenKind::Colon {
                self.advance(); // consume name
                self.advance(); // consume :
                self.skip_newlines();
                let type_expr = self.parse_type();
                let span = self.span_from(&start);
                return VariantField {
                    name: Some(name_val),
                    type_expr,
                    span,
                };
            }
        }
        // Just a type
        let type_expr = self.parse_type();
        let span = self.span_from(&start);
        VariantField {
            name: None,
            type_expr,
            span,
        }
    }

    // ─── Struct ──────────────────────────────────────────────────────

    fn parse_struct_def(&mut self) -> StructDef {
        let doc_comments = self.take_pending_docs();
        let start = self.current_span();
        self.advance(); // consume struct
        let name = self.expect_type_identifier();

        let generic_params = if self.at(TokenKind::LBracket) {
            Some(self.parse_generic_params())
        } else {
            None
        };
        // T2.02 S9: optional `where ...` clause between header and body.
        let where_clause = if self.at(TokenKind::Where) {
            Some(self.parse_where_clause())
        } else {
            None
        };
        self.skip_newlines();

        let mut fields = Vec::new();
        let mut methods: Vec<FuncDef> = Vec::new();
        let mut inner_impls: Vec<InnerImpl> = Vec::new();
        let mut derive_traits = Vec::new();
        // ruby-naming.spec.md §3.2: section-marker visibility, public default.
        let mut current_vis = Visibility::Public;
        let mut name_list_overrides: Vec<(Visibility, Vec<String>)> = Vec::new();

        while !self.at(TokenKind::End) && !self.at(TokenKind::Eof) {
            let __progress = self.pos;
            self.skip_newlines();
            if self.at(TokenKind::End) {
                break;
            }
            // Capture leading `##` doc comments so the next inner item
            // (field / method / inner impl) can pick them up.
            let docs = self.collect_doc_comments();
            if !docs.is_empty() {
                self.pending_doc_comments.extend(docs);
            }
            if self.at(TokenKind::End) {
                break;
            }

            match self.current_kind().clone() {
                // ── Section markers (ruby-naming.spec.md §3.2) ──
                TokenKind::Public => {
                    self.advance();
                    if self.at(TokenKind::Colon) {
                        let names = self.parse_visibility_name_list();
                        name_list_overrides.push((Visibility::Public, names));
                    } else {
                        current_vis = Visibility::Public;
                    }
                }
                TokenKind::Private => {
                    self.advance();
                    if self.at(TokenKind::Colon) {
                        let names = self.parse_visibility_name_list();
                        name_list_overrides.push((Visibility::Private, names));
                    } else {
                        current_vis = Visibility::Private;
                    }
                }
                TokenKind::Protected => {
                    let next = self.peek_kind();
                    let prefix_form = matches!(
                        next,
                        TokenKind::Def
                            | TokenKind::Async
                            | TokenKind::Identifier(_)
                            | TokenKind::Pub
                    );
                    if prefix_form {
                        let vis = self.parse_visibility();
                        if matches!(self.current_kind(), TokenKind::Def | TokenKind::Async) {
                            methods.push(self.parse_func_def(vis));
                        } else {
                            fields.push(self.parse_field_decl_with_vis(vis));
                        }
                    } else {
                        self.advance();
                        if self.at(TokenKind::Colon) {
                            let names = self.parse_visibility_name_list();
                            name_list_overrides.push((Visibility::Protected, names));
                        } else {
                            current_vis = Visibility::Protected;
                        }
                    }
                }
                TokenKind::Derive => {
                    self.advance();
                    // derive Trait1, Trait2, ...
                    derive_traits.push(self.expect_type_identifier());
                    while self.eat(TokenKind::Comma) {
                        self.skip_newlines();
                        derive_traits.push(self.expect_type_identifier());
                    }
                }
                // ── Method definitions (post Ruby-naming migration) ──
                TokenKind::Def | TokenKind::Async => {
                    methods.push(self.parse_func_def(current_vis));
                }
                TokenKind::Pub => {
                    // Explicit `pub` prefix wins over section state.
                    let vis = self.parse_visibility();
                    if matches!(self.current_kind(), TokenKind::Def | TokenKind::Async) {
                        methods.push(self.parse_func_def(vis));
                    } else {
                        fields.push(self.parse_field_decl_with_vis(vis));
                    }
                }
                // ── Inner impl / include directives ─────────────────
                TokenKind::Impl => {
                    inner_impls.push(self.parse_inner_impl(false));
                }
                TokenKind::Include => {
                    inner_impls.push(self.parse_include_directive(false));
                }
                TokenKind::Unsafe if matches!(self.peek_kind(), TokenKind::Impl) => {
                    self.advance();
                    inner_impls.push(self.parse_inner_impl(true));
                }
                TokenKind::Unsafe if matches!(self.peek_kind(), TokenKind::Include) => {
                    self.advance();
                    inner_impls.push(self.parse_include_directive(true));
                }
                TokenKind::Inline => {
                    self.advance();
                    if matches!(self.current_kind(), TokenKind::Def | TokenKind::Async) {
                        methods.push(self.parse_func_def(current_vis));
                    } else {
                        self.error(&format!(
                            "expected `def` after `inline` in struct body, found {:?}",
                            self.current_kind()
                        ));
                        if !self.at(TokenKind::Eof) {
                            self.advance();
                        }
                        self.synchronize();
                    }
                }
                TokenKind::Identifier(_) => {
                    fields.push(self.parse_field_decl_with_vis(current_vis));
                }
                _ => {
                    self.error(&format!(
                        "expected field, method, or impl in struct body, found {:?}",
                        self.current_kind()
                    ));
                    // Hard-advance one token so we cannot loop on a sync
                    // keyword that `synchronize()` would itself stop at
                    // (e.g. `Impl`, `Def`, `Class`).
                    if !self.at(TokenKind::Eof) {
                        self.advance();
                    }
                    self.synchronize();
                }
            }
            self.skip_newlines();
            self.ensure_loop_progress(__progress);
        }
        self.expect(TokenKind::End);
        // Apply `private :a, :b` style name-list overrides on methods.
        for (vis, names) in &name_list_overrides {
            for n in names {
                if let Some(m) = methods.iter_mut().find(|m| &m.name == n) {
                    m.visibility = *vis;
                }
            }
        }
        let span = self.span_from(&start);
        StructDef {
            name,
            generic_params,
            fields,
            methods,
            inner_impls,
            derive_traits,
            repr: Vec::new(),
            doc_comments,
            where_clause,
            span,
        }
    }

    // ─── Trait ───────────────────────────────────────────────────────

    fn parse_trait_def(&mut self) -> MixinDef {
        let doc_comments = self.take_pending_docs();
        let start = self.current_span();
        self.advance(); // consume trait
        let name = self.expect_type_identifier();

        let generic_params = if self.at(TokenKind::LBracket) {
            Some(self.parse_generic_params())
        } else {
            None
        };

        let super_traits = if self.eat(TokenKind::Colon) {
            self.parse_trait_bounds()
        } else {
            vec![]
        };
        self.skip_newlines();

        let mut items = Vec::new();
        // ruby-naming.spec.md §3.2: mixin (= `trait` token) body honours
        // section markers. Default is Public. The trait-item parser
        // (`parse_trait_item`) does not see the section state, so we
        // overwrite the visibility on default-method items after the
        // sub-parser returns. MethodSig items in traits are signatures
        // only; for mixins (no `visibility` field on MethodSig) the
        // resolver synthesizes visibility from the section as needed.
        let mut current_vis = Visibility::Public;
        let mut name_list_overrides: Vec<(Visibility, Vec<String>)> = Vec::new();
        while !self.at(TokenKind::End) && !self.at(TokenKind::Eof) {
            let __progress = self.pos;
            self.skip_newlines();
            if self.at(TokenKind::End) {
                break;
            }
            // Capture leading `##` doc comments so the next inner item
            // (method sig / default method) can pick them up.
            let docs = self.collect_doc_comments();
            if !docs.is_empty() {
                self.pending_doc_comments.extend(docs);
            }
            if self.at(TokenKind::End) {
                break;
            }
            match self.current_kind().clone() {
                TokenKind::Public => {
                    self.advance();
                    if self.at(TokenKind::Colon) {
                        let names = self.parse_visibility_name_list();
                        name_list_overrides.push((Visibility::Public, names));
                    } else {
                        current_vis = Visibility::Public;
                    }
                }
                TokenKind::Private => {
                    self.advance();
                    if self.at(TokenKind::Colon) {
                        let names = self.parse_visibility_name_list();
                        name_list_overrides.push((Visibility::Private, names));
                    } else {
                        current_vis = Visibility::Private;
                    }
                }
                TokenKind::Protected => {
                    // Bare-marker section form vs. legacy prefix form.
                    let next = self.peek_kind();
                    let prefix_form = matches!(
                        next,
                        TokenKind::Def | TokenKind::Async | TokenKind::Pub
                    );
                    if prefix_form {
                        let item = self.parse_trait_item();
                        items.push(item);
                    } else {
                        self.advance();
                        if self.at(TokenKind::Colon) {
                            let names = self.parse_visibility_name_list();
                            name_list_overrides.push((Visibility::Protected, names));
                        } else {
                            current_vis = Visibility::Protected;
                        }
                    }
                }
                _ => {
                    let mut item = self.parse_trait_item();
                    // Apply section visibility to default methods that did
                    // not have an explicit `pub` / `protected` prefix
                    // (which `parse_visibility` would have set to non-Private).
                    if let MixinItem::DefaultMethod(func) = &mut item {
                        if func.visibility == Visibility::Private {
                            func.visibility = current_vis;
                        }
                    }
                    items.push(item);
                }
            }
            self.skip_newlines();
            self.ensure_loop_progress(__progress);
        }
        self.expect(TokenKind::End);
        // Apply `private :name` name-list overrides on default methods.
        for (vis, names) in &name_list_overrides {
            for n in names {
                for item in items.iter_mut() {
                    if let MixinItem::DefaultMethod(func) = item {
                        if &func.name == n {
                            func.visibility = *vis;
                        }
                    }
                }
            }
        }
        let span = self.span_from(&start);
        MixinDef {
            name,
            generic_params,
            super_traits,
            items,
            doc_comments,
            span,
        }
    }

    fn parse_trait_item(&mut self) -> MixinItem {
        // Doc comments accumulated by `parse_trait_def`'s body loop apply to
        // the next inner item. They flow through `take_pending_docs()` to
        // any `FuncDef` we build below.
        let doc_comments = self.take_pending_docs();
        let start = self.current_span();

        if self.at(TokenKind::Type) {
            // Associated type: type Name
            self.advance();
            let name = self.expect_type_identifier();
            let span = self.span_from(&start);
            return MixinItem::AssocType { name, span };
        }

        // Method signature or default method
        // Could have visibility
        let vis = self.parse_visibility();
        // Should be `def`
        if !matches!(self.current_kind(), TokenKind::Def | TokenKind::Async) {
            self.error("expected `def` or `type` in trait body");
            self.synchronize();
            return MixinItem::AssocType {
                name: "_error".to_string(),
                span: start,
            };
        }

        // Parse method header (signature) manually, then check if body follows
        let sig = self.parse_method_signature(vis);

        self.skip_newlines();

        // Determine if this is a signature-only method or a default method with body.
        //
        // Three cases:
        // 1. `{ expr }` → single-expression default method (brace body)
        // 2. Next token is `end` → default method with empty body, consume `end`
        // 3. Next token is `def`, `pub`, `protected`, `type`, or trait-closing `end`
        //    context → signature-only method (no body, no `end` to consume)
        // 4. Next token starts an expression/statement → default method, parse body + `end`
        if self.at(TokenKind::LBrace) {
            // Case 1: Single-expression body { expr }
            self.advance();
            self.skip_newlines();
            let expr = self.parse_expression();
            self.skip_newlines();
            self.expect(TokenKind::RBrace);
            let body_span = self.span_from(&start);
            let body = Block {
                statements: vec![Statement::Expression(expr)],
                span: body_span,
            };
            let span = self.span_from(&start);
            MixinItem::DefaultMethod(FuncDef {
                visibility: sig.vis,
                is_async: sig.is_async,
                self_mode: sig.self_mode,
                is_class_method: sig.is_class_method,
                name: sig.name,
                generic_params: sig.generic_params,
                params: sig.params,
                return_type: sig.return_type,
                where_clause: None,
                body,
                doc_comments,
                span,
            })
        } else if matches!(
            self.current_kind(),
            TokenKind::Def
                | TokenKind::Async
                | TokenKind::Pub
                | TokenKind::Public
                | TokenKind::Private
                | TokenKind::Protected
                | TokenKind::Type
                | TokenKind::End
                | TokenKind::Eof
        ) {
            // Case 3: Next declaration keyword → signature only, no body
            let span = self.span_from(&start);
            MixinItem::MethodSig(MethodSig {
                is_async: sig.is_async,
                self_mode: sig.self_mode,
                is_class_method: sig.is_class_method,
                name: sig.name,
                generic_params: sig.generic_params,
                params: sig.params,
                return_type: sig.return_type,
                span,
            })
        } else {
            // Case 4: Body with statements, terminated by `end`
            let body = self.parse_body();
            self.expect(TokenKind::End);
            let span = self.span_from(&start);
            MixinItem::DefaultMethod(FuncDef {
                visibility: sig.vis,
                is_async: sig.is_async,
                self_mode: sig.self_mode,
                is_class_method: sig.is_class_method,
                name: sig.name,
                generic_params: sig.generic_params,
                params: sig.params,
                return_type: sig.return_type,
                where_clause: None,
                body,
                doc_comments,
                span,
            })
        }
    }

    /// Parse a method signature (everything except the body).
    /// Returns the parsed signature components.
    fn parse_method_signature(&mut self, visibility: Visibility) -> ParsedMethodSig {
        let is_async = self.eat(TokenKind::Async);
        self.expect(TokenKind::Def);

        // Self mode: mut or consume
        let self_mode = if self.at(TokenKind::Mut) {
            let peek = self.peek_kind();
            match peek {
                TokenKind::Identifier(_) | TokenKind::Init | TokenKind::SelfValue => {
                    self.advance();
                    Some(SelfMode::Mutable)
                }
                _ => None,
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
            let name = self.expect_identifier();
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

    // ─── Impl Block ─────────────────────────────────────────────────

    fn parse_impl_block(&mut self, is_unsafe: bool) -> ImplBlock {
        let start = self.current_span();
        self.advance(); // consume impl
        let negative_trait = self.eat(TokenKind::Bang);

        let generic_params = if self.at(TokenKind::LBracket) {
            Some(self.parse_generic_params())
        } else {
            None
        };

        // Parse the first type/trait name
        let first_type = self.parse_type();
        self.skip_newlines();

        // Check for `for` — if present, this is a trait impl
        let (trait_name, target_type) = if self.eat(TokenKind::For) {
            self.skip_newlines();
            let target = self.parse_type();
            // Extract TypePath from first_type
            let trait_path = match first_type {
                TypeExpr::Named(path) => Some(path),
                _ => {
                    self.error("expected trait name before `for`");
                    None
                }
            };
            (trait_path, target)
        } else {
            (None, first_type)
        };
        self.skip_newlines();

        let mut items = Vec::new();
        while !self.at(TokenKind::End) && !self.at(TokenKind::Eof) {
            let __progress = self.pos;
            self.skip_newlines();
            if self.at(TokenKind::End) {
                break;
            }
            // Capture leading `##` doc comments so the next inner item
            // (impl method / assoc type) can pick them up.
            let docs = self.collect_doc_comments();
            if !docs.is_empty() {
                self.pending_doc_comments.extend(docs);
            }
            if self.at(TokenKind::End) {
                break;
            }
            items.push(self.parse_impl_item());
            self.skip_newlines();
            self.ensure_loop_progress(__progress);
        }
        self.expect(TokenKind::End);
        let span = self.span_from(&start);
        ImplBlock {
            generic_params,
            is_unsafe,
            negative_trait,
            trait_name,
            target_type,
            items,
            span,
        }
    }

    fn parse_impl_item(&mut self) -> ImplItem {
        let start = self.current_span();

        if self.at(TokenKind::Type) {
            // Associated type definition: type Name = Type
            self.advance();
            let name = self.expect_type_identifier();
            self.expect(TokenKind::Eq);
            self.skip_newlines();
            let type_expr = self.parse_type();
            let span = self.span_from(&start);
            return ImplItem::AssocType {
                name,
                type_expr,
                span,
            };
        }

        let vis = self.parse_visibility();
        let func = self.parse_func_def(vis);
        ImplItem::Method(func)
    }

    // ─── Class ───────────────────────────────────────────────────────

    fn parse_class_def(&mut self) -> ClassDef {
        let doc_comments = self.take_pending_docs();
        let start = self.current_span();
        self.advance(); // consume class
        let name = self.expect_type_identifier();

        let generic_params = if self.at(TokenKind::LBracket) {
            Some(self.parse_generic_params())
        } else {
            None
        };

        // Parent class: < TypeName
        let parent = if self.eat(TokenKind::Lt) {
            Some(self.parse_type_path())
        } else {
            None
        };
        // T2.02 S9: optional `where ...` clause between header and body.
        let where_clause = if self.at(TokenKind::Where) {
            Some(self.parse_where_clause())
        } else {
            None
        };
        self.skip_newlines();

        let mut fields = Vec::new();
        let mut methods = Vec::new();
        let mut inner_impls = Vec::new();
        let mut derive_traits = Vec::new();
        // ruby-naming.spec.md §3.2: `public` / `private` / `protected` are
        // SECTION MARKERS. Public by default; each bare marker switches the
        // visibility applied to all subsequent declarations until the next
        // marker or end of body. An explicit `pub` / `protected` prefix on a
        // declaration OVERRIDES the section state (the prefix wins).
        let mut current_vis = Visibility::Public;
        // `private :method_a, :method_b` re-marks — collected here and
        // applied as a final pass after the body is parsed.
        let mut name_list_overrides: Vec<(Visibility, Vec<String>)> = Vec::new();

        while !self.at(TokenKind::End) && !self.at(TokenKind::Eof) {
            let __progress = self.pos;
            self.skip_newlines();
            if self.at(TokenKind::End) {
                break;
            }
            // Capture leading `##` doc comments so the next class member
            // (method / field / inner impl) can pick them up.
            let docs = self.collect_doc_comments();
            if !docs.is_empty() {
                self.pending_doc_comments.extend(docs);
            }
            if self.at(TokenKind::End) {
                break;
            }

            match self.current_kind().clone() {
                // ── Section markers (ruby-naming.spec.md §3.2) ──
                // A bare `public` / `private` / `protected` line followed by
                // a terminator (newline/semicolon/end) flips `current_vis`.
                // The Ruby `private :a, :b` form (marker followed by `:ident`)
                // is captured as a name-list override and applied post-pass.
                TokenKind::Public => {
                    self.advance();
                    if self.at(TokenKind::Colon) {
                        // Ruby-style name list: `public :a, :b`
                        // (ruby-naming.spec.md §3.2)
                        let names = self.parse_visibility_name_list();
                        name_list_overrides.push((Visibility::Public, names));
                    } else {
                        current_vis = Visibility::Public;
                    }
                }
                TokenKind::Private => {
                    self.advance();
                    if self.at(TokenKind::Colon) {
                        let names = self.parse_visibility_name_list();
                        name_list_overrides.push((Visibility::Private, names));
                    } else {
                        current_vis = Visibility::Private;
                    }
                }
                TokenKind::Protected => {
                    // `protected` is also a section marker. Existing
                    // `parse_visibility` consumes `Protected` as an explicit
                    // prefix on a single decl; distinguish by lookahead.
                    let next = self.peek_kind();
                    let prefix_form = matches!(
                        next,
                        TokenKind::Def
                            | TokenKind::Async
                            | TokenKind::Identifier(_)
                            | TokenKind::Pub
                    );
                    if prefix_form {
                        // Treat as legacy explicit prefix on the next decl.
                        let vis = self.parse_visibility();
                        if matches!(self.current_kind(), TokenKind::Def | TokenKind::Async) {
                            methods.push(self.parse_func_def(vis));
                        } else {
                            fields.push(self.parse_field_decl_with_vis(vis));
                        }
                    } else {
                        self.advance();
                        if self.at(TokenKind::Colon) {
                            let names = self.parse_visibility_name_list();
                            name_list_overrides.push((Visibility::Protected, names));
                        } else {
                            current_vis = Visibility::Protected;
                        }
                    }
                }
                TokenKind::Impl => {
                    inner_impls.push(self.parse_inner_impl(false));
                }
                TokenKind::Include => {
                    // `include Mixin` directive — declares mixin participation
                    // without nesting methods. Required methods are provided
                    // by class methods at the same body level; default methods
                    // are synthesized in the resolver.
                    inner_impls.push(self.parse_include_directive(false));
                }
                TokenKind::Unsafe if matches!(self.peek_kind(), TokenKind::Impl) => {
                    self.advance(); // consume `unsafe`
                    inner_impls.push(self.parse_inner_impl(true));
                }
                TokenKind::Unsafe if matches!(self.peek_kind(), TokenKind::Include) => {
                    self.advance(); // consume `unsafe`
                    inner_impls.push(self.parse_include_directive(true));
                }
                TokenKind::Async => {
                    methods.push(self.parse_func_def(current_vis));
                }
                TokenKind::Def => {
                    methods.push(self.parse_func_def(current_vis));
                }
                TokenKind::Derive => {
                    self.advance();
                    derive_traits.push(self.expect_type_identifier());
                    while self.eat(TokenKind::Comma) {
                        self.skip_newlines();
                        derive_traits.push(self.expect_type_identifier());
                    }
                }
                TokenKind::Pub => {
                    // Explicit `pub` prefix wins over the section marker.
                    let vis = self.parse_visibility();
                    if matches!(self.current_kind(), TokenKind::Def | TokenKind::Async) {
                        methods.push(self.parse_func_def(vis));
                    } else {
                        // Could be a field with visibility
                        fields.push(self.parse_field_decl_with_vis(vis));
                    }
                }
                TokenKind::Identifier(_) => {
                    // Field declaration — picks up current section visibility.
                    fields.push(self.parse_field_decl_with_vis(current_vis));
                }
                _ => {
                    self.error(&format!(
                        "expected field, method, or impl in class body, found {:?}",
                        self.current_kind()
                    ));
                    // Hard-advance one token first; otherwise
                    // `synchronize()` is a no-op when the offending
                    // token is itself a sync point (Impl/Def/Class/...).
                    if !self.at(TokenKind::Eof) {
                        self.advance();
                    }
                    self.synchronize();
                }
            }
            self.skip_newlines();
            self.ensure_loop_progress(__progress);
        }
        self.expect(TokenKind::End);
        // Apply `private :a, :b` style name-list overrides as a final pass.
        // These RE-MARK already-collected methods, overriding any section
        // marker they were under (ruby-naming.spec.md §3.2 paragraph 3).
        for (vis, names) in &name_list_overrides {
            for n in names {
                if let Some(m) = methods.iter_mut().find(|m| &m.name == n) {
                    m.visibility = *vis;
                }
            }
        }
        let span = self.span_from(&start);
        ClassDef {
            name,
            generic_params,
            parent,
            fields,
            methods,
            inner_impls,
            derive_traits,
            doc_comments,
            where_clause,
            span,
        }
    }

    /// Parse the Ruby-style name list that follows a section marker:
    /// `private :method_a, :method_b`. The leading marker has already
    /// been consumed; the cursor sits on the first `:`.
    fn parse_visibility_name_list(&mut self) -> Vec<String> {
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

    fn parse_inner_impl(&mut self, is_unsafe: bool) -> InnerImpl {
        let start = self.current_span();
        self.advance(); // consume impl
        let negative_trait = self.eat(TokenKind::Bang);
        let trait_name = self.parse_type_path();
        self.skip_newlines();

        let mut items = Vec::new();
        while !self.at(TokenKind::End) && !self.at(TokenKind::Eof) {
            let __progress = self.pos;
            self.skip_newlines();
            if self.at(TokenKind::End) {
                break;
            }
            items.push(self.parse_impl_item());
            self.skip_newlines();
            self.ensure_loop_progress(__progress);
        }
        self.expect(TokenKind::End);
        let span = self.span_from(&start);
        InnerImpl {
            is_unsafe,
            negative_trait,
            trait_name,
            items,
            span,
        }
    }

    /// Parse an `include Mixin` directive inside a class body. Produces an
    /// InnerImpl with no items — the contract is satisfied by methods
    /// scattered at the class-body level (resolver/typecker enforce).
    fn parse_include_directive(&mut self, is_unsafe: bool) -> InnerImpl {
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

    fn parse_field_decl_with_vis(&mut self, visibility: Visibility) -> FieldDecl {
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

    // ─── Function Definition ─────────────────────────────────────────

    fn parse_func_def(&mut self, visibility: Visibility) -> FuncDef {
        let doc_comments = self.take_pending_docs();
        let start = self.current_span();
        let is_async = self.eat(TokenKind::Async);
        self.expect(TokenKind::Def);

        // Self mode: mut or consume
        let self_mode = if self.at(TokenKind::Mut) {
            // Check if this is `def mut name` (self mode) vs something else
            // It's a self mode if followed by an identifier or self.ident
            let peek = self.peek_kind();
            match peek {
                TokenKind::Identifier(_) | TokenKind::Init | TokenKind::SelfValue => {
                    self.advance();
                    Some(SelfMode::Mutable)
                }
                _ => None,
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
            let name = self.expect_identifier();
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

    fn parse_params(&mut self) -> Vec<Param> {
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
                        span,
                    };
                }
            }
        }

        let name = self.expect_identifier();
        self.expect(TokenKind::Colon);
        let type_expr = self.parse_type();
        let span = self.span_from(&start);
        Param {
            auto_assign,
            name,
            type_expr,
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
        let start = self.current_span();
        let mut statements = Vec::new();

        loop {
            self.skip_newlines();
            match self.current_kind() {
                TokenKind::End | TokenKind::Else | TokenKind::Elsif | TokenKind::Eof => break,
                _ => {
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

    pub(crate) fn parse_statement(&mut self) -> Statement {
        self.skip_newlines();

        match self.current_kind() {
            TokenKind::Let | TokenKind::Var => Statement::Let(self.parse_let_binding()),
            _ => Statement::Expression(self.parse_expression()),
        }
    }

    fn parse_let_binding(&mut self) -> LetBinding {
        let start = self.current_span();
        // `var x = ...` is the mutable form; `let x = ...` is immutable
        // (with `let mut x = ...` accepted only during migration).
        let var_form = matches!(self.current_kind(), TokenKind::Var);
        self.advance(); // consume let | var

        let mutable = var_form || self.eat(TokenKind::Mut);
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

    // ─── FFI Parsing ────────────────────────────────────────────────────

    /// Route `@[derive(...)]` / `@[repr(...)]` attributes onto a struct.
    /// Unknown attribute names emit a diagnostic.
    fn apply_struct_attrs(&mut self, s: &mut StructDef, attrs: &[Attribute]) {
        for attr in attrs {
            match attr.name.as_str() {
                "derive" => {
                    for arg in &attr.args {
                        s.derive_traits.push(arg.as_str().to_string());
                    }
                }
                "repr" => {
                    for arg in &attr.args {
                        s.repr.push(arg.as_str().to_string());
                    }
                }
                other => {
                    self.error(&format!("attribute `{}` is not valid on `struct`", other));
                }
            }
        }
    }

    /// Route `@[derive(...)]` attributes onto a class. `@[repr(...)]` is
    /// rejected — classes use the boxed GC layout and cannot opt into
    /// a C layout.
    fn apply_class_attrs(&mut self, c: &mut ClassDef, attrs: &[Attribute]) {
        for attr in attrs {
            match attr.name.as_str() {
                "derive" => {
                    for arg in &attr.args {
                        c.derive_traits.push(arg.as_str().to_string());
                    }
                }
                other => {
                    self.error(&format!("attribute `{}` is not valid on `class`", other));
                }
            }
        }
    }

    /// Route `@[derive(...)]` attributes onto an enum.
    fn apply_enum_attrs(&mut self, e: &mut EnumDef, attrs: &[Attribute]) {
        for attr in attrs {
            match attr.name.as_str() {
                "derive" => {
                    for arg in &attr.args {
                        e.derive_traits.push(arg.as_str().to_string());
                    }
                }
                other => {
                    self.error(&format!("attribute `{}` is not valid on `enum`", other));
                }
            }
        }
    }

    /// Parse `@[name(args)]` attributes.
    fn parse_attributes(&mut self) -> Vec<Attribute> {
        let mut attrs = Vec::new();
        while self.at(TokenKind::At) {
            let start = self.current_span();
            self.advance(); // consume @
            self.expect(TokenKind::LBracket);
            self.skip_newlines();

            // `derive` is a reserved keyword but is also a valid attribute
            // name (`@[derive(Copy, Clone)]`). Accept it here explicitly so
            // the tokenizer's keyword status doesn't block the attribute.
            let name = if self.at(TokenKind::Derive) {
                self.advance();
                "derive".to_string()
            } else {
                self.expect_any_identifier()
            };
            let mut args = Vec::new();

            if self.at(TokenKind::LParen) {
                self.advance(); // consume (
                self.skip_newlines();
                if !self.at(TokenKind::RParen) {
                    // Parse arguments as strings or identifiers
                    args.push(self.parse_attr_arg());
                    while self.eat(TokenKind::Comma) {
                        self.skip_newlines();
                        if self.at(TokenKind::RParen) {
                            break;
                        }
                        args.push(self.parse_attr_arg());
                    }
                }
                self.skip_newlines();
                self.expect(TokenKind::RParen);
            }

            self.skip_newlines();
            self.expect(TokenKind::RBracket);
            self.skip_newlines();

            let span = self.span_from(&start);
            attrs.push(Attribute { name, args, span });
        }
        attrs
    }

    // (apply_type_attrs removed during merge — superseded by per-kind helpers
    // apply_struct_attrs, apply_class_attrs, apply_enum_attrs from HEAD which
    // already exist on master and have richer per-kind validation including
    // E0607 error codes for misplaced @[derive].)

    /// Parse a single attribute argument (string literal or identifier).
    fn parse_attr_arg(&mut self) -> AttrArg {
        let start = self.current_span();
        match self.current_kind().clone() {
            TokenKind::StringLiteral(s) => {
                self.advance();
                AttrArg::Str(s, self.span_from(&start))
            }
            TokenKind::Identifier(s) => {
                self.advance();
                AttrArg::Ident(s, self.span_from(&start))
            }
            TokenKind::TypeIdentifier(s) => {
                self.advance();
                AttrArg::Ident(s, self.span_from(&start))
            }
            _ => {
                self.error("expected string or identifier in attribute argument");
                self.advance();
                AttrArg::Ident(String::new(), self.span_from(&start))
            }
        }
    }

    /// Parse `lib Name ... end`
    fn parse_lib_decl(&mut self, link_attrs: Vec<LinkAttr>) -> LibDecl {
        let start = self.current_span();
        self.advance(); // consume `lib`
        self.skip_newlines();

        // ruby-naming.spec.md §3.7: `lib "name" ... end` takes a string
        // literal link name. The legacy TypeIdentifier form
        // (`lib LibM ... end`) is retained for transitional sources.
        let name = match self.current_kind().clone() {
            TokenKind::StringLiteral(s) => {
                self.advance();
                s
            }
            TokenKind::TypeIdentifier(n) => {
                self.advance();
                n
            }
            _ => {
                self.error("expected lib name — a string literal (`lib \"c\"`) or TypeIdentifier");
                "_Error".to_string()
            }
        };

        // ruby-naming.spec.md §3.7: `lib "x", version: "3", path: "..."`
        // — options follow as keyword arguments. Accept and discard for
        // now; the typeck/codegen plumbing for these options lands in a
        // follow-up.
        while self.eat(TokenKind::Comma) {
            self.skip_newlines();
            if matches!(self.current_kind(), TokenKind::Identifier(_)) {
                self.advance(); // option name
                if self.eat(TokenKind::Colon) {
                    // option value — accept any literal
                    if matches!(
                        self.current_kind(),
                        TokenKind::StringLiteral(_)
                            | TokenKind::IntLiteral(_, _)
                            | TokenKind::True
                            | TokenKind::False
                    ) {
                        self.advance();
                    }
                }
            } else {
                break;
            }
        }

        self.skip_newlines();
        let mut functions = Vec::new();

        while !self.at(TokenKind::End) && !self.at(TokenKind::Eof) {
            let __progress = self.pos;
            self.skip_newlines();
            if self.at(TokenKind::End) {
                break;
            }
            if self.at(TokenKind::Def) {
                functions.push(self.parse_ffi_function());
            } else {
                self.error(&format!(
                    "expected `def` in lib block, found {:?}",
                    self.current_kind()
                ));
                self.advance();
            }
            self.expect_terminator();
            self.ensure_loop_progress(__progress);
        }

        self.expect(TokenKind::End);
        let span = self.span_from(&start);

        LibDecl {
            name,
            functions,
            link_attrs,
            span,
        }
    }

    /// Parse `extern "C" ... end`
    fn parse_extern_block(&mut self) -> ExternBlock {
        let start = self.current_span();
        self.advance(); // consume `extern`
        self.skip_newlines();

        // Parse the ABI string
        let abi = match self.current_kind().clone() {
            TokenKind::StringLiteral(s) => {
                self.advance();
                s
            }
            _ => {
                self.error("expected ABI string after `extern` (e.g., \"C\")");
                "C".to_string()
            }
        };

        self.skip_newlines();
        let mut functions = Vec::new();

        while !self.at(TokenKind::End) && !self.at(TokenKind::Eof) {
            let __progress = self.pos;
            self.skip_newlines();
            if self.at(TokenKind::End) {
                break;
            }
            if self.at(TokenKind::Def) {
                functions.push(self.parse_ffi_function());
            } else {
                self.error(&format!(
                    "expected `def` in extern block, found {:?}",
                    self.current_kind()
                ));
                self.advance();
            }
            self.expect_terminator();
            self.ensure_loop_progress(__progress);
        }

        self.expect(TokenKind::End);
        let span = self.span_from(&start);

        ExternBlock {
            abi,
            functions,
            span,
        }
    }

    /// Parse a single FFI function declaration: `def name(params) -> RetType`
    fn parse_ffi_function(&mut self) -> FfiFunction {
        let start = self.current_span();
        self.advance(); // consume `def`
        self.skip_newlines();

        let name = self.expect_any_identifier();

        let mut params = Vec::new();
        let mut is_variadic = false;

        if self.at(TokenKind::LParen) {
            self.advance(); // consume (
            self.skip_newlines();

            while !self.at(TokenKind::RParen) && !self.at(TokenKind::Eof) {
                // Check for variadic `...`
                if self.at(TokenKind::DotDot) {
                    self.advance(); // consume ..
                    if self.at(TokenKind::Dot) {
                        self.advance(); // consume the third .
                    }
                    is_variadic = true;
                    self.skip_newlines();
                    break;
                }

                let param_start = self.current_span();
                let param_name = self.expect_any_identifier();
                self.expect(TokenKind::Colon);
                self.skip_newlines();
                let param_type = self.parse_type();
                let param_span = self.span_from(&param_start);

                params.push(FfiParam {
                    name: param_name,
                    type_expr: param_type,
                    span: param_span,
                });

                if !self.eat(TokenKind::Comma) {
                    break;
                }
                self.skip_newlines();
            }

            self.skip_newlines();
            self.expect(TokenKind::RParen);
        }

        let return_type = if self.eat(TokenKind::Arrow) {
            self.skip_newlines();
            Some(self.parse_type())
        } else {
            None
        };

        let span = self.span_from(&start);
        FfiFunction {
            name,
            params,
            return_type,
            is_variadic,
            span,
        }
    }
}
