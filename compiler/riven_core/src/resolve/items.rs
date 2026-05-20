#![allow(unused_imports)]

use std::collections::HashMap;

use crate::diagnostics::Diagnostic;
use crate::hir::context::TypeContext;
use crate::hir::nodes::*;
use crate::hir::types::{MixinRef, MoveSemantics, Ty};
use crate::lexer::token::Span;
use crate::parser::ast::{self, Visibility};

use super::const_helpers;
use super::scope::{ScopeId, ScopeKind, ScopeStack};
use super::symbols::*;
use super::{ClosureCaptureContext, ResolveResult, Resolver};

impl Resolver {
    pub(super) fn resolve_item(&mut self, item: &ast::TopLevelItem) -> Option<HirItem> {
        match item {
            ast::TopLevelItem::Class(class) => Some(HirItem::Class(self.resolve_class(class))),
            ast::TopLevelItem::Struct(s) => Some(HirItem::Struct(self.resolve_struct(s))),
            ast::TopLevelItem::Enum(e) => Some(HirItem::Enum(self.resolve_enum(e))),
            ast::TopLevelItem::Mixin(t) => Some(HirItem::Mixin(self.resolve_trait(t))),
            ast::TopLevelItem::Impl(imp) => Some(HirItem::Impl(self.resolve_impl(imp))),
            ast::TopLevelItem::Function(f) => {
                Some(HirItem::Function(self.resolve_func_def(f, None)))
            }
            ast::TopLevelItem::TypeAlias(ta) => {
                let def_id = self
                    .type_registry
                    .get(&ta.name)
                    .copied()
                    .unwrap_or(UNRESOLVED_DEF);
                let ty = self.resolve_type_expr(&ta.type_expr);
                Some(HirItem::TypeAlias(HirTypeAlias {
                    def_id,
                    name: ta.name.clone(),
                    ty,
                    span: ta.span.clone(),
                }))
            }
            ast::TopLevelItem::Newtype(nt) => {
                let def_id = self
                    .type_registry
                    .get(&nt.name)
                    .copied()
                    .unwrap_or(UNRESOLVED_DEF);
                let inner_ty = self.resolve_type_expr(&nt.inner_type);
                Some(HirItem::Newtype(HirNewtype {
                    def_id,
                    name: nt.name.clone(),
                    inner_ty,
                    span: nt.span.clone(),
                }))
            }
            ast::TopLevelItem::Module(m) => Some(HirItem::Module(self.resolve_module(m))),
            ast::TopLevelItem::Const(c) => {
                let ty = self.resolve_type_expr(&c.type_expr);
                let value = self.resolve_expr(&c.value);
                let def_id = self.symbols.define(
                    c.name.clone(),
                    DefKind::Const { ty: ty.clone() },
                    Visibility::Public,
                    c.span.clone(),
                );
                self.scopes.insert(c.name.clone(), def_id);
                Some(HirItem::Const(HirConst {
                    def_id,
                    name: c.name.clone(),
                    ty,
                    value,
                    doc_comments: c.doc_comments.clone(),
                    span: c.span.clone(),
                }))
            }
            ast::TopLevelItem::Use(use_decl) => {
                self.resolve_use_decl(use_decl);
                None
            }
            ast::TopLevelItem::Lib(_) | ast::TopLevelItem::Extern(_) => {
                // FFI declarations are handled during codegen — they don't produce
                // HIR items. The functions they declare are resolved by name at
                // call sites during codegen (via runtime_name / get_or_declare_func).
                None
            }
        }
    }

    // ─── Class Resolution ───────────────────────────────────────────

