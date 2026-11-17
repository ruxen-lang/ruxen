//! Top-level item parsing dispatcher and parsers for non-class items:
//! module, use, type alias, newtype, and const declarations.
//!
//! The actual `class` / `struct` / `enum` / `mixin` / `extension` parsers
//! live in `parser/classes.rs`; this module dispatches into them.

use crate::diagnostics::Diagnostic;
use crate::lexer::token::{Span, TokenKind};
use crate::parser::ast::*;
use crate::parser::methods::BodyState;
use crate::parser::Parser;

impl Parser {
    pub(super) fn parse_top_level_item(&mut self) -> Option<TopLevelItem> {
        self.skip_newlines();
        // Capture any leading `##` doc comments so the next item can pick
        // them up via `take_pending_docs()`.
        let docs = self.collect_doc_comments();
        if !docs.is_empty() {
            self.pending_doc_comments.extend(docs);
        }

        // A file may end with trailing doc comments, or consist ENTIRELY of
        // module-level `##` documentation (e.g. a doc-only stdlib surface
        // file such as `library/std/string/src/lib.rx`). Once those docs are
        // consumed there is no item left to build — stop cleanly instead of
        // erroring `expected top-level declaration, found Eof`. This keeps
        // the parser (shared by the compiler, IDE/LSP, and formatter) from
        // rejecting files the toolchain otherwise treats as valid.
        if self.at(TokenKind::Eof) {
            return None;
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
            TokenKind::Lib => Some(TopLevelItem::Lib(self.parse_lib_decl())),
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
            // `alias new old` at top level (contextual keyword — `alias`
            // lexes as an Identifier; it only starts an alias item when an
            // identifier follows). docs/decisions/alias-keyword.md.
            TokenKind::Identifier(_) if self.is_alias_item_start() => {
                Some(TopLevelItem::Alias(self.parse_alias_item()))
            }
            TokenKind::Protected => {
                let vis = self.parse_visibility().unwrap_or(Visibility::Private);
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
                // A top-level EXPRESSION STATEMENT: any statement Ruby runs
                // at the top of a file, such as a call with a trailing
                // `do…end` (`Tester.describe("…") do … end`, the shape of
                // every `tests/*.rx`), an assignment, `while` / `for` / `if` /
                // `match`, or a literal receiver (`[1, 2].each { … }`). The
                // compile pipeline moves clean expressions into an implicit
                // `main`; the shared parser retains the raw item so
                // `ruxen fmt` can round-trip source faithfully.
                let before_pos = self.pos;
                let before_diags = self.diagnostics.len();
                let expr = self.parse_expression();
                let consumed = self.pos > before_pos;
                let clean = self.diagnostics.len() == before_diags;
                let at_boundary = matches!(
                    self.current_kind(),
                    TokenKind::Newline | TokenKind::Eof | TokenKind::End
                );
                // Accept a fully-clean expression that lands on a statement
                // boundary.
                if consumed && clean && at_boundary {
                    return Some(TopLevelItem::Expr(expr));
                }
                // A statement that parsed past its first token but hit an
                // error inside (`f = ->(x: Int) { |y| y }`) keeps that
                // error: it says what is wrong with the statement.
                if consumed && !clean {
                    return None;
                }
                // Anything else (a declaration typo, a stray token) gets the
                // "expected top-level declaration" diagnostic below; drop
                // what the speculative parse recorded.
                self.pos = before_pos;
                self.diagnostics.truncate(before_diags);
                self.error(&format!(
                    "expected top-level declaration, found {:?}",
                    self.current_kind()
                ));
                None
            }
        }
    }

    /// Whether the cursor begins a top-level declaration item (`def`,
    /// `class`, `use`, `lib`, `protected def`, `unsafe extension`,
    /// `alias new old`, ...) rather than an expression statement: the
    /// declaration arms of [`Self::parse_top_level_item`]. The REPL and the
    /// test runner use it to tell items from statements.
    pub fn is_item_start(&self) -> bool {
        match self.current_kind() {
            TokenKind::Module
            | TokenKind::Class
            | TokenKind::Struct
            | TokenKind::Enum
            | TokenKind::Mixin
            | TokenKind::Extension
            | TokenKind::Use
            | TokenKind::Type
            | TokenKind::Newtype
            | TokenKind::Const
            | TokenKind::Lib
            | TokenKind::Async
            | TokenKind::Def
            | TokenKind::Protected => true,
            TokenKind::Unsafe => matches!(self.peek_kind(), TokenKind::Extension),
            TokenKind::Identifier(_) => self.is_alias_item_start(),
            _ => false,
        }
    }

