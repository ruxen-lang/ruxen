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
    fn overload_accepts_arg_count(&self, def_id: DefId, arg_count: usize) -> bool {
        let Some(def) = self.symbols.get(def_id) else {
            return false;
        };
        let signature = match &def.kind {
            DefKind::Function { signature } | DefKind::Method { signature, .. } => signature,
            _ => return false,
        };
        let required = signature
            .params
            .iter()
            .filter(|p| p.default.is_none())
            .count();
        arg_count >= required && arg_count <= signature.params.len()
    }

    fn overload_accepts_args(&self, def_id: DefId, args: &[HirExpr]) -> bool {
        let Some(def) = self.symbols.get(def_id) else {
            return false;
        };
        let signature = match &def.kind {
            DefKind::Function { signature } | DefKind::Method { signature, .. } => signature,
            _ => return false,
        };
        if !self.overload_accepts_arg_count(def_id, args.len()) {
            return false;
        }
        args.iter()
            .zip(signature.params.iter())
            .all(|(arg, param)| {
                arg.ty.is_infer()
                    || arg.ty.is_error()
                    || arg.ty == param.ty
                    || matches!((&arg.ty, &param.ty), (Ty::Str, Ty::String))
            })
    }

    pub(super) fn select_overload_by_args(&self, def_id: DefId, args: &[HirExpr]) -> DefId {
        let Some(def) = self.symbols.get(def_id) else {
            return def_id;
        };
        let DefKind::OverloadSet { candidates } = &def.kind else {
            return def_id;
        };
        candidates
            .iter()
            .copied()
            .filter(|candidate| self.overload_accepts_args(*candidate, args))
            .find(|candidate| {
                self.symbols
                    .get(*candidate)
                    .and_then(|def| match &def.kind {
                        DefKind::Function { signature } | DefKind::Method { signature, .. } => {
                            Some(signature.params.len() == args.len())
                        }
                        _ => None,
                    })
                    .unwrap_or(false)
            })
            .or_else(|| {
                candidates
                    .iter()
                    .copied()
                    .find(|candidate| self.overload_accepts_args(*candidate, args))
            })
            .or_else(|| {
                candidates
                    .iter()
                    .copied()
                    .find(|candidate| self.overload_accepts_arg_count(*candidate, args.len()))
            })
            .unwrap_or(def_id)
    }

    pub(super) fn select_overload_candidate_by_args(
        &self,
        candidates: &[DefId],
        args: &[HirExpr],
    ) -> Option<DefId> {
        candidates
            .iter()
            .copied()
            .filter(|candidate| self.overload_accepts_args(*candidate, args))
            .find(|candidate| {
                self.symbols
                    .get(*candidate)
                    .and_then(|def| match &def.kind {
                        DefKind::Function { signature } | DefKind::Method { signature, .. } => {
                            Some(signature.params.len() == args.len())
                        }
                        _ => None,
                    })
                    .unwrap_or(false)
            })
            .or_else(|| {
                candidates
                    .iter()
                    .copied()
                    .find(|candidate| self.overload_accepts_args(*candidate, args))
            })
            .or_else(|| {
                candidates
                    .iter()
                    .copied()
                    .find(|candidate| self.overload_accepts_arg_count(*candidate, args.len()))
            })
    }

    /// True if `p` is the explicit `&block:` parameter (parser stores its
    /// name as `&block`). Ruby-block-semantics ADR D1/D4.
    pub(super) fn is_explicit_block_param(p: &ast::Param) -> bool {
        p.name == "&block"
    }

    /// Synthesize and register the `__block` slot for an explicit
    /// `&block: Fn[(T…) -> R]` parameter (Ruby-block-semantics ADR).
    ///
    /// Reuses the exact representation the implicit-`yield` path uses (a
    /// trailing `__block` param holding the closure-pair-pointer) so `yield`,
    /// `block.(…)`, and call-site trailing-block forwarding all work
    /// unchanged. Differences from the implicit path:
    ///   - the block's `Ty::Fn` params/return come from the DECLARED
    ///     annotation, so `yield`'s value is the declared `R` (ADR D2);
    ///   - the param carries a `nil` DEFAULT, making the block OPTIONAL at
    ///     call sites (ADR D5) — `append_default_args` fills a null sentinel
    ///     (ADR D1) when no block is passed;
    ///   - the slot is bound under `__block` (yield desugar), `&block` (the
    ///     yield fallback lookup), and `block` (so `block.(…)` resolves).
    ///
    /// Returns the synthesized `HirParam` to push onto the function's param
    /// list (last position is enforced separately, E1119).
    pub(super) fn register_explicit_block_param(&mut self, p: &ast::Param) -> HirParam {
        // `parse_block_type` produces a `TypeExpr::Function`, so this resolves
        // to a concrete `Ty::Fn { params, ret }` — its `ret` is what makes
        // `yield`'s value typed `R` (ADR D2). A stray non-Function annotation
        // resolves as-is and the typeck `.call` path handles it.
        let block_ty = self.resolve_type_expr(&p.type_expr);
        let block_def_id = self.symbols.define(
            "__block".to_string(),
            DefKind::Param {
                ty: block_ty.clone(),
                auto_assign: false,
            },
            Visibility::Private,
            p.span.clone(),
        );
        // Bind every name the body / desugar might use for the slot.
        self.scopes.insert("__block".to_string(), block_def_id);
        self.scopes.insert("&block".to_string(), block_def_id);
        self.scopes.insert("block".to_string(), block_def_id);
        HirParam {
            def_id: block_def_id,
            name: "__block".to_string(),
            ty: block_ty,
            auto_assign: false,
            // `nil` → NullLiteral → null closure-pair-pointer sentinel.
            default: Some(ast::Expr {
                kind: ast::ExprKind::NullLiteral,
                span: p.span.clone(),
            }),
            span: p.span.clone(),
        }
    }

    pub(super) fn append_default_args(&mut self, def_id: DefId, args: &mut Vec<HirExpr>) {
        let defaults: Vec<crate::parser::ast::Expr> = self
            .symbols
            .get(def_id)
            .and_then(|def| match &def.kind {
                DefKind::Function { signature } | DefKind::Method { signature, .. } => {
                    Some(signature)
                }
                _ => None,
            })
            .map(|signature| {
                signature
                    .params
                    .iter()
                    .skip(args.len())
                    .filter_map(|p| p.default.clone())
                    .collect()
            })
            .unwrap_or_default();
        for default in defaults {
            args.push(self.resolve_expr(&default));
        }
    }

    fn bind_callable_name(&mut self, source_name: &str, def_id: DefId, span: Span) -> String {
        let mut final_name = source_name.to_string();
        let existing = self
            .scopes
            .lookup_with_scope(source_name)
            .and_then(|(id, scope)| {
                if scope == self.scopes.current_id() {
                    Some(id)
                } else {
                    None
                }
            });
        match existing.and_then(|id| self.symbols.get(id).map(|d| (id, d.kind.clone()))) {
            Some((prev_id, DefKind::Function { .. } | DefKind::Method { .. }))
                if self
                    .symbols
                    .get(prev_id)
                    .map(|d| d.span == span)
                    .unwrap_or(false) =>
            {
                self.scopes.insert(source_name.to_string(), def_id);
            }
            Some((set_id, DefKind::OverloadSet { mut candidates })) => {
                if let Some(slot) = candidates.iter_mut().find(|candidate| {
                    self.symbols
                        .get(**candidate)
                        .map(|d| d.span == span)
                        .unwrap_or(false)
                }) {
                    final_name = self
                        .symbols
                        .get(*slot)
                        .map(|d| d.name.clone())
                        .unwrap_or_else(|| source_name.to_string());
                    if let Some(def) = self.symbols.get_mut(def_id) {
                        def.name = final_name.clone();
                    }
                    *slot = def_id;
                } else {
                    final_name = format!("{}__overload{}", source_name, def_id);
                    if let Some(def) = self.symbols.get_mut(def_id) {
                        def.name = final_name.clone();
                    }
                    candidates.push(def_id);
                }
                if let Some(def) = self.symbols.get_mut(set_id) {
                    def.kind = DefKind::OverloadSet { candidates };
                }
            }
            Some((prev_id, DefKind::Function { .. })) | Some((prev_id, DefKind::Method { .. })) => {
                final_name = format!("{}__overload{}", source_name, def_id);
                if let Some(def) = self.symbols.get_mut(def_id) {
                    def.name = final_name.clone();
                }
                let set_id = self.symbols.define(
                    source_name.to_string(),
                    DefKind::OverloadSet {
                        candidates: vec![prev_id, def_id],
                    },
                    Visibility::Public,
                    span,
                );
                self.scopes.insert(source_name.to_string(), set_id);
            }
            _ => {
                self.scopes.insert(source_name.to_string(), def_id);
            }
        }
        final_name
    }

    fn bind_method_name(
        &mut self,
        source_name: &str,
        parent: DefId,
        def_id: DefId,
        span: Span,
    ) -> String {
        let duplicate = self.symbols.iter().any(|def| {
            if def.id == def_id {
                return false;
            }
            let DefKind::Method {
                parent: method_parent,
                ..
            } = &def.kind
            else {
                return false;
            };
            *method_parent == parent
                && def.span != span
                && (def.name == source_name
                    || def.name.starts_with(&format!("{}__overload", source_name)))
        });
        if duplicate {
            let final_name = format!("{}__overload{}", source_name, def_id);
            if let Some(def) = self.symbols.get_mut(def_id) {
                def.name = final_name.clone();
            }
            final_name
        } else {
            source_name.to_string()
        }
    }

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
                        let refs: Vec<MixinRef> = pred
                            .bounds
                            .iter()
                            .map(|bound| MixinRef {
                                name: bound.path.segments.join("."),
                                generic_args: bound
                                    .path
                                    .generic_args
                                    .as_ref()
                                    .map(|args| {
                                        args.iter().map(|a| self.resolve_type_expr(a)).collect()
                                    })
                                    .unwrap_or_default(),
                            })
                            .collect();
                        if let Some(gp) = generic_params.iter_mut().find(|g| &g.name == name) {
                            // The predicate constrains one of THIS method's own
                            // generic params — merge into its bounds.
                            gp.bounds.extend(refs);
                        } else if self
                            .scopes
                            .lookup_type(name)
                            .and_then(|d| self.symbols.get(d))
                            .map(|d| matches!(d.kind, DefKind::TypeParam { .. }))
                            .unwrap_or(false)
                        {
                            // The predicate constrains an ENCLOSING (class /
                            // struct / enum) generic — the receiver's element
                            // type, e.g. `class Array[T]`'s `def sum where T:
                            // Add`. The method's own generics are not yet in
                            // scope at this point (inserted just below), so a
                            // `lookup_type` hit here is necessarily a class
                            // generic. Thread it into the signature as a
                            // synthetic bounded param so the call-site
                            // receiver-element seam (`bridge_builtin_method` /
                            // `infer_selected_method`) can bind `{T → element}`
                            // and run the SAME `check_generic_param_bounds`
                            // enforcement. Without this the bound is silently
                            // dropped (the historical `sum`/E0700 fork).
                            generic_params.push(HirGenericParam {
                                name: name.clone(),
                                bounds: refs,
                                span: pred.span.clone(),
                            });
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

        // Separate the explicit `&block:` parameter (Ruby-block-semantics
        // ADR) from the ordinary positional parameters. It MUST be last
        // (ADR D4 → E1119): any positional parameter after it is an error.
        let block_param_pos = f.params.iter().position(Self::is_explicit_block_param);
        let mut explicit_block_param: Option<&ast::Param> = None;
        let ordinary_params: Vec<ast::Param> = if let Some(pos) = block_param_pos {
            explicit_block_param = Some(&f.params[pos]);
            // Enforce last-position: nothing may follow the block param.
            if pos != f.params.len() - 1 {
                let offender = &f.params[pos + 1];
                self.diagnostics.push(Diagnostic::error_with_code(
                    format!(
                        "block parameter `&block` must be the last parameter, but `{}` follows it",
                        offender.name
                    ),
                    offender.span.clone(),
                    "E1119",
                ));
            }
            f.params
                .iter()
                .filter(|p| !Self::is_explicit_block_param(p))
                .cloned()
                .collect()
        } else {
            f.params.clone()
        };

        // Resolve parameters (ordinary positional params only).
        let mut params = self.resolve_and_register_params(&ordinary_params);

        // Register the explicit `&block` slot, if any. This supersedes the
        // implicit yield-scan synthesis below (the slot already exists).
        if let Some(bp) = explicit_block_param {
            let hir_block = self.register_explicit_block_param(bp);
            params.push(hir_block);
        }
        let has_explicit_block = explicit_block_param.is_some();

        // If this function's body contains `yield`, append a synthetic
        // `__block: Fn(…) -> ()` parameter so `yield VALUE` can desugar
        // to `__block.(VALUE)` and callers can forward a trailing block.
        // Skipped when an explicit `&block` already provided the slot —
        // the explicit annotation is authoritative (typed `R`, optional).
        if let Some(&arity) = self.yield_fns.get(&f.name).filter(|_| !has_explicit_block) {
            // For each yield argument that is a bare `self`, type the matching
            // block parameter as the enclosing class instead of a fresh type
            // variable. A method's `yield self` then propagates a CONCRETE
            // block-parameter type to the call site — the method-call path
            // seeds the trailing block from a cloned signature, which loses
            // the link to a fresh var the body would later resolve (free
            // functions already resolve it). This is what makes the Ruby
            // builder DSL `widget do |w| … end` infer `w` without annotation.
            let self_ty_opt = self.current_self_ty.clone();
            let self_mask = super::yield_scan::first_yield_self_mask_in_block(&f.body);
            let block_ty = Ty::Fn {
                params: (0..arity)
                    .map(|i| {
                        let is_self = self_mask
                            .as_ref()
                            .and_then(|m| m.get(i))
                            .copied()
                            .unwrap_or(false);
                        if is_self {
                            if let Some(ref ty) = self_ty_opt {
                                return ty.clone();
                            }
                        }
                        self.type_context.fresh_type_var()
                    })
                    .collect(),
                // The block's return value is never consumed by the `yield`
                // desugar in v1 (yield is statement-position). A fresh var is
                // only ever constrained when `yield` happens to be the tail
                // expression, so methods that `yield` mid-body left it
                // unresolved and tripped `could not infer type for __block`.
                // Fix it to Unit: every `.rx` block-method (each, builder
                // DSLs) discards the block value; combinators that DO use it
                // take explicit `f: any Fn[...]` params, not `yield`.
                ret: Box::new(Ty::Unit),
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
                default: None,
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
                    default: p.default.clone(),
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

        // Register the function name in the enclosing scope (not the function scope we just popped).
        let actual_name = if let Some(parent) = parent {
            self.bind_method_name(&f.name, parent, def_id, f.span.clone())
        } else {
            self.bind_callable_name(&f.name, def_id, f.span.clone())
        };

        HirFuncDef {
            def_id,
            name: actual_name,
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
                //
                // Multi-segment case: `use rondo.Rondo.Router` for a
                // dep that wraps its types in `module Rondo`. The
                // package prefix flat-merges so `Rondo` is in scope
                // as a top-level Module — walk from there through
                // the remaining segments via `resolve_use_path_from`
                // (the same helper the happy path uses).
                if path.len() >= 2 {
                    if let Some(mid) = self
                        .scopes
                        .lookup_type(&path[1])
                        .or_else(|| self.scopes.lookup(&path[1]))
                    {
                        if let Some(final_id) =
                            self.resolve_use_path_from(mid, &path[2..], use_decl)
                        {
                            match &use_decl.kind {
                                ast::UseKind::Simple => {
                                    let import_name = path.last().unwrap().clone();
                                    self.scopes.insert(import_name.clone(), final_id);
                                    self.scopes.insert_type(import_name, final_id);
                                }
                                ast::UseKind::Alias(alias) => {
                                    self.scopes.insert(alias.clone(), final_id);
                                    self.scopes.insert_type(alias.clone(), final_id);
                                }
                                ast::UseKind::Group(names) => {
                                    for name in names {
                                        if let Some(cid) =
                                            self.resolve_child_in_def(final_id, name, use_decl)
                                        {
                                            self.scopes.insert(name.clone(), cid);
                                            self.scopes.insert_type(name.clone(), cid);
                                        }
                                    }
                                }
                            }
                            return;
                        }
                    }
                }
                let last = path.last().unwrap();
                let fallback = self
                    .scopes
                    .lookup_type(last)
                    .or_else(|| self.scopes.lookup(last));
                if let Some(final_id) = fallback {
                    // Symbol already in scope via flat-merge. Re-bind
                    // under the requested name (Simple/Alias/Group)
                    // so the explicit `use` still produces a usable
                    // local alias.
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
                        // Fallback: pass-1 doesn't populate Module.items
                        // for nested classes/structs declared inside the
                        // module body — only the QUALIFIED type_registry
                        // entry (`<module>.<child>`) gets the DefId. Look
                        // there so `use rondo.Rondo.Router` walks the
                        // last hop via the qualified key.
                        // Pin: `docs/rondo_v1_blockers.md` B12 follow-up.
                        let qualified = format!("{}.{}", def.name, name);
                        if let Some(id) = self.type_registry.get(&qualified).copied() {
                            return Some(id);
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

        let item_ids: Vec<DefId> = items
            .iter()
            .filter_map(|item| match item {
                HirItem::Function(f) => Some(f.def_id),
                HirItem::Class(c) => Some(c.def_id),
                HirItem::Struct(s) => Some(s.def_id),
                HirItem::Enum(e) => Some(e.def_id),
                HirItem::Mixin(t) => Some(t.def_id),
                HirItem::Module(m) => Some(m.def_id),
                HirItem::TypeAlias(t) => Some(t.def_id),
                HirItem::Newtype(n) => Some(n.def_id),
                HirItem::Const(c) => Some(c.def_id),
                HirItem::Impl(_) => None,
            })
            .collect();
        if let Some(def) = self.symbols.get_mut(def_id) {
            if let DefKind::Module {
                items: module_items,
            } = &mut def.kind
            {
                for item_id in item_ids {
                    if !module_items.contains(&item_id) {
                        module_items.push(item_id);
                    }
                }
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