    pub(super) fn resolve_class(&mut self, class: &ast::ClassDef) -> HirClassDef {
        let def_id = self
            .type_registry
            .get(&class.name)
            .copied()
            .unwrap_or(UNRESOLVED_DEF);

        let generic_params = self.resolve_generic_params(&class.generic_params);

        let parent_def = class.parent.as_ref().and_then(|p| {
            let name = p.segments.join(".");
            self.type_registry.get(&name).copied()
        });

        // Build the self type
        let self_ty = Ty::Class {
            name: class.name.clone(),
            generic_args: generic_params
                .iter()
                .map(|gp| Ty::TypeParam {
                    name: gp.name.clone(),
                    bounds: gp.bounds.clone(),
                })
                .collect(),
        };

        let old_self_ty = self.current_self_ty.replace(self_ty.clone());
        let old_class_def = self.current_class_def.replace(def_id);

        self.scopes.push(ScopeKind::Class);

        // Register generic type parameters in scope
        for gp in &generic_params {
            let gp_def = self.symbols.define(
                gp.name.clone(),
                DefKind::TypeParam {
                    bounds: gp.bounds.clone(),
                },
                Visibility::Private,
                gp.span.clone(),
            );
            self.scopes.insert_type(gp.name.clone(), gp_def);
        }

        // Register `Self` type
        let self_def_id = self.symbols.define(
            "Self".to_string(),
            DefKind::TypeAlias {
                target: self_ty.clone(),
            },
            Visibility::Private,
            class.span.clone(),
        );
        self.scopes.insert_type("Self".to_string(), self_def_id);

        // Resolve fields
        let mut fields = Vec::new();
        let mut field_def_ids = Vec::new();
        let mut opt_out_send = false;
        let mut opt_out_sync = false;
        let mut manual_send = false;
        let mut manual_sync = false;
        for (idx, field) in class.fields.iter().enumerate() {
            let ty = self.resolve_type_expr(&field.type_expr);
            let fid = self.symbols.define(
                field.name.clone(),
                DefKind::Field {
                    parent: def_id,
                    ty: ty.clone(),
                    index: idx,
                },
                field.visibility,
                field.span.clone(),
            );
            self.scopes.insert(field.name.clone(), fid);
            field_def_ids.push(fid);
            fields.push(HirFieldDef {
                def_id: fid,
                name: field.name.clone(),
                ty,
                visibility: field.visibility,
                index: idx,
                span: field.span.clone(),
            });
        }

        // Resolve methods
        let mut methods = Vec::new();
        let mut method_def_ids = Vec::new();
        for method in &class.methods {
            let hir_method = self.resolve_func_def(method, Some(def_id));
            method_def_ids.push(hir_method.def_id);
            methods.push(hir_method);
        }

        // Resolve inner impl blocks
        let mut impl_blocks = Vec::new();
        for inner in &class.inner_impls {
            let trait_ref = MixinRef {
                name: inner.trait_name.segments.join("."),
                generic_args: inner
                    .trait_name
                    .generic_args
                    .as_ref()
                    .map(|args| args.iter().map(|a| self.resolve_type_expr(a)).collect())
                    .unwrap_or_default(),
            };

            // Collect `type Foo = X` bindings from the inner impl block so
            // that `Self.Foo` in method signatures resolves concretely.
            let old_assoc = std::mem::take(&mut self.current_impl_assoc_types);
            for ii in &inner.items {
                if let ast::ImplItem::AssocType {
                    name, type_expr, ..
                } = ii
                {
                    let ty = self.resolve_type_expr(type_expr);
                    self.current_impl_assoc_types.insert(name.clone(), ty);
                }
            }

            let mut items = Vec::new();
            for ii in &inner.items {
                match ii {
                    ast::ImplItem::Method(f) => {
                        items.push(HirImplItem::Method(self.resolve_func_def(f, Some(def_id))));
                    }
                    ast::ImplItem::AssocType {
                        name,
                        type_expr,
                        span,
                    } => {
                        items.push(HirImplItem::AssocType {
                            name: name.clone(),
                            ty: self.resolve_type_expr(type_expr),
                            span: span.clone(),
                        });
                    }
                    ast::ImplItem::Include {
                        is_unsafe,
                        negative_trait,
                        trait_name,
                        span,
                    } => {
                        items.push(HirImplItem::Include {
                            is_unsafe: *is_unsafe,
                            negative_trait: *negative_trait,
                            trait_name: trait_name.segments.join("."),
                            span: span.clone(),
                        });
                    }
                }
            }
            self.current_impl_assoc_types = old_assoc;
            self.record_auto_trait_flags(
                &self_ty,
                Some(&trait_ref.name),
                inner.negative_trait,
                inner.is_unsafe,
                inner.span.clone(),
            );
            // Spec B10 (send_sync_enforcement.spec.md): plain
            // `include Send` is a user opt-in marker — equivalent to
            // `unsafe include Send` (B12) for v1, since user classes
            // have no auto-derive to suppress.
            match trait_ref.name.as_str() {
                "Send" if inner.negative_trait => opt_out_send = true,
                "Sync" if inner.negative_trait => opt_out_sync = true,
                "Send" => manual_send = true,
                "Sync" => manual_sync = true,
                _ => {}
            }

            impl_blocks.push(HirImplBlock {
                generic_params: vec![],
                is_unsafe: inner.is_unsafe,
                negative_trait: inner.negative_trait,
                trait_ref: Some(trait_ref),
                target_ty: self_ty.clone(),
                items,
                span: inner.span.clone(),
            });
        }

        self.scopes.pop();
        self.current_self_ty = old_self_ty;
        self.current_class_def = old_class_def;

        // Update the symbol table with full class info.  Pre-compute
        // generic_params (which needs `&mut self` for type-expr
        // resolution) before grabbing the mutable borrow of the symbol.
        let class_generic_param_infos = self.collect_generic_param_infos(&class.generic_params);
        // T2.02 S9: lower where-clause const predicates so the
        // instantiation site can evaluate them against the binding map.
        let const_predicates: Vec<_> = class
            .where_clause
            .as_ref()
            .map(|wc| {
                wc.const_predicates
                    .iter()
                    .map(const_helpers::lower_const_predicate)
                    .collect()
            })
            .unwrap_or_default();
        // #06.8 T0c: preserve the `layout flat_heap_struct` marker
        // captured in pass 1 across the pass-2 rewrite.
        let flat_heap_struct = class.layout.iter().any(|s| s == "flat_heap_struct");
        // #06.8 Phase 3b: append the class-body `lib` FFI methods that
        // pass-1 registered onto the side-map. They were registered as
        // `DefKind::Method` with `parent = def_id` and need to appear
        // in `ClassInfo.methods` so name lookups (`Foo.bar`) find them
        // alongside the in-body `def`s above.
        if let Some(lib_method_ids) = self.pass1_class_lib_methods.get(&def_id) {
            method_def_ids.extend(lib_method_ids.iter().copied());
        }
        if let Some(def) = self.symbols.get_mut(def_id) {
            def.kind = DefKind::Class {
                info: ClassInfo {
                    generic_params: class_generic_param_infos,
                    parent: parent_def,
                    fields: field_def_ids,
                    methods: method_def_ids,
                    derive_traits: class.derive_traits.clone(),
                    opt_out_send,
                    opt_out_sync,
                    manual_send,
                    manual_sync,
                    const_predicates,
                    flat_heap_struct,
                },
            };
        }

        HirClassDef {
            def_id,
            name: class.name.clone(),
            generic_params,
            parent: parent_def,
            fields,
            methods,
            impl_blocks,
            derive_traits: class.derive_traits.clone(),
            doc_comments: class.doc_comments.clone(),
            span: class.span.clone(),
        }
    }

