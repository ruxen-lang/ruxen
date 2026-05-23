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
    pub(super) fn resolve_func_def(
        &mut self,
        f: &ast::FuncDef,
        parent: Option<DefId>,
    ) -> HirFuncDef {
        let mut generic_params = self.resolve_generic_params(&f.generic_params);
        // Merge `where T: Bound, ...` predicates into the matching generic
        // parameter's bounds. Predicates whose left-hand side is not a
        // declared type parameter (e.g., associated-type constraints like
        // `Iterable[Item = Int]`) are parsed and dropped for now — they
        // require associated-type infrastructure not yet present.
        if let Some(ref wc) = f.where_clause {
            for pred in &wc.predicates {
                if let ast::TypeExpr::Named(path) = &pred.type_expr {
                    if path.segments.len() == 1 && path.generic_args.is_none() {
                        let name = &path.segments[0];
                        if let Some(gp) = generic_params.iter_mut().find(|g| &g.name == name) {
                            for bound in &pred.bounds {
                                gp.bounds.push(MixinRef {
                                    name: bound.path.segments.join("."),
                                    generic_args: bound
                                        .path
                                        .generic_args
                                        .as_ref()
                                        .map(|args| {
                                            args.iter().map(|a| self.resolve_type_expr(a)).collect()
                                        })
                                        .unwrap_or_default(),
                                });
                            }
                        }
                    }
                }
                // TODO: associated-type bounds (e.g. `A: Iterable[Item = Int]`)
                // are parsed but ignored until the type system models them.
            }
        }

        self.scopes.push(ScopeKind::Function);

        // Register generic type params in scope
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

        let self_mode = f.self_mode.map(|m| self.convert_self_mode(m));

        // Register self if this is a method.
        // If we're inside a class/impl body (current_self_ty is set) and
        // the function has no explicit self_mode, default to:
        //   - &mut self for init (needs to assign fields)
        //   - &self for all other instance methods
        // Class methods (self.method_name) don't get implicit self.
        let self_mode =
            if self_mode.is_none() && self.current_self_ty.is_some() && !f.is_class_method {
                if f.name == "init" {
                    Some(HirSelfMode::RefMut)
                } else {
                    Some(HirSelfMode::Ref)
                }
            } else {
                self_mode
            };

        if let Some(ref self_ty) = self.current_self_ty {
            if self_mode.is_some() {
                let self_def = self.symbols.define(
                    "self".to_string(),
                    DefKind::SelfValue {
                        ty: self_ty.clone(),
                    },
                    Visibility::Private,
                    f.span.clone(),
                );
                self.scopes.insert("self".to_string(), self_def);
            }
        }

        // Resolve parameters
        let mut params = self.resolve_and_register_params(&f.params);

        // If this function's body contains `yield`, append a synthetic
        // `__block: Fn(…) -> ()` parameter so `yield VALUE` can desugar
        // to `__block.(VALUE)` and callers can forward a trailing block.
        if let Some(&arity) = self.yield_fns.get(&f.name) {
            let block_ty = Ty::Fn {
                params: (0..arity)
                    .map(|_| self.type_context.fresh_type_var())
                    .collect(),
                ret: Box::new(self.type_context.fresh_type_var()),
            };
            let block_def_id = self.symbols.define(
                "__block".to_string(),
                DefKind::Param {
                    ty: block_ty.clone(),
                    auto_assign: false,
                },
                Visibility::Private,
                f.span.clone(),
            );
            self.scopes.insert("__block".to_string(), block_def_id);
            params.push(HirParam {
                def_id: block_def_id,
                name: "__block".to_string(),
                ty: block_ty,
                auto_assign: false,
                span: f.span.clone(),
            });
        }

        let return_ty = f
            .return_type
            .as_ref()
            .map(|t| self.resolve_type_expr(t))
            .unwrap_or_else(|| {
                // Default to Unit for:
                // - init methods (constructors)
                // - mut methods (typically mutate in place, return nothing)
                // - main function
                // - display/display_all methods (void-like)
                // Otherwise use a fresh type var for inference
                let is_mut = matches!(f.self_mode, Some(ast::SelfMode::Mutable));
                let is_init = f.name == "init";
                let is_main = f.name == "main" && self.current_self_ty.is_none();
                let is_display_like = f.name == "display" || f.name == "display_all";
                if is_init || is_mut || is_main || is_display_like {
                    Ty::Unit
                } else {
                    self.type_context.fresh_type_var()
                }
            });

        let old_return_ty = self.current_return_ty.replace(return_ty.clone());
        let old_async_scope_depth = self.async_scope_depth;
        if f.is_async {
            self.async_scope_depth += 1;
        }

        let body = self.resolve_block_as_expr(&f.body);

        self.async_scope_depth = old_async_scope_depth;
        self.current_return_ty = old_return_ty;
        self.scopes.pop();

        let sig = FnSignature {
            self_mode,
            is_class_method: f.is_class_method,
            is_async: f.is_async,
            generic_params: self.collect_generic_param_infos(&f.generic_params),
            params: params
                .iter()
                .map(|p| ParamInfo {
                    name: p.name.clone(),
                    ty: p.ty.clone(),
                    auto_assign: p.auto_assign,
                })
                .collect(),
            return_ty: return_ty.clone(),
            c_symbol: None,
        };

        let def_kind = if let Some(parent) = parent {
            DefKind::Method {
                parent,
                signature: sig,
            }
        } else {
            DefKind::Function { signature: sig }
        };

        let def_id = self
            .symbols
            .define(f.name.clone(), def_kind, f.visibility, f.span.clone());

        // Register the function name in the enclosing scope (not the function scope we just popped)
        self.scopes.insert(f.name.clone(), def_id);

        HirFuncDef {
            def_id,
            name: f.name.clone(),
            visibility: f.visibility,
            is_async: f.is_async,
            self_mode,
            is_class_method: f.is_class_method,
            generic_params,
            params,
            return_ty,
            body: Box::new(body),
            doc_comments: f.doc_comments.clone(),
            span: f.span.clone(),
        }
    }

    // ─── Module Resolution ──────────────────────────────────────────

    // ─── Use Declaration Resolution ────────────────────────────────

    pub(super) fn resolve_use_decl(&mut self, use_decl: &ast::UseDecl) {
        let path = &use_decl.path;
        if path.is_empty() {
            self.diagnostics.push(Diagnostic::error(
                "empty use path".to_string(),
                use_decl.span.clone(),
            ));
            return;
        }

        // Try to resolve the first segment as a known type or module
        let first = &path[0];
        let root_def_id = self
            .scopes
            .lookup_type(first)
            .or_else(|| self.scopes.lookup(first));

        match root_def_id {
            Some(def_id) => {
                // Walk the remaining path segments to resolve nested names
                let target_def_id = self.resolve_use_path_from(def_id, &path[1..], use_decl);

                if let Some(final_id) = target_def_id {
                    // Import the name(s) into the current scope based on UseKind
                    match &use_decl.kind {
                        ast::UseKind::Simple => {
                            // `use Foo.Bar.Baz` — import the last segment name
                            let import_name = path.last().unwrap().clone();
                            self.scopes.insert(import_name.clone(), final_id);
                            self.scopes.insert_type(import_name, final_id);
                        }
                        ast::UseKind::Alias(alias) => {
                            // `use Foo.Bar as B` — import under the alias
                            self.scopes.insert(alias.clone(), final_id);
                            self.scopes.insert_type(alias.clone(), final_id);
                        }
                        ast::UseKind::Group(names) => {
                            // `use Foo.Bar.{ X, Y }` — import each named item
                            // final_id should be a module; resolve each name within it
                            for name in names {
                                let child_id = self.resolve_child_in_def(final_id, name, use_decl);
                                if let Some(cid) = child_id {
                                    self.scopes.insert(name.clone(), cid);
                                    self.scopes.insert_type(name.clone(), cid);
                                }
                            }
                        }
                    }
                }
            }
            None => {
                // Flat-merge path-deps (build.rs) don't register the
                // package name as a module — every type the dep exports
                // already lives at top-level. So `use rondo.Rondo`
                // can't find `rondo` as a module, but `Rondo` IS in
                // scope. Honour the use statement by checking whether
                // the final segment resolves at top-level; if so,
                // treat it as a no-op (the symbol is already imported
                // by the flat merge). This is the workaround until
                // module-wrapped path-deps work end-to-end (the
                // module-wrap exposes a nested-class user-body method
                // MIR-naming bug — see B12 in
                // `docs/rondo_v1_blockers.md`).
                let last = path.last().unwrap();
                let fallback = self
                    .scopes
                    .lookup_type(last)
                    .or_else(|| self.scopes.lookup(last));
                if fallback.is_some() {
                    // Symbol already in scope via flat-merge. Re-bind
                    // under the requested name (Simple/Alias/Group)
                    // so the explicit `use` still produces a usable
                    // local alias.
                    let final_id = fallback.unwrap();
                    match &use_decl.kind {
                        ast::UseKind::Simple => {
                            let import_name = last.clone();
                            self.scopes.insert(import_name.clone(), final_id);
                            self.scopes.insert_type(import_name, final_id);
                        }
                        ast::UseKind::Alias(alias) => {
                            self.scopes.insert(alias.clone(), final_id);
                            self.scopes.insert_type(alias.clone(), final_id);
                        }
                        ast::UseKind::Group(_) => {
                            // For grouped `use rondo.{Rondo, Request}`,
                            // each name must already be top-level. Walk
                            // and bind any that resolve; silently skip
                            // those that don't (matches the existing
                            // module-walk semantics).
                            if let ast::UseKind::Group(names) = &use_decl.kind {
                                for name in names {
                                    let id = self
                                        .scopes
                                        .lookup_type(name)
                                        .or_else(|| self.scopes.lookup(name));
                                    if let Some(cid) = id {
                                        self.scopes.insert(name.clone(), cid);
                                        self.scopes.insert_type(name.clone(), cid);
                                    }
                                }
                            }
                        }
                    }
                    return;
                }
                self.diagnostics.push(Diagnostic::error(
                    format!(
                        "unknown module '{}'. Did you forget to add it to [dependencies]?",
                        first
                    ),
                    use_decl.span.clone(),
                ));
            }
        }
    }

    /// Walk a use path from a starting DefId through remaining segments.
    pub(super) fn resolve_use_path_from(
        &mut self,
        mut current: DefId,
        segments: &[String],
        use_decl: &ast::UseDecl,
    ) -> Option<DefId> {
        for seg in segments {
            match self.resolve_child_in_def(current, seg, use_decl) {
                Some(child) => current = child,
                None => return None,
            }
        }
        Some(current)
    }

    /// Resolve a child name within a definition (module, class, enum, etc.).
    pub(super) fn resolve_child_in_def(
        &mut self,
        parent: DefId,
        name: &str,
        use_decl: &ast::UseDecl,
    ) -> Option<DefId> {
        let parent_def = self.symbols.get(parent).cloned();
        match parent_def {
            Some(def) => {
                match &def.kind {
                    DefKind::Module { items } => {
                        // Search module items for the name
                        for &item_id in items {
                            if let Some(item_def) = self.symbols.get(item_id) {
                                if item_def.name == name {
                                    return Some(item_id);
                                }
                            }
                        }
                        self.diagnostics.push(Diagnostic::error(
                            format!("'{}' not found in module '{}'", name, def.name),
                            use_decl.span.clone(),
                        ));
                        None
                    }
                    DefKind::Enum { info } => {
                        // Allow `use MyEnum.Variant`
                        for &variant_id in &info.variants {
                            if let Some(variant_def) = self.symbols.get(variant_id) {
                                if variant_def.name == name {
                                    return Some(variant_id);
                                }
                            }
                        }
                        self.diagnostics.push(Diagnostic::error(
                            format!("'{}' is not a variant of enum '{}'", name, def.name),
                            use_decl.span.clone(),
                        ));
                        None
                    }
                    DefKind::Class { info } => {
                        // Allow `use MyClass.method` for class methods
                        for &method_id in &info.methods {
                            if let Some(method_def) = self.symbols.get(method_id) {
                                if method_def.name == name {
                                    return Some(method_id);
                                }
                            }
                        }
                        self.diagnostics.push(Diagnostic::error(
                            format!("'{}' not found in class '{}'", name, def.name),
                            use_decl.span.clone(),
                        ));
                        None
                    }
                    _ => {
                        self.diagnostics.push(Diagnostic::error(
                            format!("'{}' is not a module or namespace", def.name),
                            use_decl.span.clone(),
                        ));
                        None
                    }
                }
            }
            None => {
                self.diagnostics.push(Diagnostic::error(
                    "unresolved name in use path".to_string(),
                    use_decl.span.clone(),
                ));
                None
            }
        }
    }

    pub(super) fn resolve_module(&mut self, m: &ast::ModuleDef) -> HirModule {
        let def_id = self
            .type_registry
            .get(&m.name)
            .copied()
            .unwrap_or(UNRESOLVED_DEF);

        self.scopes.push(ScopeKind::Module);
        // Track the module path so nested class/struct/enum resolution
        // can build qualified `type_registry` keys (pass-1 registers
        // a nested `class Bar` under `"foo.Bar"`, so pass-2's
        // resolve_class needs the same qualified key — without this
        // it gets `UNRESOLVED_DEF` and the class's fields/methods
        // land on a dangling DefId, surfacing as "no field x on type
        // Bar" inside method bodies). Pin: `rondo_v1_blockers.md` B12.
        self.current_module_path.push(m.name.clone());

        let mut items = Vec::new();
        for item in &m.items {
            if let Some(hir_item) = self.resolve_item(item) {
                items.push(hir_item);
            }
        }

        self.current_module_path.pop();
        self.scopes.pop();

        HirModule {
            def_id,
            name: m.name.clone(),
            items,
            span: m.span.clone(),
        }
    }

    // ─── Expression Resolution ──────────────────────────────────────
}
