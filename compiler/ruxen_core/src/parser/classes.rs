//! Parsing of user-defined types: enum, struct, mixin (`trait` keyword),
//! `extension` impl blocks, and class definitions — along with their inner
//! variant/field/method/include items.

use crate::lexer::token::TokenKind;
use crate::parser::ast::*;
use crate::parser::methods::{BodyState, TypeMembers};
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
        let mut members = TypeMembers::default();
        // #06.8 T0c: in-body `layout <kind>` directive. The only kind
        // currently accepted on an enum body is `tagged`, which pins
        // variant declaration order as the runtime tag assignment.
        let mut layout: Vec<String> = Vec::new();
        let mut state = BodyState::new();
        loop {
            let progress = self.pos;
            if !self.at_body_member() {
                break;
            }
            if !self.parse_type_body_member("enum", false, &mut state, &mut members) {
                match self.current_kind().clone() {
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
                    // ── Default: variant declaration ────────────────────
                    _ => {
                        variants.push(self.parse_variant());
                    }
                }
            }
            self.skip_newlines();
            self.ensure_loop_progress(progress);
        }
        self.expect(TokenKind::End);
        state.apply_name_lists(members.methods.iter_mut());
        let TypeMembers {
            methods,
            inner_impls,
            aliases,
        } = members;
        let span = self.span_from(&start);
        EnumDef {
            name,
            generic_params,
            variants,
            methods,
            inner_impls,
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
        let mut members = TypeMembers::default();
        // ruby-naming.spec.md §3.5: `layout c` / `layout packed` /
        // `layout transparent` directives populate this list (replaces
        // the retired `@[repr(...)]` prefix attribute).
        let mut layout: Vec<String> = Vec::new();
        let mut state = BodyState::new();
        loop {
            let progress = self.pos;
            if !self.at_body_member() {
                break;
            }
            if !self.parse_type_body_member("struct", true, &mut state, &mut members) {
                match self.current_kind().clone() {
                    // `protected name: Type`: an explicit field visibility.
                    TokenKind::Protected => {
                        self.advance();
                        fields.push(self.parse_field_decl_with_vis(Visibility::Protected));
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
                    TokenKind::Identifier(_) => {
                        fields.push(self.parse_field_decl_with_vis(state.current_vis));
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
            }
            self.skip_newlines();
            self.ensure_loop_progress(progress);
        }
        self.expect(TokenKind::End);
        state.apply_name_lists(members.methods.iter_mut());
        let TypeMembers {
            methods,
            inner_impls,
            aliases,
        } = members;
        let span = self.span_from(&start);
        StructDef {
            name,
            generic_params,
            fields,
            methods,
            inner_impls,
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
        // section markers. Default is Public. The trait-item parser gives an
        // unprefixed default method the current section's visibility.
        // MethodSig items in traits are signatures only; for mixins (no
        // `visibility` field on MethodSig) the resolver synthesizes
        // visibility from the section as needed.
        let mut state = BodyState::new();
        loop {
            let progress = self.pos;
            if !self.at_body_member() {
                break;
            }
            match self.current_kind().clone() {
                TokenKind::Public => self.parse_section_marker(Visibility::Public, &mut state),
                TokenKind::Private => self.parse_section_marker(Visibility::Private, &mut state),
                // `protected def` is a prefix; a bare `protected` a section.
                TokenKind::Protected
                    if matches!(self.peek_kind(), TokenKind::Def | TokenKind::Async) =>
                {
                    items.push(self.parse_trait_item(state.current_vis));
                }
                TokenKind::Protected => {
                    self.parse_section_marker(Visibility::Protected, &mut state)
                }
                // ── #06.8 follow-up: in-body `lib "X" ... end` block ──
                // Mirrors the class-body arm. Reuses `parse_lib_decl`.
                TokenKind::Lib => {
                    lib_decls.push(self.parse_lib_decl());
                }
                // `alias new old` synonym inside a mixin body. Checked
                // before the catch-all `parse_trait_item` so `alias` is not
                // misread as a method-sig name.
                TokenKind::Identifier(_) if self.is_alias_item_start() => {
                    items.push(MixinItem::Alias(self.parse_alias_item()));
                }
                _ => {
                    items.push(self.parse_trait_item(state.current_vis));
                }
            }
            self.skip_newlines();
            self.ensure_loop_progress(progress);
        }
        self.expect(TokenKind::End);
        // Name lists re-mark default methods.
        state.apply_name_lists(items.iter_mut().filter_map(|item| match item {
            MixinItem::DefaultMethod(func) => Some(func),
            MixinItem::AssocType { .. } | MixinItem::MethodSig(_) | MixinItem::Alias(_) => None,
        }));
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

    fn parse_trait_item(&mut self, section_vis: Visibility) -> MixinItem {
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

        // Method signature or default method, with an optional `protected`
        // prefix; unprefixed methods take the section's visibility.
        let vis = self.parse_visibility().unwrap_or(section_vis);
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
        loop {
            let progress = self.pos;
            if !self.at_body_member() {
                break;
            }
            items.push(self.parse_impl_item());
            self.skip_newlines();
            self.ensure_loop_progress(progress);
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

        let vis = self.parse_visibility().unwrap_or(Visibility::Private);
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
        let mut members = TypeMembers::default();
        // #06.8 follow-up: in-body `lib "X" ... end` FFI blocks. Each
        // block's `FfiFunction`s are scoped to this class (e.g. `File.open`).
        // Reuses `parse_lib_decl` verbatim — no fork.
        let mut lib_decls: Vec<LibDecl> = Vec::new();
        // #06.8 T0c: in-body `layout <kind>` directive (see the arm below).
        let mut layout: Vec<String> = Vec::new();
        let mut assoc_types: Vec<AssocTypeBinding> = Vec::new();
        // ruby-naming.spec.md §3.2: `public` / `private` / `protected` are
        // SECTION MARKERS; an explicit `protected` prefix on a declaration
        // overrides the section, and `private :a, :b` re-marks after the body.
        let mut state = BodyState::new();
        loop {
            let progress = self.pos;
            if !self.at_body_member() {
                break;
            }
            if !self.parse_type_body_member("class", true, &mut state, &mut members) {
                match self.current_kind().clone() {
                    // `protected name: Type`: an explicit field visibility.
                    TokenKind::Protected => {
                        self.advance();
                        fields.push(self.parse_field_decl_with_vis(Visibility::Protected));
                    }
                    // ── #06.8 follow-up: in-body `lib "X" ... end` block ──
                    // Class-scoped FFI bindings for stdlib self-hosting
                    // (`class File ... lib "ruxen_runtime" def open as "..."
                    // end end`), reusing the top-level `parse_lib_decl`.
                    TokenKind::Lib => {
                        lib_decls.push(self.parse_lib_decl());
                    }
                    TokenKind::Identifier(_) => {
                        // Field declaration — picks up current section visibility.
                        fields.push(self.parse_field_decl_with_vis(state.current_vis));
                    }
                    // ── `layout <kind>` directive (class-level) ──
                    //
                    //   * `layout flat_heap_struct` — #06.8 T0c. Marks the
                    //     class as a flat heap struct (the `RuxenFile` /
                    //     `RuxenTcpStream` runtime pattern). The link-time
                    //     layout-mismatch check (E0724) is reserved but
                    //     unwired.
                    //   * `layout c` — task #27 (ffi.spec.md §B8 extended to
                    //     class bodies): fields in declaration order, native
                    //     alignment, no reordering, so C and Ruxen method
                    //     bodies share one wire layout.
                    //   * `layout closure` — the class's values are closure
                    //     pairs: it is the method home of every stored
                    //     closure value (`class Closure` in std/core,
                    //     `SymbolTable::closure_shell`).
                    //
                    // `packed` and `transparent` aren't (yet) meaningful on
                    // class bodies — defer until a class needs them.
                    TokenKind::Layout => {
                        self.advance();
                        match self.current_kind().clone() {
                            TokenKind::Identifier(name) => {
                                self.advance();
                                if matches!(
                                    name.as_str(),
                                    "flat_heap_struct" | "closure" | "elem_desc" | "key_desc"
                                ) {
                                    layout.push(name);
                                } else if name.eq_ignore_ascii_case("c") {
                                    layout.push("C".to_string());
                                } else {
                                    self.error(&format!(
                                        "expected `flat_heap_struct`, `closure`, `elem_desc`, `key_desc` or `c` after `layout` in class body, found `{}`",
                                        name
                                    ));
                                }
                            }
                            other => {
                                self.error(&format!(
                                    "expected `flat_heap_struct`, `closure` or `c` after `layout` in class body, found {:?}",
                                    other
                                ));
                            }
                        }
                    }
                    TokenKind::Type => {
                        // ruby-naming.spec.md §3.4: a class that includes a
                        // mixin with `type Item` may bind it via
                        // `type Item = Concrete`. Resolve does not read the
                        // binding yet — the class also declares its
                        // concrete-return method, which carries the type
                        // through codegen — but the AST keeps it so
                        // `ruxen fmt` round-trips it.
                        let start = self.current_span();
                        self.advance();
                        let name = self.expect_type_identifier();
                        self.expect(TokenKind::Eq);
                        self.skip_newlines();
                        let type_expr = self.parse_type();
                        let span = self.span_from(&start);
                        assoc_types.push(AssocTypeBinding {
                            name,
                            type_expr,
                            span,
                        });
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
            }
            self.skip_newlines();
            self.ensure_loop_progress(progress);
        }
        self.expect(TokenKind::End);
        state.apply_name_lists(members.methods.iter_mut());
        let TypeMembers {
            methods,
            inner_impls,
            aliases,
        } = members;
        let span = self.span_from(&start);
        ClassDef {
            name,
            generic_params,
            parent,
            fields,
            methods,
            inner_impls,
            layout,
            lib_decls,
            doc_comments,
            where_clause,
            aliases,
            assoc_types,
            span,
        }
    }
}