    // ─── Struct Resolution ──────────────────────────────────────────

    pub(super) fn resolve_struct(&mut self, s: &ast::StructDef) -> HirStructDef {
        let def_id = self
            .type_registry
            .get(&s.name)
            .copied()
            .unwrap_or(UNRESOLVED_DEF);
        let generic_params = self.resolve_generic_params(&s.generic_params);

        let mut fields = Vec::new();
        let mut field_def_ids = Vec::new();
        for (idx, field) in s.fields.iter().enumerate() {
            let ty = self.resolve_type_expr(&field.type_expr);
            let fid = self.symbols.define(
                field.name.clone(),
                DefKind::Field {
                    parent: def_id,
                    ty: ty.clone(),
                    index: idx,
                },
                field.visibility,
                field.span.clone(),
            );
            field_def_ids.push(fid);
            fields.push(HirFieldDef {
                def_id: fid,
                name: field.name.clone(),
                ty,
                visibility: field.visibility,
                index: idx,
                span: field.span.clone(),
            });
        }

        // Update symbol table.  Pre-compute generic_params before
        // grabbing the mutable symbol borrow.
        let struct_generic_param_infos = self.collect_generic_param_infos(&s.generic_params);
        // T2.02 S9: lower where-clause const predicates.
        let const_predicates: Vec<_> = s
            .where_clause
            .as_ref()
            .map(|wc| {
                wc.const_predicates
                    .iter()
                    .map(const_helpers::lower_const_predicate)
                    .collect()
            })
            .unwrap_or_default();
        if let Some(def) = self.symbols.get_mut(def_id) {
            def.kind = DefKind::Struct {
                info: StructInfo {
                    generic_params: struct_generic_param_infos,
                    fields: field_def_ids,
                    derive_traits: s.derive_traits.clone(),
                    layout: s.layout.clone(),
                    opt_out_send: false,
                    opt_out_sync: false,
                    manual_send: false,
                    manual_sync: false,
                    const_predicates,
                },
            };
        }

        // ruby-naming.spec.md §3.4a: structs may carry inline methods
        // and `include Mixin` directives.
        let old_self_ty = self.current_self_ty.take();
        let self_ty = Ty::Struct {
            name: s.name.clone(),
            generic_args: vec![],
        };
        self.current_self_ty = Some(self_ty.clone());
        let methods = s
            .methods
            .iter()
            .map(|m| self.resolve_func_def(m, Some(def_id)))
            .collect::<Vec<_>>();
        let impl_blocks = self.lower_inner_impls(&s.inner_impls, &self_ty, Some(def_id));
        self.current_self_ty = old_self_ty;

        HirStructDef {
            def_id,
            name: s.name.clone(),
            generic_params,
            fields,
            methods,
            impl_blocks,
            derive_traits: s.derive_traits.clone(),
            layout: s.layout.clone(),
            doc_comments: s.doc_comments.clone(),
            span: s.span.clone(),
        }
    }

