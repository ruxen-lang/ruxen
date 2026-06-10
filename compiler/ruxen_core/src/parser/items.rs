//! Top-level item parsing dispatcher and parsers for non-class items:
//! module, use, type alias, newtype, and const declarations.
//!
//! The actual `class` / `struct` / `enum` / `mixin` / `extension` parsers
//! live in `parser/classes.rs`; this module dispatches into them.

use crate::lexer::token::TokenKind;
use crate::parser::ast::*;
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
            // `alias new old` at top level (contextual keyword — `alias`
            // lexes as an Identifier; it only starts an alias item when an
            // identifier follows). docs/decisions/alias-keyword.md.
            TokenKind::Identifier(_) if self.is_alias_item_start() => {
                Some(TopLevelItem::Alias(self.parse_alias_item()))
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
                // A top-level EXPRESSION STATEMENT — e.g. a call with a
                // trailing `do…end` at module scope (`Tester.describe("…")
                // do … end`, the shape of every `tests/*.rx`). The compile
                // path never reaches here for those files (`ruxen test`
                // hoists items + wraps statements in a synthesised `def
                // main` first); resolve rejects a `TopLevelItem::Expr` with
                // E0608 if a raw file is compiled directly. Accepting it
                // here lets the SHARED parser (and therefore `ruxen fmt`)
                // round-trip test files instead of erroring at 1:1. (Q23b.)
                if self.starts_top_level_expr_stmt() {
                    let before_pos = self.pos;
                    let before_diags = self.diagnostics.len();
                    let expr = self.parse_expression();
                    let consumed = self.pos > before_pos;
                    let clean = self.diagnostics.len() == before_diags;
                    let at_boundary = matches!(
                        self.current_kind(),
                        TokenKind::Newline | TokenKind::Eof | TokenKind::End
                    );
                    // Accept only a fully-clean expression that lands on a
                    // statement boundary. Anything else falls through to the
                    // standard error path so a real top-level-declaration typo
                    // still gets the clear "expected top-level declaration"
                    // diagnostic instead of being silently swallowed.
                    if consumed && clean && at_boundary {
                        return Some(TopLevelItem::Expr(expr));
                    }
                    // Reset cursor + drop any diagnostics the speculative
                    // parse recorded, then fall through.
                    self.pos = before_pos;
                    self.diagnostics.truncate(before_diags);
                }
                self.error(&format!(
                    "expected top-level declaration, found {:?}",
                    self.current_kind()
                ));
                None
            }
        }
    }

    /// Heuristic: does the cursor begin a plausible top-level expression
    /// statement (an identifier / type-identifier / `self` that starts a
    /// call or method-call chain)? Deliberately narrow — we only want to
    /// accept the `Recv.method(args) do…end` test-file shape, not arbitrary
    /// expressions that would mask real top-level-declaration typos.
    fn starts_top_level_expr_stmt(&self) -> bool {
        matches!(
            self.current_kind(),
            TokenKind::Identifier(_) | TokenKind::TypeIdentifier(_) | TokenKind::SelfValue
        )
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
                // An operator-spelled new name (`alias << push`) is parsed
                // too, so resolve can emit the staged E1123 rather than the
                // parser tripping a generic error (ADR D6).
                || Self::is_operator_name_start(&self.peek_kind()))
    }

    /// Parse `alias new_name old_name`. The cursor sits on the `alias`
    /// contextual keyword. Both names are bare identifiers (incl. `?`/`!`
    /// names, which the lexer absorbs into the identifier). The comma form
    /// `alias new, old` is rejected (D1) — a comma after the new name is a
    /// clean parse error here.
    pub(super) fn parse_alias_item(&mut self) -> AliasDef {
        let start = self.current_span();
        self.advance(); // consume the `alias` contextual keyword
                        // `parse_def_name` accepts both plain/`?`/`!` identifiers and
                        // operator-symbol names (`<<`, `[]`); operator names are caught at
                        // resolve with E1123 (ADR D6), so they parse cleanly here.
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

    pub(super) fn parse_visibility(&mut self) -> Visibility {
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