    /// Split a script-style file (a test file: declarations mixed with
    /// statements that run in `main`) into its top-level units, in source
    /// order. Each unit's span starts at its first token, after any `##`
    /// docs, and ends at its last token.
    pub fn parse_script_units(&mut self) -> Result<Vec<ScriptUnit>, Vec<Diagnostic>> {
        let mut units = Vec::new();
        loop {
            self.skip_terminators();
            self.collect_doc_comments();
            self.pending_doc_comments.clear();
            if self.at_eof() {
                break;
            }
            let start = self.current_span();
            let before = self.pos;
            let item = if self.is_item_start() {
                self.parse_top_level_item()
            } else {
                self.parse_statement();
                None
            };
            if self.pos == before {
                // Nothing parsed; the error is already recorded.
                break;
            }
            let end = self.tokens[self.pos - 1].span.end;
            units.push(ScriptUnit {
                item,
                span: Span::new(start.start, end, start.line, start.column),
            });
        }
        if self.diagnostics.is_empty() {
            Ok(units)
        } else {
            Err(self.diagnostics.clone())
        }
    }

    /// True if the cursor begins a Ruby `alias new_name old_name` item:
    /// the contextual keyword `alias` (an Identifier) followed immediately
    /// by another identifier (the new name) — and crucially NOT by a colon,
    /// which would make `alias` an ordinary field/binding name
    /// (`alias: String`). The disambiguation is sound because a field decl
    /// is always `name: Type`. docs/decisions/alias-keyword.md (D1).
    pub(super) fn is_alias_item_start(&self) -> bool {
        matches!(self.current_kind(), TokenKind::Identifier(s) if s == "alias")
            && (matches!(self.peek_kind(), TokenKind::Identifier(_))
                // Operator-spelled new names (`alias << push`, `alias [] get`)
                // are parsed too; resolve admits operators with expression
                // dispatch paths and emits E1123 for the remaining staged
                // forms (ADR D6).
                || Self::is_operator_name_start(&self.peek_kind()))
    }

    /// Parse `alias new_name old_name`. The cursor sits on the `alias`
    /// contextual keyword. Both names are bare identifiers (incl. `?`/`!`
    /// names, which the lexer absorbs into the identifier). The comma form
    /// `alias new, old` is rejected (D1) — a comma after the new name is a
    /// clean parse error here.
    pub(super) fn parse_alias_item(&mut self) -> AliasDef {
        // `##` docs above an alias document the alias; `AliasDef` keeps no
        // docs, and left pending they would attach to the next `def`.
        self.take_pending_docs();
        let start = self.current_span();
        self.advance(); // consume the `alias` contextual keyword
                        // `parse_def_name` accepts both plain/`?`/`!` identifiers and
                        // operator-symbol names (`<<`, `[]`); resolve admits supported
                        // operator dispatch paths and rejects the rest with E1123 (ADR D6).
        let new_name = self.parse_def_name();
        // Ruby's `alias` takes NO comma. Reject the comma form explicitly so
        // the diagnostic is actionable instead of a generic "expected
        // identifier" further along.
        if self.at(TokenKind::Comma) {
            self.error(
                "`alias` uses the space form `alias new_name old_name` (no comma); \
                 the comma form is not supported",
            );
            self.eat(TokenKind::Comma);
        }
        let old_name = self.parse_def_name();
        let span = self.span_from(&start);
        AliasDef {
            new_name,
            old_name,
            span,
        }
    }

    /// The explicit `protected` prefix on a `def`, or `None` when the
    /// declaration has no prefix and takes its context's visibility.
    pub(super) fn parse_visibility(&mut self) -> Option<Visibility> {
        match self.current_kind() {
            TokenKind::Protected => {
                self.advance();
                Some(Visibility::Protected)
            }
            _ => None,
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
        // Only module functions take the section's visibility; nested types
        // (Class/Struct/Module/...) carry their own visibility model.
        let mut state = BodyState::new();
        loop {
            let progress = self.pos;
            if !self.at_body_member() {
                break;
            }
            // Section markers and unprefixed `def`s first:
            // `parse_top_level_item` knows neither.
            match self.current_kind().clone() {
                TokenKind::Public => self.parse_section_marker(Visibility::Public, &mut state),
                TokenKind::Private => self.parse_section_marker(Visibility::Private, &mut state),
                // `protected def` falls to `parse_top_level_item`.
                TokenKind::Protected
                    if !matches!(self.peek_kind(), TokenKind::Def | TokenKind::Async) =>
                {
                    self.parse_section_marker(Visibility::Protected, &mut state)
                }
                TokenKind::Def | TokenKind::Async => {
                    items.push(TopLevelItem::Function(
                        self.parse_func_def(state.current_vis),
                    ));
                }
                _ => {
                    if let Some(item) = self.parse_top_level_item() {
                        items.push(item);
                    } else {
                        self.synchronize();
                    }
                }
            }
            self.skip_newlines();
            self.ensure_loop_progress(progress);
        }
        self.expect(TokenKind::End);
        // Name lists re-mark module functions.
        state.apply_name_lists(items.iter_mut().filter_map(|item| match item {
            TopLevelItem::Function(f) => Some(f),
            _ => None,
        }));
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