    // ─── Enum Resolution ────────────────────────────────────────────

    /// Lower a list of AST `InnerImpl` directives (collected from a
    /// struct or enum body under ruby-naming.spec.md §3.4a) into HIR
    /// `HirImplBlock` records. The same routine the class path uses,
    /// minus the class-specific `opt_out_*` tracking — struct/enum
    /// auto-trait flags live on their own info structs.
    pub(super) fn lower_inner_impls(
        &mut self,
        inner_impls: &[ast::InnerImpl],
        self_ty: &Ty,
        parent_def: Option<DefId>,
    ) -> Vec<HirImplBlock> {
        let mut impl_blocks = Vec::new();
        for inner in inner_impls {
            let trait_ref = MixinRef {
                name: inner.trait_name.segments.join("."),
                generic_args: inner
                    .trait_name
                    .generic_args
                    .as_ref()
                    .map(|args| args.iter().map(|a| self.resolve_type_expr(a)).collect())
                    .unwrap_or_default(),
            };

            let old_assoc = std::mem::take(&mut self.current_impl_assoc_types);
            for ii in &inner.items {
                if let ast::ImplItem::AssocType {
                    name, type_expr, ..
                } = ii
                {
                    let ty = self.resolve_type_expr(type_expr);
                    self.current_impl_assoc_types.insert(name.clone(), ty);
                }
            }

            let mut items = Vec::new();
            for ii in &inner.items {
                match ii {
                    ast::ImplItem::Method(f) => {
                        items.push(HirImplItem::Method(self.resolve_func_def(f, parent_def)));
                    }
                    ast::ImplItem::AssocType {
                        name,
                        type_expr,
                        span,
                    } => {
                        items.push(HirImplItem::AssocType {
                            name: name.clone(),
                            ty: self.resolve_type_expr(type_expr),
                            span: span.clone(),
                        });
                    }
                    ast::ImplItem::Include {
                        is_unsafe,
                        negative_trait,
                        trait_name,
                        span,
                    } => {
                        items.push(HirImplItem::Include {
                            is_unsafe: *is_unsafe,
                            negative_trait: *negative_trait,
                            trait_name: trait_name.segments.join("."),
                            span: span.clone(),
                        });
                    }
                }
            }
            self.current_impl_assoc_types = old_assoc;

            impl_blocks.push(HirImplBlock {
                generic_params: vec![],
                is_unsafe: inner.is_unsafe,
                negative_trait: inner.negative_trait,
                trait_ref: Some(trait_ref),
                target_ty: self_ty.clone(),
                items,
                span: inner.span.clone(),
            });
        }
        impl_blocks
    }

