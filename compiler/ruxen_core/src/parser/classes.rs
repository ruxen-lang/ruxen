//! Parsing of user-defined types: enum, struct, mixin (`trait` keyword),
//! `extension` impl blocks, and class definitions — along with their inner
//! variant/field/method/include items.

use crate::lexer::token::TokenKind;
use crate::parser::ast::*;
use crate::parser::Parser;

impl Parser {
    // ─── Enum ────────────────────────────────────────────────────────

    pub(super) fn parse_enum_def(&mut self) -> EnumDef {
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
        let derive_traits = Vec::new();
        // #06.8 T0c: in-body `layout <kind>` directive. The only kind
        // currently accepted on an enum body is `tagged`, which pins
        // variant declaration order as the runtime tag assignment.
        let mut layout: Vec<String> = Vec::new();
        // ruby-naming.spec.md §3.2: section-marker visibility for inline
        // method declarations. Default public, mirrors `parse_class_def`.
        let mut current_vis = Visibility::Public;
        let mut name_list_overrides: Vec<(Visibility, Vec<String>)> = Vec::new();
        let mut aliases: Vec<AliasDef> = Vec::new();
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
                    let prefix_form = matches!(next, TokenKind::Def | TokenKind::Async);
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
                // ── In-body `include Trait` directives ──────────────
                TokenKind::Include => {
                    inner_impls.push(self.parse_include_directive(false));
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
                // ── #06.8 T0c: `layout tagged` directive ────────────
                // The only kind accepted on an enum body. Pins variant
                // declaration order as the runtime tag assignment;
                // resolver enforces uniqueness in scope and emits E0723
                // on a duplicate. Other kinds are a parser-level error.
                TokenKind::Layout => {
                    self.advance();
                    match self.current_kind().clone() {
                        TokenKind::Identifier(name) => {
                            self.advance();
                            if name == "tagged" {
                                layout.push("tagged".to_string());
                            } else {
                                self.error(&format!(
                                    "expected `tagged` after `layout` in enum body, found `{}`",
                                    name
                                ));
                            }
                        }
                        other => {
                            self.error(&format!(
                                "expected `tagged` after `layout` in enum body, found {:?}",
                                other
                            ));
                        }
                    }
                }
                // `alias new old` synonym — checked before the variant
                // default arm (variants are TypeIdentifiers; `alias` is a
                // lowercase Identifier, so this never shadows a variant).
                TokenKind::Identifier(_) if self.is_alias_item_start() => {
                    aliases.push(self.parse_alias_item());
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
            layout,
            doc_comments,
            where_clause,
            aliases,
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
            // ruby-naming.spec.md §3.10: the lexer maps `nil` →
            // `TokenKind::Nil`, which lowers to `Option::None` here.
            TokenKind::Nil => {
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

    pub(super) fn parse_struct_def(&mut self) -> StructDef {
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
        let derive_traits = Vec::new();
        // ruby-naming.spec.md §3.5: `layout c` / `layout packed` /
        // `layout transparent` directives populate this list (replaces
        // the retired `@[repr(...)]` prefix attribute).
        let mut layout: Vec<String> = Vec::new();
        // ruby-naming.spec.md §3.2: section-marker visibility, public default.
        let mut current_vis = Visibility::Public;
        let mut name_list_overrides: Vec<(Visibility, Vec<String>)> = Vec::new();
        let mut aliases: Vec<AliasDef> = Vec::new();

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
                        TokenKind::Def | TokenKind::Async | TokenKind::Identifier(_)
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
                // ── Method definitions (post Ruby-naming migration) ──
                TokenKind::Def | TokenKind::Async => {
                    methods.push(self.parse_func_def(current_vis));
                }
                // ── In-body `include Trait` directives ──────────────
                TokenKind::Include => {
                    inner_impls.push(self.parse_include_directive(false));
                }
                TokenKind::Unsafe if matches!(self.peek_kind(), TokenKind::Include) => {
                    self.advance();
                    inner_impls.push(self.parse_include_directive(true));
                }
                // ── `layout <c|packed|transparent>` directive (§3.5) ──
                // Replaces the retired `@[repr(...)]` prefix attribute.
                TokenKind::Layout => {
                    self.advance();
                    match self.current_kind().clone() {
                        TokenKind::Identifier(name) => {
                            self.advance();
                            let token = if name.eq_ignore_ascii_case("c") {
                                "C".to_string()
                            } else {
                                name.to_string()
                            };
                            layout.push(token);
                        }
                        other => {
                            self.error(&format!(
                                "expected layout kind (`c` / `packed` / `transparent`) after `layout`, found {:?}",
                                other
                            ));
                        }
                    }
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
                TokenKind::Identifier(_) if self.is_alias_item_start() => {
                    aliases.push(self.parse_alias_item());
                }
                TokenKind::Identifier(_) => {
                    fields.push(self.parse_field_decl_with_vis(current_vis));
                }
                _ => {
                    self.error(&format!(
                        "expected field, method, or include directive in struct body, found {:?}",
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
            layout,
            doc_comments,
            where_clause,
            aliases,
            span,
        }
    }

    // ─── Trait ───────────────────────────────────────────────────────

    pub(super) fn parse_trait_def(&mut self) -> MixinDef {
        let doc_comments = self.take_pending_docs();
        let start = self.current_span();
        self.advance(); // consume trait
        let name = self.expect_type_identifier();

        let generic_params = if self.at(TokenKind::LBracket) {
            Some(self.parse_generic_params())
        } else {
            None
        };

        // Spec — `docs/specs/types/mixin_vtables.spec.md` §B1.
        // Optional `dispatch runtime` modifier. `dispatch` and `runtime`
        // are NOT keywords — they lex as plain `Identifier`s. Recognise
        // them contextually here: after the mixin's name + optional
        // generics, before the (optional) `:` bounds list. The only legal
        // tokens at this position otherwise are `Colon` / newline / `End`,
        // so peeking for the identifier pair is unambiguous.
        let dispatch_mode = if matches!(self.current_kind(), TokenKind::Identifier(n) if n == "dispatch")
            && matches!(self.peek_kind(), TokenKind::Identifier(ref m) if m == "runtime")
        {
            self.advance(); // consume `dispatch`
            self.advance(); // consume `runtime`
            DispatchMode::Runtime
        } else {
            DispatchMode::Static
        };

        let super_traits = if self.eat(TokenKind::Colon) {
            self.parse_trait_bounds()
        } else {
            vec![]
        };
        self.skip_newlines();

        let mut items = Vec::new();
        // #06.8 follow-up: in-body `lib "X" ... end` blocks in mixin bodies
        // mirror the class-body form. Parser-only plumbing.
        let mut lib_decls: Vec<LibDecl> = Vec::new();
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
                    // Bare-marker section form vs. prefix form on a `def`.
                    let next = self.peek_kind();
                    let prefix_form = matches!(next, TokenKind::Def | TokenKind::Async);
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
                // ── #06.8 follow-up: in-body `lib "X" ... end` block ──
                // Mirrors the class-body arm. Reuses `parse_lib_decl`.
                TokenKind::Lib => {
                    lib_decls.push(self.parse_lib_decl(vec![]));
                }
                // `alias new old` synonym inside a mixin body. Checked
                // before the catch-all `parse_trait_item` so `alias` is not
                // misread as a method-sig name.
                TokenKind::Identifier(_) if self.is_alias_item_start() => {
                    items.push(MixinItem::Alias(self.parse_alias_item()));
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
            lib_decls,
            doc_comments,
            dispatch_mode,
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
            self.error("expected `def` or `type` in mixin body");
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
                | TokenKind::Public
                | TokenKind::Private
                | TokenKind::Protected
                | TokenKind::Type
                | TokenKind::End
                | TokenKind::Eof
                // #06.8 follow-up: a `lib "X" ... end` block following an
                // unbodied method signature inside a mixin body must
                // close the signature, not be consumed as its body.
                | TokenKind::Lib
                // Q10 (gui-stack-v1-issues): a `##` doc comment after a
                // bodiless signature belongs to the NEXT item, so it
                // terminates this signature too. Left unconsumed, the outer
                // `parse_trait_def` loop's `collect_doc_comments()` floats
                // it forward to the following `def`. Without this, the doc
                // comment fell into the "default method body" branch below
                // and the parser choked on the next `def`.
                | TokenKind::DocComment(_)
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

    // ─── Impl Block ─────────────────────────────────────────────────

    pub(super) fn parse_impl_block(&mut self, is_unsafe: bool) -> ImplBlock {
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
                    self.error("expected mixin name before `for`");
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

        // ruby-naming.spec.md §3.4a: `extension` bodies may carry
        // `include Mixin` directives. Accept the bare `include` form
        // and the `unsafe include` modifier here.
        if self.at(TokenKind::Include)
            || (self.at(TokenKind::Unsafe) && self.peek_kind() == TokenKind::Include)
        {
            let is_unsafe = if self.at(TokenKind::Unsafe) {
                self.advance();
                true
            } else {
                false
            };
            self.advance(); // consume `include`
            let negative_trait = self.eat(TokenKind::Bang);
            let trait_name = self.parse_type_path();
            let span = self.span_from(&start);
            return ImplItem::Include {
                is_unsafe,
                negative_trait,
                trait_name,
                span,
            };
        }

        // `alias new old` inside an `extension` body — checked before the
        // method parse (docs/decisions/alias-keyword.md).
        if self.is_alias_item_start() {
            return ImplItem::Alias(self.parse_alias_item());
        }

        let vis = self.parse_visibility();
        let func = self.parse_func_def(vis);
        ImplItem::Method(func)
    }

    // ─── Class ───────────────────────────────────────────────────────

    pub(super) fn parse_class_def(&mut self) -> ClassDef {
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
        let derive_traits = Vec::new();
        // #06.8 follow-up: in-body `lib "X" ... end` FFI blocks. Each
        // block's `FfiFunction`s are scoped to this class (e.g. `File.open`).
        // Reuses `parse_lib_decl` verbatim — no fork.
        let mut lib_decls: Vec<LibDecl> = Vec::new();
        // #06.8 T0c: in-body `layout <kind>` directive. The only kind
        // currently accepted on a class body is `flat_heap_struct`,
        // which marks the class as following the runtime flat-heap-
        // struct C layout (`RuxenFile`, `RuxenTcpStream` pattern). The
        // link-time layout-mismatch check (E0724) is reserved but not
        // yet emitted; this is the Wave 1 marker plumbing only.
        let mut layout: Vec<String> = Vec::new();
        // ruby-naming.spec.md §3.2: `public` / `private` / `protected` are
        // SECTION MARKERS. Public by default; each bare marker switches the
        // visibility applied to all subsequent declarations until the next
        // marker or end of body. An explicit `pub` / `protected` prefix on a
        // declaration OVERRIDES the section state (the prefix wins).
        let mut current_vis = Visibility::Public;
        // `private :method_a, :method_b` re-marks — collected here and
        // applied as a final pass after the body is parsed.
        let mut name_list_overrides: Vec<(Visibility, Vec<String>)> = Vec::new();
        // Ruby `alias new old` synonyms (docs/decisions/alias-keyword.md).
        let mut aliases: Vec<AliasDef> = Vec::new();

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
                    // `protected` is also a section marker. `parse_visibility`
                    // consumes `Protected` as an explicit prefix on a single
                    // decl; distinguish by lookahead.
                    let next = self.peek_kind();
                    let prefix_form = matches!(
                        next,
                        TokenKind::Def | TokenKind::Async | TokenKind::Identifier(_)
                    );
                    if prefix_form {
                        // Treat as explicit prefix on the next decl.
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
                TokenKind::Include => {
                    // `include Mixin` directive — declares mixin participation
                    // without nesting methods. Required methods are provided
                    // by class methods at the same body level; default methods
                    // are synthesized in the resolver.
                    inner_impls.push(self.parse_include_directive(false));
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
                // ── #06.8 follow-up: in-body `lib "X" ... end` block ──
                // Stdlib self-hosting needs class-scoped FFI bindings so
                // e.g. `library/std/io/src/lib.rx` can write
                // `class File ... lib "ruxen_runtime" def open as "..." end end`.
                // The block reuses the top-level `parse_lib_decl`; the
                // resolver wiring that surfaces each `FfiFunction` as
                // `<ClassName>.<ruxen_name>` lands in the follow-up
                // commit, not here.
                TokenKind::Lib => {
                    lib_decls.push(self.parse_lib_decl(vec![]));
                }
                // `alias new old` synonym — must be checked before the field
                // arm, since `alias` lexes as an Identifier. A real field
                // named `alias` is `alias: Type` (colon), so `is_alias_item_start`
                // (identifier/operator after `alias`, no colon) disambiguates.
                TokenKind::Identifier(_) if self.is_alias_item_start() => {
                    aliases.push(self.parse_alias_item());
                }
                TokenKind::Identifier(_) => {
                    // Field declaration — picks up current section visibility.
                    fields.push(self.parse_field_decl_with_vis(current_vis));
                }
                // ── `layout <kind>` directive (class-level) ──
                //
                // Two kinds today:
                //
                //   * `layout flat_heap_struct` — #06.8 T0c. Marks the
                //     class as a flat heap struct (matches the
                //     `RuxenFile` / `RuxenTcpStream` runtime pattern).
                //     Wave 1 parser+resolver marker only; the link-time
                //     layout-mismatch check (E0724) is reserved but
                //     unwired.
                //
                //   * `layout c` — task #27 (extends `layout c` from
                //     struct bodies, per ffi.spec.md §B8, to class
                //     bodies). Same C-compatible-layout guarantee:
                //     fields in declaration order, native alignment,
                //     no reordering. Once codegen is wired, instances
                //     of such classes are emitted as flat repr-C structs
                //     (NOT opaque heap handles), so Ruxen method bodies
                //     can read `self.<field>` directly from the same
                //     wire layout C functions manipulate. Unlocks #19
                //     (porting trivial C accessors to inline Ruxen).
                //
                // `packed` and `transparent` aren't (yet) meaningful on
                // class bodies — defer until a class needs them.
                TokenKind::Layout => {
                    self.advance();
                    match self.current_kind().clone() {
                        TokenKind::Identifier(name) => {
                            self.advance();
                            if name == "flat_heap_struct" {
                                layout.push("flat_heap_struct".to_string());
                            } else if name.eq_ignore_ascii_case("c") {
                                layout.push("C".to_string());
                            } else {
                                self.error(&format!(
                                    "expected `flat_heap_struct` or `c` after `layout` in class body, found `{}`",
                                    name
                                ));
                            }
                        }
                        other => {
                            self.error(&format!(
                                "expected `flat_heap_struct` or `c` after `layout` in class body, found {:?}",
                                other
                            ));
                        }
                    }
                }
                TokenKind::Type => {
                    // ruby-naming.spec.md §3.4: a class that includes a
                    // mixin with `type Item` may bind it via
                    // `type Item = Concrete`. v1 parses the binding and
                    // discards it — the class is also expected to
                    // declare its concrete-return method directly, which
                    // is what carries the type through codegen.
                    self.advance();
                    let _ = self.expect_type_identifier();
                    self.expect(TokenKind::Eq);
                    self.skip_newlines();
                    let _ = self.parse_type();
                }
                _ => {
                    self.error(&format!(
                        "expected field, method, or include directive in class body, found {:?}",
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
            layout,
            lib_decls,
            doc_comments,
            where_clause,
            aliases,
            span,
        }
    }
}