    pub(super) fn resolve_enum(&mut self, e: &ast::EnumDef) -> HirEnumDef {
        let def_id = self
            .type_registry
            .get(&e.name)
            .copied()
            .unwrap_or(UNRESOLVED_DEF);
        let generic_params = self.resolve_generic_params(&e.generic_params);

        // Push a scope so enum generic params are visible while resolving
        // variant field types (e.g. `Some(T)` in `enum MyOpt[T]`). Without
        // this, `T` resolved to `undefined type`, which propagated as an
        // `Error` payload type and kept the match/codegen paths from
        // producing a valid lowering.
        self.scopes.push(ScopeKind::Class);
        for gp in &generic_params {
            let gp_def = self.symbols.define(
                gp.name.clone(),
                DefKind::TypeParam {
                    bounds: gp.bounds.clone(),
                },
                Visibility::Private,
                gp.span.clone(),
            );
            self.scopes.insert_type(gp.name.clone(), gp_def);
        }

        let mut variants = Vec::new();
        let mut variant_def_ids = Vec::new();

        for (idx, variant) in e.variants.iter().enumerate() {
            let kind = match &variant.fields {
                ast::VariantKind::Unit => HirVariantKind::Unit,
                ast::VariantKind::Tuple(fields) => HirVariantKind::Tuple(
                    fields
                        .iter()
                        .map(|f| HirVariantField {
                            name: f.name.clone(),
                            ty: self.resolve_type_expr(&f.type_expr),
                            span: f.span.clone(),
                        })
                        .collect(),
                ),
                ast::VariantKind::Struct(fields) => HirVariantKind::Struct(
                    fields
                        .iter()
                        .map(|f| HirVariantField {
                            name: f.name.clone(),
                            ty: self.resolve_type_expr(&f.type_expr),
                            span: f.span.clone(),
                        })
                        .collect(),
                ),
            };

            // Look up the variant DefId registered in pass 1
            let composite_name = format!("{}.{}", e.name, variant.name);
            let vid = self.scopes.lookup(&composite_name).unwrap_or_else(|| {
                // Shouldn't happen if pass 1 ran correctly, but be defensive
                self.symbols.define(
                    variant.name.clone(),
                    DefKind::EnumVariant {
                        parent: def_id,
                        variant_idx: idx,
                        kind: VariantDefKind::Unit,
                    },
                    Visibility::Public,
                    variant.span.clone(),
                )
            });
            variant_def_ids.push(vid);

            variants.push(HirVariant {
                def_id: vid,
                name: variant.name.clone(),
                kind,
                index: idx,
                span: variant.span.clone(),
            });
        }

        // Update symbol table.  Pre-compute generic_params before
        // grabbing the mutable symbol borrow.
        let enum_generic_param_infos = self.collect_generic_param_infos(&e.generic_params);
        if let Some(def) = self.symbols.get_mut(def_id) {
            def.kind = DefKind::Enum {
                info: EnumInfo {
                    generic_params: enum_generic_param_infos,
                    variants: variant_def_ids,
                    derive_traits: e.derive_traits.clone(),
                    opt_out_send: false,
                    opt_out_sync: false,
                    manual_send: false,
                    manual_sync: false,
                    const_predicates: vec![],
                },
            };
        }

        // ruby-naming.spec.md §3.4a: enums may carry inline methods
        // and `include Mixin` directives.
        let old_self_ty = self.current_self_ty.take();
        let self_ty = Ty::Enum {
            name: e.name.clone(),
            generic_args: vec![],
        };
        self.current_self_ty = Some(self_ty.clone());
        let methods = e
            .methods
            .iter()
            .map(|m| self.resolve_func_def(m, Some(def_id)))
            .collect::<Vec<_>>();
        let impl_blocks = self.lower_inner_impls(&e.inner_impls, &self_ty, Some(def_id));
        self.current_self_ty = old_self_ty;

        self.scopes.pop();

        HirEnumDef {
            def_id,
            name: e.name.clone(),
            generic_params,
            variants,
            methods,
            impl_blocks,
            derive_traits: e.derive_traits.clone(),
            doc_comments: e.doc_comments.clone(),
            span: e.span.clone(),
        }
    }

    // ─── Trait Resolution ───────────────────────────────────────────

    pub(super) fn resolve_trait(&mut self, t: &ast::MixinDef) -> HirMixinDef {
        let def_id = self
            .type_registry
            .get(&t.name)
            .copied()
            .unwrap_or(UNRESOLVED_DEF);
        let generic_params = self.resolve_generic_params(&t.generic_params);

        self.scopes.push(ScopeKind::Trait);

        // Register Self as a type alias pointing to a TypeParam with this trait as bound
        let self_ty = Ty::TypeParam {
            name: "Self".to_string(),
            bounds: vec![MixinRef {
                name: t.name.clone(),
                generic_args: vec![],
            }],
        };
        let self_type_id = self.symbols.define(
            "Self".to_string(),
            DefKind::TypeAlias {
                target: self_ty.clone(),
            },
            Visibility::Private,
            t.span.clone(),
        );
        self.scopes.insert_type("Self".to_string(), self_type_id);

        // Make `self` (the value) available inside default method bodies so
        // expressions like `self.name` resolve to the abstract trait method.
        // The concrete `self` type is supplied when each impl monomorphises
        // the default body; here we only need a placeholder so the resolver
        // and typechecker treat it as a valid method-context value.
        let old_self_ty = self.current_self_ty.replace(self_ty);

        let super_traits: Vec<MixinRef> = t
            .super_traits
            .iter()
            .map(|b| MixinRef {
                name: b.path.segments.join("."),
                generic_args: b
                    .path
                    .generic_args
                    .as_ref()
                    .map(|args| args.iter().map(|a| self.resolve_type_expr(a)).collect())
                    .unwrap_or_default(),
            })
            .collect();

        // Make the trait's declared associated-type names visible so
        // `Self.Name` inside method signatures resolves to a placeholder
        // `Ty::TypeParam` (which behaves opaquely during trait resolution).
        let assoc_names: Vec<String> = t
            .items
            .iter()
            .filter_map(|ti| match ti {
                ast::MixinItem::AssocType { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect();
        let old_trait_ctx = self
            .current_trait_context
            .replace((t.name.clone(), assoc_names));

        let mut items = Vec::new();
        for ti in &t.items {
            match ti {
                ast::MixinItem::AssocType { name, span } => {
                    items.push(HirMixinItem::AssocType {
                        name: name.clone(),
                        span: span.clone(),
                    });
                }
                ast::MixinItem::MethodSig(sig) => {
                    let params = self.resolve_params(&sig.params);
                    let return_ty = sig
                        .return_type
                        .as_ref()
                        .map(|t| self.resolve_type_expr(t))
                        .unwrap_or(Ty::Unit);
                    let self_mode = sig.self_mode.map(|m| self.convert_self_mode(m));

                    items.push(HirMixinItem::MethodSig {
                        name: sig.name.clone(),
                        self_mode,
                        is_class_method: sig.is_class_method,
                        params,
                        return_ty,
                        span: sig.span.clone(),
                    });
                }
                ast::MixinItem::DefaultMethod(f) => {
                    items.push(HirMixinItem::DefaultMethod(self.resolve_func_def(f, None)));
                }
            }
        }

        self.current_trait_context = old_trait_ctx;
        self.current_self_ty = old_self_ty;
        self.scopes.pop();

        HirMixinDef {
            def_id,
            name: t.name.clone(),
            generic_params,
            super_traits,
            items,
            doc_comments: t.doc_comments.clone(),
            dispatch_mode: t.dispatch_mode,
            span: t.span.clone(),
        }
    }

    // ─── Impl Block Resolution ──────────────────────────────────────

    pub(super) fn resolve_impl(&mut self, imp: &ast::ImplBlock) -> HirImplBlock {
        let generic_params = self.resolve_generic_params(&imp.generic_params);
        let target_ty = self.resolve_type_expr(&imp.target_type);
        let trait_ref = imp.trait_name.as_ref().map(|tp| MixinRef {
            name: tp.segments.join("."),
            generic_args: tp
                .generic_args
                .as_ref()
                .map(|args| args.iter().map(|a| self.resolve_type_expr(a)).collect())
                .unwrap_or_default(),
        });
        if let Some(ref trait_ref) = trait_ref {
            self.record_auto_trait_flags(
                &target_ty,
                Some(&trait_ref.name),
                imp.negative_trait,
                imp.is_unsafe,
                imp.span.clone(),
            );
        } else if imp.is_unsafe {
            self.diagnostics.push(Diagnostic::error_with_code(
                "`unsafe include` is only meaningful for mixin inclusions",
                imp.span.clone(),
                "E1014",
            ));
        }

        // Determine the class def for self resolution
        let class_def = match &target_ty {
            Ty::Class { name, .. } | Ty::Enum { name, .. } | Ty::Struct { name, .. } => {
                self.type_registry.get(name).copied()
            }
            _ => None,
        };

        let old_self_ty = self.current_self_ty.replace(target_ty.clone());
        let old_class_def = std::mem::replace(&mut self.current_class_def, class_def);

        self.scopes.push(ScopeKind::Impl);

        // Register Self type
        let self_type_id = self.symbols.define(
            "Self".to_string(),
            DefKind::TypeAlias {
                target: target_ty.clone(),
            },
            Visibility::Private,
            imp.span.clone(),
        );
        self.scopes.insert_type("Self".to_string(), self_type_id);

        // First pass: collect `type Foo = X` bindings so that `Self.Foo`
        // references inside method signatures/bodies resolve to the
        // concrete type declared here.
        let old_assoc = std::mem::take(&mut self.current_impl_assoc_types);
        for ii in &imp.items {
            if let ast::ImplItem::AssocType {
                name, type_expr, ..
            } = ii
            {
                let ty = self.resolve_type_expr(type_expr);
                self.current_impl_assoc_types.insert(name.clone(), ty);
            }
        }

        let mut items = Vec::new();
        for ii in &imp.items {
            match ii {
                ast::ImplItem::Method(f) => {
                    items.push(HirImplItem::Method(self.resolve_func_def(f, class_def)));
                }
                ast::ImplItem::AssocType {
                    name,
                    type_expr,
                    span,
                } => {
                    items.push(HirImplItem::AssocType {
                        name: name.clone(),
                        ty: self.resolve_type_expr(type_expr),
                        span: span.clone(),
                    });
                }
                ast::ImplItem::Include {
                    is_unsafe,
                    negative_trait,
                    trait_name,
                    span,
                } => {
                    items.push(HirImplItem::Include {
                        is_unsafe: *is_unsafe,
                        negative_trait: *negative_trait,
                        trait_name: trait_name.segments.join("."),
                        span: span.clone(),
                    });
                }
            }
        }

        self.current_impl_assoc_types = old_assoc;
        self.scopes.pop();
        self.current_self_ty = old_self_ty;
        self.current_class_def = old_class_def;

        HirImplBlock {
            generic_params,
            is_unsafe: imp.is_unsafe,
            negative_trait: imp.negative_trait,
            trait_ref,
            target_ty,
            items,
            span: imp.span.clone(),
        }
    }

    pub(super) fn record_auto_trait_flags(
        &mut self,
        target_ty: &Ty,
        trait_name: Option<&str>,
        negative_trait: bool,
        is_unsafe: bool,
        span: Span,
    ) {
        let Some(trait_name) = trait_name else {
            return;
        };

        let (mark_send, mark_sync) = match trait_name {
            "Send" => (true, false),
            "Sync" => (false, true),
            _ => {
                if negative_trait {
                    self.diagnostics.push(Diagnostic::error_with_code(
                        "negative include (`exclude`) is only supported for Send and Sync",
                        span,
                        "E1014",
                    ));
                } else if is_unsafe {
                    self.diagnostics.push(Diagnostic::error_with_code(
                        "`unsafe include` is only required for Send and Sync",
                        span,
                        "E1014",
                    ));
                }
                return;
            }
        };

        // Spec B10 (send_sync_enforcement.spec.md): plain `include Send`
        // / `include Sync` is a user opt-in marker for v1. `unsafe
        // include Send` (B12) is the explicit-safety-assertion variant
        // for classes that wrap raw pointers / non-Send containers.
        // Both forms set `manual_send` / `manual_sync` — v1 has no
        // user-class auto-derive, so the two are equivalent here.
        // Negative include (`include !Send`) sets `opt_out_send` (B11).
        // E1014 stays reserved (was previously raised here when neither
        // form was present, but the spec promoted the bare marker form).
        let _ = is_unsafe; // accepted but not differentiated for v1

        let Some(def) = const_helpers::nominal_type_definition_mut(target_ty, &mut self.symbols)
        else {
            return;
        };

        match &mut def.kind {
            DefKind::Class { info } => {
                if mark_send {
                    info.opt_out_send = negative_trait;
                    info.manual_send = !negative_trait;
                }
                if mark_sync {
                    info.opt_out_sync = negative_trait;
                    info.manual_sync = !negative_trait;
                }
            }
            DefKind::Struct { info } => {
                if mark_send {
                    info.opt_out_send = negative_trait;
                    info.manual_send = !negative_trait;
                }
                if mark_sync {
                    info.opt_out_sync = negative_trait;
                    info.manual_sync = !negative_trait;
                }
            }
            DefKind::Enum { info } => {
                if mark_send {
                    info.opt_out_send = negative_trait;
                    info.manual_send = !negative_trait;
                }
                if mark_sync {
                    info.opt_out_sync = negative_trait;
                    info.manual_sync = !negative_trait;
                }
            }
            _ => {}
        }
    }

    // ─── Function Resolution ────────────────────────────────────────

}
