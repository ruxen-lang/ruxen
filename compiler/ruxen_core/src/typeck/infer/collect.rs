//! Method-call resolution, iterator / `collect` typing, field lookup,
//! and generic substitution.
//!
//! This is the "type-druxen dispatch" side of inference — given a value
//! of some type, find the field/method that matches a name (looking
//! through inheritance + mixin bounds), and substitute the receiver's
//! generic args into the result type so callers see fully-resolved
//! types.

use crate::diagnostics::Diagnostic;
use crate::hir::nodes::*;
use crate::hir::types::Ty;
use crate::lexer::token::Span;
use crate::parser::ast;
use crate::resolve::symbols::DefKind;

use super::super::unify::unify;
use super::InferenceEngine;

impl<'a> InferenceEngine<'a> {
    pub(super) fn resolve_method_call(
        &mut self,
        obj_ty: &Ty,
        method_name: &str,
        args: &[HirExpr],
        span: &Span,
    ) -> Ty {
        // A `.call(...)` invocation on a Fn/FnMut/FnOnce-typed receiver
        // (used for closure invocation and `yield` desugaring) unifies
        // the arguments with the function's parameter types and returns
        // the declared return type.  This binds fresh inference vars in
        // the receiver's `Ty::Fn { params, ret }` to concrete types.
        if method_name == "call" {
            let derefed = match obj_ty {
                Ty::Ref(inner)
                | Ty::RefMut(inner)
                | Ty::RefLifetime(_, inner)
                | Ty::RefMutLifetime(_, inner) => inner.as_ref(),
                other => other,
            };
            if let Ty::Fn { params, ret } | Ty::FnMut { params, ret } | Ty::FnOnce { params, ret } =
                derefed
            {
                for (arg, param_ty) in args.iter().zip(params.iter()) {
                    let _ = unify(&arg.ty, param_ty, self.ctx, span);
                }
                return self.ctx.resolve(ret);
            }
            // Dyn-erased closure receivers — `any Fn(T) -> U` shows up
            // as `Ty::AnyMixin([MixinRef{name:"Fn", generic_args:[Fn{...}]}])`
            // (and likewise `some Fn(...)` → `Ty::SomeMixin`). Peel the
            // single `Fn` / `FnMut` / `FnOnce` bound's first generic
            // arg, which carries the underlying `Ty::Fn { params, ret }`,
            // and treat the call like a direct Fn dispatch above.
            // Without this, `let r = closure.(arg)` on a dyn-typed
            // receiver leaves `r`'s type at `Ty::Infer`, blowing up
            // every subsequent `r.field` / `r.method` at codegen with
            // an unresolved `?T_<m>` symbol.
            if let Ty::AnyMixin(bounds) | Ty::SomeMixin(bounds) = derefed {
                for bound in bounds {
                    if matches!(bound.name.as_str(), "Fn" | "FnMut" | "FnOnce") {
                        if let Some(Ty::Fn { params, ret })
                        | Some(Ty::FnMut { params, ret })
                        | Some(Ty::FnOnce { params, ret }) = bound.generic_args.first()
                        {
                            for (arg, param_ty) in args.iter().zip(params.iter()) {
                                let _ = unify(&arg.ty, param_ty, self.ctx, span);
                            }
                            return self.ctx.resolve(ret);
                        }
                    }
                }
            }
        }

        // Handle built-in methods on known types
        if let Some(ret) = self.builtin_method_type(obj_ty, method_name, args, span) {
            return ret;
        }

        // Look up in trait resolver
        if let Some(sig) =
            self.traits
                .lookup_method_with_args(obj_ty, method_name, args, self.symbols)
        {
            return self.wrap_async_return(&sig);
        }

        // Method call on a generic type parameter `T: Trait + Trait`
        // or `impl Trait` / `dyn Trait`: search the trait bounds for the
        // declaring trait and report ambiguity when multiple bounds match.
        if let Some(ret) = self.lookup_on_type_param_bounds(obj_ty, method_name, args, span) {
            return ret;
        }

        // For inference variables, we can't resolve yet — return a fresh var
        if obj_ty.is_infer() || obj_ty.is_error() {
            return self.ctx.fresh_type_var();
        }

        // `.await` is postfix syntax, not a real method: on a synthesised
        // future class (e.g. `__FetchUserFuture`) it is resolved by the
        // async-lowering elision path long after typeck, so it never has
        // a method signature here. Leave it to that later phase — erroring
        // would reject every `expr.await` whose future is not the bridge
        // `Future` class.
        if method_name == "await" {
            return self.ctx.fresh_type_var();
        }

        // Method not found. For a scalar value-primitive receiver
        // (numeric, Bool, Char) an unknown method is definitively an
        // error: these types have no class shell and no user-defined
        // method surface, so the call would mangle to `<Type>_<method>`
        // (e.g. `Int_to_f`) with no matching runtime symbol — a link
        // error in AOT builds and a hard JIT panic in the REPL
        // (`can't resolve symbol Int_to_f`). Emit a clean, source-spanned
        // diagnostic instead, mirroring the field-access path in
        // `infer/expr.rs` so `a.bogus` and `a.bogus()` fail identically.
        //
        // We deliberately do NOT error for class / struct / enum /
        // collection / generic receivers: methods on those are resolved
        // by later phases (e.g. `Array[T]_get_var` → `ruxen_vec_get_opt`,
        // user methods on generic classes like `Repository[Todo]`), which
        // the lenient fresh-var fallback below still feeds.
        let resolved = self.ctx.resolve(obj_ty);
        if resolved.is_numeric() || matches!(resolved, Ty::Bool | Ty::Char) {
            self.error(
                format!("no method `{method_name}` on type `{resolved}`"),
                span,
            );
            return Ty::Error;
        }

        // Method not found — but don't error for common chaining patterns
        // on richer receiver types (resolved by later phases).
        self.ctx.fresh_type_var()
    }

    pub(super) fn infer_iter_collect_type(
        &mut self,
        obj_ty: &Ty,
        generic_args: &[Ty],
        span: &Span,
    ) -> Ty {
        if generic_args.len() != 1 {
            self.diagnostics.push(Diagnostic::error_with_code(
                "`collect` requires exactly one target type: `iter.collect[Array[T]]`".to_string(),
                span.clone(),
                "E0700",
            ));
            return Ty::Error;
        }

        let Some(item_ty) = self.iter_item_ty(obj_ty) else {
            self.diagnostics.push(Diagnostic::error_with_code(
                format!("`collect` is only defined on iterator values; got `{obj_ty}`"),
                span.clone(),
                "E0700",
            ));
            return Ty::Error;
        };

        let target = self.ctx.resolve(&generic_args[0]);
        if !self.collect_target_compatible(&target, &item_ty, span) {
            return Ty::Error;
        }
        target
    }

    pub(super) fn iter_item_ty(&self, ty: &Ty) -> Option<Ty> {
        match ty {
            Ty::Class { name, generic_args } if name.ends_with("Iter") => {
                generic_args.first().cloned()
            }
            Ty::Ref(inner)
            | Ty::RefMut(inner)
            | Ty::RefLifetime(_, inner)
            | Ty::RefMutLifetime(_, inner) => self.iter_item_ty(inner),
            _ => None,
        }
    }

    pub(super) fn collect_target_compatible(
        &mut self,
        target: &Ty,
        item_ty: &Ty,
        span: &Span,
    ) -> bool {
        match target {
            Ty::Array(elem) => {
                let ok = unify(elem.as_ref(), item_ty, self.ctx, span).is_ok();
                if !ok {
                    self.diagnostics.push(Diagnostic::error_with_code(
                        format!(
                            "`collect[{}]` expects iterator items of type `{}`; got `{}`",
                            target,
                            elem.as_ref(),
                            item_ty
                        ),
                        span.clone(),
                        "E0700",
                    ));
                }
                ok
            }
            Ty::String | Ty::Str => {
                let resolved = self.ctx.resolve(item_ty);
                let ok = matches!(resolved, Ty::String | Ty::Str);
                if !ok {
                    self.diagnostics.push(Diagnostic::error_with_code(
                        format!(
                            "`collect[String]` expects iterator items of type `String` or `&str`; got `{}`",
                            resolved
                        ),
                        span.clone(),
                        "E0700",
                    ));
                }
                ok
            }
            Ty::Map(k, v) => {
                let resolved = self.ctx.resolve(item_ty);
                let ok = match resolved {
                    Ty::Tuple(ref elems) if elems.len() == 2 => {
                        unify(k.as_ref(), &elems[0], self.ctx, span).is_ok()
                            && unify(v.as_ref(), &elems[1], self.ctx, span).is_ok()
                    }
                    _ => false,
                };
                if !ok {
                    self.diagnostics.push(Diagnostic::error_with_code(
                        format!(
                            "`collect[{}]` expects iterator items of type `(K, V)`; got `{}`",
                            target, item_ty
                        ),
                        span.clone(),
                        "E0700",
                    ));
                }
                ok
            }
            Ty::Set(elem) => {
                let ok = unify(elem.as_ref(), item_ty, self.ctx, span).is_ok();
                if !ok {
                    self.diagnostics.push(Diagnostic::error_with_code(
                        format!(
                            "`collect[{}]` expects iterator items of type `{}`; got `{}`",
                            target,
                            elem.as_ref(),
                            item_ty
                        ),
                        span.clone(),
                        "E0700",
                    ));
                }
                ok
            }
            other => {
                self.diagnostics.push(Diagnostic::error_with_code(
                    format!(
                        "`collect[{other}]` is not supported yet; use Array, String, Map, or Set"
                    ),
                    span.clone(),
                    "E0700",
                ));
                false
            }
        }
    }

    /// If `ty` is (a reference to) a `TypeParam`, `impl Trait`, or `dyn Trait`,
    /// consult the trait bounds for a method named `name`. Reports an
    /// ambiguity diagnostic when more than one bound declares the method.
    pub(super) fn lookup_on_type_param_bounds(
        &mut self,
        ty: &Ty,
        name: &str,
        args: &[HirExpr],
        span: &Span,
    ) -> Option<Ty> {
        // Peel reference layers — a method call on `&T` / `&mut T` / `&'a T`
        // where `T: Mixin` must consult the same bound list as a direct
        // `T.method(...)` call. Without the peel, generic fns shaped like
        // `def foo[T: Hashable](a: &T) -> Int { a.hash_code }` fall through
        // every lookup branch and the body's return type stays a fresh var
        // (which unifies to `Unit` eventually), producing a misleading
        // "expected Int, found ()" against the declared signature. Pin:
        // `derive_hashable_dispatches_through_trait_bounds`.
        let mut cur = ty;
        while let Ty::Ref(inner)
        | Ty::RefMut(inner)
        | Ty::RefLifetime(_, inner)
        | Ty::RefMutLifetime(_, inner) = cur
        {
            cur = inner.as_ref();
        }
        let bounds: &[crate::hir::types::MixinRef] = match cur {
            Ty::TypeParam { bounds, .. } | Ty::SomeMixin(bounds) | Ty::AnyMixin(bounds) => {
                bounds.as_slice()
            }
            _ => return None,
        };
        if bounds.is_empty() {
            return None;
        }
        match self.traits.lookup_method_on_bounds(bounds, name, args) {
            Ok(Some(sig)) => Some(self.wrap_async_return(&sig)),
            Ok(None) => None,
            Err(providers) => {
                self.error(
                    format!(
                        "ambiguous method `{}`: provided by multiple mixin bounds ({}) — \
                         disambiguate with `Mixin.{}(…)`",
                        name,
                        providers.join(", "),
                        name,
                    ),
                    span,
                );
                Some(Ty::Error)
            }
        }
    }

    pub(super) fn builtin_method_type(
        &mut self,
        ty: &Ty,
        method: &str,
        args: &[HirExpr],
        span: &Span,
    ) -> Option<Ty> {
        super::super::method_resolvers::builtin_method_type(self, ty, method, args, span)
    }

    /// Zero-Rust-stdlib bridge (Phase B / M3+M4), Option C "delegate":
    /// resolve a method on a BUILTIN `Ty` head (`String`/`&str`/`Array`/
    /// `Set`/`Map`/scalars) from its `.rx` method-home class, returning the
    /// fully-substituted surface type. This is the SOURCE OF TRUTH for the
    /// builtin-head delegating resolver in `typeck/method_resolvers`; the
    /// resolver arm itself carries zero hardcoded method knowledge.
    ///
    /// Called from the resolver pipeline (the inference-order-tolerant site
    /// at `resolve_method_call` line 77, BEFORE the `is_infer` fallback) so
    /// it participates in the same inference fixpoint the old hardcoded arms
    /// did — fixing the interpolation-ordering regression that a
    /// post-fallback (line 82) lookup hit.
    ///
    /// Mirrors the trait-lookup branch (line 82-86) PLUS the call-site
    /// generic substitution (expr.rs:505): `lookup_method_with_args` →
    /// `substitute_generics_in_return` (so `Array[Int].pop` yields
    /// `Option[Int]`, not the declared `Option[T]`) → async wrap.
    pub(in crate::typeck) fn bridge_builtin_method(
        &mut self,
        ty: &Ty,
        method: &str,
        args: &[HirExpr],
    ) -> Option<Ty> {
        let sig = self
            .traits
            .lookup_method_with_args(ty, method, args, self.symbols)?;
        let raw = self.wrap_async_return(&sig);
        // Receiver-generic substitution (`Array[Int].pop` → `Option[Int]`)
        // first, then METHOD-LEVEL generic harvesting from the args. The
        // latter is a no-op for every stdlib method declared today (none
        // carry their own `generic_params`), but it is the load-bearing
        // step for the `.rx` closure combinators: `map[U](f: any Fn[Fn(T)
        // -> U]) -> Array[U]` binds `U` from the closure argument's return
        // type so the call expression's type is `Array[<closure-ret>]`
        // rather than the un-substituted `Array[U]`.
        let receiver_subst = self.substitute_generics_in_return(ty, &raw);
        Some(self.harvest_and_subst_generics(&sig, args, &receiver_subst))
    }

    pub(super) fn infer_index_ty(&self, obj_ty: &Ty) -> Ty {
        match obj_ty {
            Ty::Array(elem) => *elem.clone(),
            Ty::FixedArray(elem, _) => *elem.clone(),
            // `self[i]` INSIDE a migrated `class Array[T]` stdlib method
            // body: `self` is typed as the FFI-shell `Ty::Class { name:
            // "Array" | "Vec", generic_args: [T] }`, not `Ty::Array`. Yield
            // the first generic arg as the element type so `.rx` combinator
            // bodies can index their receiver (mirrors the lower.rs Index
            // dispatch's `is_indexable_vec_ty`).
            Ty::Class { name, generic_args }
                if {
                    let base = name.split('[').next().unwrap_or(name);
                    matches!(base, "Vec" | "Array")
                } =>
            {
                generic_args.first().cloned().unwrap_or(Ty::Error)
            }
            // `m[&k]` panics on missing keys (mirrors Rust's `Index for
            // HashMap`); the runtime helper `ruxen_hash_index` returns the
            // raw value slot rather than an Option, so the surface type is
            // V directly. See lower.rs Index handler for the dispatch.
            Ty::Map(_, v) => *v.clone(),
            // Reference-to-HashMap also indexable (e.g. `&map[k]`).
            Ty::Ref(inner) | Ty::RefMut(inner) => self.infer_index_ty(inner),
            Ty::Tuple(elems) => {
                // Dynamic index — can't know at compile time
                if elems.is_empty() {
                    Ty::Error
                } else {
                    elems[0].clone()
                }
            }
            Ty::String | Ty::Str => Ty::Char,
            _ => Ty::Error,
        }
    }

    pub(super) fn lookup_field(&self, type_name: &str, field_name: &str) -> Option<(Ty, usize)> {
        for def in self.symbols.iter() {
            if def.name == field_name {
                if let DefKind::Field { parent, ty, index } = &def.kind {
                    if let Some(parent_def) = self.symbols.get(*parent) {
                        if parent_def.name == type_name {
                            return Some((ty.clone(), *index));
                        }
                    }
                }
            }
        }
        None
    }

    /// Look up a field by name, also checking parent classes in the inheritance chain.
    pub(super) fn lookup_field_with_parents(
        &self,
        type_name: &str,
        field_name: &str,
    ) -> Option<(Ty, usize)> {
        // First try the type itself
        if let Some(result) = self.lookup_field(type_name, field_name) {
            return Some(result);
        }
        // Walk the parent chain
        for def in self.symbols.iter() {
            if def.name == type_name {
                if let DefKind::Class { info } = &def.kind {
                    if let Some(parent_id) = info.parent {
                        if let Some(parent_def) = self.symbols.get(parent_id) {
                            return self.lookup_field_with_parents(&parent_def.name, field_name);
                        }
                    }
                }
            }
        }
        None
    }

    /// Look up a user-defined method on a class (or its parents) and return its return type.
    pub(in crate::typeck) fn lookup_class_method_return(
        &self,
        type_name: &str,
        method_name: &str,
    ) -> Option<Ty> {
        for def in self.symbols.iter() {
            if def.name == method_name {
                if let DefKind::Method { parent, signature } = &def.kind {
                    if let Some(parent_def) = self.symbols.get(*parent) {
                        if parent_def.name == type_name {
                            return Some(self.ctx.resolve(&signature.return_ty));
                        }
                    }
                }
            }
        }
        // Walk the parent chain
        for def in self.symbols.iter() {
            if def.name == type_name {
                if let DefKind::Class { info } = &def.kind {
                    if let Some(parent_id) = info.parent {
                        if let Some(parent_def) = self.symbols.get(parent_id) {
                            return self.lookup_class_method_return(&parent_def.name, method_name);
                        }
                    }
                }
            }
        }
        None
    }

    fn method_name_matches(def_name: &str, method_name: &str) -> bool {
        def_name == method_name || def_name.starts_with(&format!("{}__overload", method_name))
    }

    fn method_accepts_arg_count(&self, method_id: DefId, argc: usize) -> bool {
        let Some(def) = self.symbols.get(method_id) else {
            return false;
        };
        let DefKind::Method { signature, .. } = &def.kind else {
            return false;
        };
        let required = signature
            .params
            .iter()
            .filter(|p| p.default.is_none())
            .count();
        argc >= required && argc <= signature.params.len()
    }

    /// Effective argument count for method selection: the positional
    /// `args` plus any trailing `do…end` block, which satisfies the
    /// method's final (closure) parameter slot. A call like
    /// `b.probe do |x| … end` parses the block separately from `args`,
    /// so without this the closure parameter is invisible to arity
    /// selection and the method is rejected — degrading the call's
    /// return type to a fresh inference var (`Infer`). See
    /// `tests/release-e2e/cases/100_closure_method_heap_return.rx`.
    pub(super) fn effective_argc(args: &[HirExpr], has_block: bool) -> usize {
        args.len() + usize::from(has_block)
    }

    fn method_accepts_args(&self, method_id: DefId, args: &[HirExpr], has_block: bool) -> bool {
        let Some(def) = self.symbols.get(method_id) else {
            return false;
        };
        let DefKind::Method { signature, .. } = &def.kind else {
            return false;
        };
        if !self.method_accepts_arg_count(method_id, Self::effective_argc(args, has_block)) {
            return false;
        }
        args.iter()
            .zip(signature.params.iter())
            .all(|(arg, param)| {
                let arg_ty = self.ctx.resolve(&arg.ty);
                let param_ty = self.ctx.resolve(&param.ty);
                arg_ty.is_infer()
                    || arg_ty.is_error()
                    || arg_ty == param_ty
                    || matches!((&arg_ty, &param_ty), (Ty::Str, Ty::String))
                    || matches!(
                        (&arg_ty, &param_ty),
                        (Ty::Ref(a), Ty::Ref(b)) if matches!((&**a, &**b), (Ty::Str, Ty::String))
                    )
                    || matches!(&param_ty, Ty::Ref(inner) | Ty::RefMut(inner) if **inner == arg_ty)
            })
    }

    fn class_method_candidates(&self, type_name: &str, method_name: &str) -> Vec<DefId> {
        self.symbols
            .iter()
            .filter_map(|def| {
                if !Self::method_name_matches(&def.name, method_name) {
                    return None;
                }
                let DefKind::Method { parent, .. } = &def.kind else {
                    return None;
                };
                let parent_def = self.symbols.get(*parent)?;
                (parent_def.name == type_name).then_some(def.id)
            })
            .collect()
    }

    fn find_class_def(&self, type_name: &str) -> Option<DefId> {
        self.symbols.iter().find_map(|def| {
            if def.name == type_name && matches!(def.kind, DefKind::Class { .. }) {
                Some(def.id)
            } else {
                None
            }
        })
    }

    fn parent_class_name(&self, type_name: &str) -> Option<String> {
        let class_id = self.find_class_def(type_name)?;
        let class_def = self.symbols.get(class_id)?;
        let DefKind::Class { info } = &class_def.kind else {
            return None;
        };
        let parent_id = info.parent?;
        self.symbols
            .get(parent_id)
            .map(|parent| parent.name.clone())
    }

    fn select_method_candidate_strict(
        &self,
        candidates: &[DefId],
        args: &[HirExpr],
        has_block: bool,
    ) -> Option<DefId> {
        let effective_argc = Self::effective_argc(args, has_block);
        candidates
            .iter()
            .copied()
            .filter(|candidate| self.method_accepts_args(*candidate, args, has_block))
            .find(|candidate| {
                self.symbols
                    .get(*candidate)
                    .and_then(|def| match &def.kind {
                        DefKind::Method { signature, .. } => {
                            Some(signature.params.len() == effective_argc)
                        }
                        _ => None,
                    })
                    .unwrap_or(false)
            })
            .or_else(|| {
                candidates
                    .iter()
                    .copied()
                    .find(|candidate| self.method_accepts_args(*candidate, args, has_block))
            })
    }

    fn select_method_candidate_arity(
        &self,
        candidates: &[DefId],
        args: &[HirExpr],
        has_block: bool,
    ) -> Option<DefId> {
        let effective_argc = Self::effective_argc(args, has_block);
        candidates
            .iter()
            .copied()
            .find(|candidate| self.method_accepts_arg_count(*candidate, effective_argc))
    }

    pub(super) fn select_class_method(
        &self,
        type_name: &str,
        method_name: &str,
        args: &[HirExpr],
        has_block: bool,
    ) -> Option<DefId> {
        // First walk the inheritance chain looking for a candidate whose
        // parameter TYPES accept the call args. This prevents a child's
        // unrelated overload (same name, wrong param types) from masking
        // an inherited overload that actually matches. Only after no
        // ancestor has a strict match do we fall back to arity-only
        // matching, again walking the chain from this class up.
        //
        // `has_block` carries whether the call site supplied a trailing
        // `do…end` block; it occupies the method's final (closure)
        // parameter slot and is counted toward arity by `effective_argc`.
        self.select_class_method_strict(type_name, method_name, args, has_block)
            .or_else(|| self.select_class_method_arity(type_name, method_name, args, has_block))
    }

    fn select_class_method_strict(
        &self,
        type_name: &str,
        method_name: &str,
        args: &[HirExpr],
        has_block: bool,
    ) -> Option<DefId> {
        let candidates = self.class_method_candidates(type_name, method_name);
        if let Some(selected) = self.select_method_candidate_strict(&candidates, args, has_block) {
            return Some(selected);
        }
        let parent = self.parent_class_name(type_name)?;
        self.select_class_method_strict(&parent, method_name, args, has_block)
    }

    fn select_class_method_arity(
        &self,
        type_name: &str,
        method_name: &str,
        args: &[HirExpr],
        has_block: bool,
    ) -> Option<DefId> {
        let candidates = self.class_method_candidates(type_name, method_name);
        if let Some(selected) = self.select_method_candidate_arity(&candidates, args, has_block) {
            return Some(selected);
        }
        let parent = self.parent_class_name(type_name)?;
        self.select_class_method_arity(&parent, method_name, args, has_block)
    }

    fn default_ast_to_hir(&mut self, default: &ast::Expr) -> Option<HirExpr> {
        let ty = match &default.kind {
            ast::ExprKind::IntLiteral(_, _) => Ty::Int,
            ast::ExprKind::FloatLiteral(_, _) => Ty::Float,
            ast::ExprKind::StringLiteral(_) | ast::ExprKind::InterpolatedString(_) => Ty::String,
            ast::ExprKind::CharLiteral(_) => Ty::Char,
            ast::ExprKind::BoolLiteral(_) => Ty::Bool,
            ast::ExprKind::UnitLiteral => Ty::Unit,
            _ => return None,
        };
        Some(HirExpr {
            kind: match &default.kind {
                ast::ExprKind::IntLiteral(v, _) => HirExprKind::IntLiteral(*v),
                ast::ExprKind::FloatLiteral(v, _) => HirExprKind::FloatLiteral(*v),
                ast::ExprKind::StringLiteral(v) => HirExprKind::StringLiteral(v.clone()),
                ast::ExprKind::InterpolatedString(_) => return None,
                ast::ExprKind::CharLiteral(v) => HirExprKind::CharLiteral(*v),
                ast::ExprKind::BoolLiteral(v) => HirExprKind::BoolLiteral(*v),
                ast::ExprKind::UnitLiteral => HirExprKind::UnitLiteral,
                _ => return None,
            },
            ty,
            span: default.span.clone(),
        })
    }

    pub(super) fn append_method_default_args(&mut self, method_id: DefId, args: &mut Vec<HirExpr>) {
        let defaults: Vec<ast::Expr> = self
            .symbols
            .get(method_id)
            .and_then(|def| match &def.kind {
                DefKind::Method { signature, .. } => Some(signature),
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
            if let Some(hir) = self.default_ast_to_hir(&default) {
                args.push(hir);
            }
        }
    }

    /// Infer the generic arguments of a class from the concrete types of a
    /// constructor call's arguments.  Walks the init method parameters and
    /// matches each TypeParam position with the corresponding argument's
    /// type. Returns `None` if the class has no generic params or if
    /// inference cannot cover every parameter.
    pub(super) fn infer_class_generics(
        &self,
        class_name: &str,
        args: &[HirExpr],
    ) -> Option<Vec<Ty>> {
        // Find the class definition.
        let generic_params: Vec<String> = {
            let mut result = None;
            for def in self.symbols.iter() {
                if def.name == class_name {
                    if let DefKind::Class { info } = &def.kind {
                        result = Some(
                            info.generic_params
                                .iter()
                                .map(|gp| gp.name.clone())
                                .collect(),
                        );
                        break;
                    }
                }
            }
            result?
        };
        if generic_params.is_empty() {
            return None;
        }

        // Find the init method's parameter types.
        let init_params: Vec<Ty> = {
            let mut result = None;
            for def in self.symbols.iter() {
                if def.name == "init" {
                    if let DefKind::Method { parent, signature } = &def.kind {
                        if let Some(parent_def) = self.symbols.get(*parent) {
                            if parent_def.name == class_name {
                                result =
                                    Some(signature.params.iter().map(|p| p.ty.clone()).collect());
                                break;
                            }
                        }
                    }
                }
            }
            result?
        };

        // Walk the parameters and capture TypeParam positions.
        let mut bindings: std::collections::HashMap<String, Ty> = std::collections::HashMap::new();
        for (param_ty, arg) in init_params.iter().zip(args.iter()) {
            Self::collect_typeparam_bindings(param_ty, &self.ctx.resolve(&arg.ty), &mut bindings);
        }

        // Assemble generic args in declaration order. If any is missing,
        // fall back to Error so downstream substitution leaves it alone.
        let mut out = Vec::with_capacity(generic_params.len());
        for gp in &generic_params {
            match bindings.get(gp) {
                Some(ty) => out.push(ty.clone()),
                None => return None,
            }
        }
        Some(out)
    }

    /// Walk a parameter type and an argument type in parallel, capturing
    /// every TypeParam name → concrete type binding encountered.
    fn collect_typeparam_bindings(
        param: &Ty,
        arg: &Ty,
        bindings: &mut std::collections::HashMap<String, Ty>,
    ) {
        match (param, arg) {
            (Ty::TypeParam { name, .. }, concrete) => {
                bindings
                    .entry(name.clone())
                    .or_insert_with(|| concrete.clone());
            }
            (Ty::Ref(a), Ty::Ref(b)) | (Ty::RefMut(a), Ty::RefMut(b)) => {
                Self::collect_typeparam_bindings(a, b, bindings);
            }
            (Ty::Ref(a), b) | (Ty::RefMut(a), b) => {
                Self::collect_typeparam_bindings(a, b, bindings);
            }
            (a, Ty::Ref(b)) | (a, Ty::RefMut(b)) => {
                Self::collect_typeparam_bindings(a, b, bindings);
            }
            (Ty::Array(a), Ty::Array(b)) => {
                Self::collect_typeparam_bindings(a, b, bindings);
            }
            (Ty::Option(a), Ty::Option(b)) => {
                Self::collect_typeparam_bindings(a, b, bindings);
            }
            _ => {}
        }
    }

    /// Substitute every `TypeParam { name: X }` in `ret_ty` with the
    /// corresponding generic argument from `obj_ty` (a `Ty::Class` or
    /// `Ty::Struct`).
    pub(super) fn substitute_generics_in_return(&self, obj_ty: &Ty, ret_ty: &Ty) -> Ty {
        // Peel references so `(&Array[Int]).pop` substitutes like
        // `Array[Int].pop` (the receiver of a `&self` method is borrowed).
        let peeled = match obj_ty {
            Ty::Ref(inner)
            | Ty::RefMut(inner)
            | Ty::RefLifetime(_, inner)
            | Ty::RefMutLifetime(_, inner) => inner.as_ref(),
            other => other,
        };
        // Zero-Rust-stdlib bridge (Phase B / M3): map the builtin generic
        // heads to the synthetic `(class-name, generic_args)` of their
        // `.rx` method-home class so the class's declared type params
        // (`class Array[T]` → `[T]`) substitute against the receiver's
        // concrete element type. Mirrors `MixinResolver::method_home_key`.
        let owned_synthetic: (String, Vec<Ty>);
        let (name, generic_args): (&String, &Vec<Ty>) = match peeled {
            Ty::Class { name, generic_args } | Ty::Struct { name, generic_args }
                if !generic_args.is_empty() =>
            {
                (name, generic_args)
            }
            Ty::Array(elem) => {
                owned_synthetic = ("Array".to_string(), vec![elem.as_ref().clone()]);
                (&owned_synthetic.0, &owned_synthetic.1)
            }
            Ty::Set(elem) => {
                owned_synthetic = ("Set".to_string(), vec![elem.as_ref().clone()]);
                (&owned_synthetic.0, &owned_synthetic.1)
            }
            Ty::Map(k, v) => {
                // `Ty::Map` Displays as `Hash[K, V]`; its method-home class
                // is `class Hash[K, V]` (map/src/lib.rx), so substitute
                // against that class's declared `[K, V]` params. Mirrors
                // `MixinResolver::method_home_key`'s `Ty::Map → "Hash"`.
                owned_synthetic = (
                    "Hash".to_string(),
                    vec![k.as_ref().clone(), v.as_ref().clone()],
                );
                (&owned_synthetic.0, &owned_synthetic.1)
            }
            // `Ty::Option`/`Ty::Result` home their methods on the builtin
            // `enum Option[T]` / `enum Result[T, E]` (registered by
            // `resolve/stdlib/{option,result}.rs`). Substitute against
            // those enums' declared params so `Option[Int].unwrap`
            // yields `Int` (not the declared `T`) and
            // `Result[Foo, IoError].unwrap` yields `Foo`. Mirrors
            // `MixinResolver::method_home_key`'s Option/Result arms.
            Ty::Option(inner) => {
                owned_synthetic = ("Option".to_string(), vec![inner.as_ref().clone()]);
                (&owned_synthetic.0, &owned_synthetic.1)
            }
            Ty::Result(ok, err) => {
                owned_synthetic = (
                    "Result".to_string(),
                    vec![ok.as_ref().clone(), err.as_ref().clone()],
                );
                (&owned_synthetic.0, &owned_synthetic.1)
            }
            _ => return ret_ty.clone(),
        };

        // Build a name→type map using the class's declared generic params.
        let class_params: Vec<String> = {
            let mut out = Vec::new();
            for def in self.symbols.iter() {
                if def.name == *name {
                    if let DefKind::Class { info } = &def.kind {
                        out = info
                            .generic_params
                            .iter()
                            .map(|gp| gp.name.clone())
                            .collect();
                        break;
                    }
                    if let DefKind::Struct { info } = &def.kind {
                        out = info
                            .generic_params
                            .iter()
                            .map(|gp| gp.name.clone())
                            .collect();
                        break;
                    }
                    // Builtin `enum Option[T]` / `enum Result[T, E]` —
                    // their method-home is the enum, so read the enum's
                    // declared params for the synthetic Option/Result
                    // heads above.
                    if let DefKind::Enum { info } = &def.kind {
                        out = info
                            .generic_params
                            .iter()
                            .map(|gp| gp.name.clone())
                            .collect();
                        break;
                    }
                }
            }
            out
        };
        if class_params.len() != generic_args.len() {
            return ret_ty.clone();
        }
        let subst: std::collections::HashMap<String, Ty> = class_params
            .into_iter()
            .zip(generic_args.iter().cloned())
            .collect();
        Self::subst_ty(ret_ty, &subst)
    }

    /// Substitute every `TypeParam` named in `subst` with its bound type,
    /// recursing through ALL nested `Ty` children via `Ty::map_inner`. The only
    /// special arm is `TypeParam`; everything else delegates to the exhaustive
    /// fold, so reference layers (incl. `&'a`/`&'a mut`), `Map`/`Set`/`FixedArray`,
    /// `Fn`/`Newtype`/`Alias` etc. are all covered automatically. See
    /// docs/specs/types/typed_ffi_returns.spec.md.
    pub(super) fn subst_ty(ty: &Ty, subst: &std::collections::HashMap<String, Ty>) -> Ty {
        match ty {
            Ty::TypeParam { name, .. } => subst.get(name).cloned().unwrap_or_else(|| ty.clone()),
            // `Ty::map_inner` treats `Some/AnyMixin` as LEAVES (their
            // children are `MixinRef`, not `Ty`), so a generic param nested
            // inside a mixin bound's generic args — e.g. the `T`/`U` in
            // `any Fn[Fn(T) -> U]` — would NOT be substituted by the fold
            // below. The `.rx` closure combinators carry exactly that shape
            // in their closure parameter, and receiver-generic substitution
            // (`T → Int`) must reach the inner `Fn(T)` so the closure param
            // seeds concretely. Substitute through each bound's generic args
            // explicitly here.
            Ty::AnyMixin(bounds) | Ty::SomeMixin(bounds) => {
                let new_bounds: Vec<crate::hir::types::MixinRef> = bounds
                    .iter()
                    .map(|b| crate::hir::types::MixinRef {
                        name: b.name.clone(),
                        generic_args: b
                            .generic_args
                            .iter()
                            .map(|g| Self::subst_ty(g, subst))
                            .collect(),
                    })
                    .collect();
                match ty {
                    Ty::AnyMixin(_) => Ty::AnyMixin(new_bounds),
                    _ => Ty::SomeMixin(new_bounds),
                }
            }
            other => other
                .clone()
                .map_inner(&mut |child| Self::subst_ty(child, subst)),
        }
    }

    /// Generic free-function call inference: harvest `{ type_param → concrete }`
    /// bindings by structurally matching each declared formal parameter type
    /// (which may contain the function's own `TypeParam`s) against the
    /// resolved actual argument type. One-directional — only names in
    /// `param_names` (the function's own generic params) are bound; every
    /// other position is walked for nested bindings. Mirrors the
    /// `subst_generics_in_return` direction so the two stay consistent.
    ///
    /// Used by the `FnCall` handler so `expect[T](actual: T) -> Matcher[T]`
    /// called as `expect(aString)` yields `{ T → String }`, making the call
    /// expression's type `Matcher[String]` — which the MIR class-mono pass
    /// then specializes.
    pub(super) fn bind_type_params_from_args(
        param_names: &std::collections::HashSet<String>,
        formal: &Ty,
        actual: &Ty,
        out: &mut std::collections::HashMap<String, Ty>,
    ) {
        // Peel matching reference layers on both sides so `&T` vs `&String`
        // binds `T → String`. If only one side is a reference, fall through
        // to the structural match below (the `TypeParam` arm still binds the
        // whole actual, which is the safe over-approximation).
        match (formal, actual) {
            (Ty::Ref(f), Ty::Ref(a))
            | (Ty::RefMut(f), Ty::RefMut(a))
            | (Ty::RefLifetime(_, f), Ty::RefLifetime(_, a))
            | (Ty::RefMutLifetime(_, f), Ty::RefMutLifetime(_, a)) => {
                Self::bind_type_params_from_args(param_names, f, a, out);
                return;
            }
            // A `&T` / `&var T` formal matched against a non-reference actual
            // (callers commonly pass an owned value where the signature
            // borrows): bind `T` to the actual value type.
            (Ty::Ref(f), _) | (Ty::RefMut(f), _) => {
                Self::bind_type_params_from_args(param_names, f, actual, out);
                return;
            }
            _ => {}
        }

        match formal {
            Ty::TypeParam { name, .. } if param_names.contains(name) => {
                // Don't bind to a still-unresolved inference variable; that
                // would only restate the unknown and risk over-eager
                // substitution. Leave `T` free so the existing
                // `wrap_async_return` behaviour (TypeParam passthrough) is
                // preserved for return-only / turbofish cases.
                if !matches!(actual, Ty::Infer(_) | Ty::TypeParam { .. }) {
                    // First binding wins; consistent re-binding is a no-op.
                    out.entry(name.clone()).or_insert_with(|| actual.clone());
                }
            }
            Ty::Option(f) => {
                if let Ty::Option(a) = actual {
                    Self::bind_type_params_from_args(param_names, f, a, out);
                }
            }
            Ty::Array(f) => {
                if let Ty::Array(a) = actual {
                    Self::bind_type_params_from_args(param_names, f, a, out);
                }
            }
            Ty::Set(f) => {
                if let Ty::Set(a) = actual {
                    Self::bind_type_params_from_args(param_names, f, a, out);
                }
            }
            Ty::Map(fk, fv) => {
                if let Ty::Map(ak, av) = actual {
                    Self::bind_type_params_from_args(param_names, fk, ak, out);
                    Self::bind_type_params_from_args(param_names, fv, av, out);
                }
            }
            Ty::Result(fo, fe) => {
                if let Ty::Result(ao, ae) = actual {
                    Self::bind_type_params_from_args(param_names, fo, ao, out);
                    Self::bind_type_params_from_args(param_names, fe, ae, out);
                }
            }
            Ty::Tuple(fs) => {
                if let Ty::Tuple(as_) = actual {
                    if fs.len() == as_.len() {
                        for (f, a) in fs.iter().zip(as_.iter()) {
                            Self::bind_type_params_from_args(param_names, f, a, out);
                        }
                    }
                }
            }
            Ty::Class {
                name: fname,
                generic_args: fargs,
            } => {
                if let Ty::Class {
                    name: aname,
                    generic_args: aargs,
                } = actual
                {
                    if fname == aname && fargs.len() == aargs.len() {
                        for (f, a) in fargs.iter().zip(aargs.iter()) {
                            Self::bind_type_params_from_args(param_names, f, a, out);
                        }
                    }
                }
            }
            Ty::Struct {
                name: fname,
                generic_args: fargs,
            } => {
                if let Ty::Struct {
                    name: aname,
                    generic_args: aargs,
                } = actual
                {
                    if fname == aname && fargs.len() == aargs.len() {
                        for (f, a) in fargs.iter().zip(aargs.iter()) {
                            Self::bind_type_params_from_args(param_names, f, a, out);
                        }
                    }
                }
            }
            Ty::Enum {
                name: fname,
                generic_args: fargs,
            } => {
                if let Ty::Enum {
                    name: aname,
                    generic_args: aargs,
                } = actual
                {
                    if fname == aname && fargs.len() == aargs.len() {
                        for (f, a) in fargs.iter().zip(aargs.iter()) {
                            Self::bind_type_params_from_args(param_names, f, a, out);
                        }
                    }
                }
            }
            // Closure-typed formal parameter — the load-bearing arm for
            // the `.rx` closure combinators (`map[U](f: any Fn[Fn(T) ->
            // U])`). The function's own generic `U` lives in the closure's
            // RETURN position, so it can only be bound from the actual
            // closure argument's return type. Match the formal's params
            // and ret against the actual's structurally; the `TypeParam`
            // arm above binds `U` once it reaches the ret leaf.
            Ty::Fn {
                params: fparams,
                ret: fret,
            }
            | Ty::FnMut {
                params: fparams,
                ret: fret,
            }
            | Ty::FnOnce {
                params: fparams,
                ret: fret,
            } => {
                if let Ty::Fn {
                    params: aparams,
                    ret: aret,
                }
                | Ty::FnMut {
                    params: aparams,
                    ret: aret,
                }
                | Ty::FnOnce {
                    params: aparams,
                    ret: aret,
                } = actual
                {
                    if fparams.len() == aparams.len() {
                        for (f, a) in fparams.iter().zip(aparams.iter()) {
                            Self::bind_type_params_from_args(param_names, f, a, out);
                        }
                    }
                    Self::bind_type_params_from_args(param_names, fret, aret, out);
                }
            }
            // `any Fn[Fn(T) -> U]` / `some Fn[...]` formal — the surface
            // spelling the `.rx` combinators use. The underlying
            // `Ty::Fn { params, ret }` rides in the `Fn`/`FnMut`/`FnOnce`
            // bound's first generic arg (mirrors the `call` dispatch peel
            // at the top of `resolve_method_call`). Peel the bound and
            // recurse so the inner `Ty::Fn` arm above binds `U`. The
            // actual closure arg may itself be a bare `Ty::Fn` (a closure
            // literal) or an identically-wrapped mixin (a forwarded dyn
            // closure value) — both are handled by recursing with the
            // peeled formal against the un-peeled actual (the inner arm
            // peels the actual's wrapper too if present).
            Ty::AnyMixin(bounds) | Ty::SomeMixin(bounds) => {
                for bound in bounds {
                    if matches!(bound.name.as_str(), "Fn" | "FnMut" | "FnOnce") {
                        if let Some(inner) = bound.generic_args.first() {
                            let actual_inner = Self::peel_fn_mixin(actual);
                            Self::bind_type_params_from_args(param_names, inner, actual_inner, out);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Peel an `any Fn[Fn(...)]` / `some Fn[...]` wrapper down to the
    /// underlying `Ty::Fn { params, ret }` it carries. A bare `Ty::Fn`
    /// (closure literal) is returned unchanged. Used by
    /// `bind_type_params_from_args` so a closure ARGUMENT that is itself
    /// dyn-erased (a forwarded `any Fn` value) still exposes its
    /// `params`/`ret` for generic-param harvesting.
    fn peel_fn_mixin(ty: &Ty) -> &Ty {
        if let Ty::AnyMixin(bounds) | Ty::SomeMixin(bounds) = ty {
            for bound in bounds {
                if matches!(bound.name.as_str(), "Fn" | "FnMut" | "FnOnce") {
                    if let Some(inner) = bound.generic_args.first() {
                        return inner;
                    }
                }
            }
        }
        ty
    }

    /// Harvest `{generic_param → concrete}` bindings from (args × formal
    /// params) and substitute them into `ret`. This is the single driver
    /// shared by the `FnCall` path and the selected-method (`MethodCall`)
    /// path: both build a `param_names` set from `signature.generic_params`,
    /// match each formal param against the resolved actual arg via
    /// [`Self::bind_type_params_from_args`], and `subst_ty` the result into
    /// the return type. Extracting it removes the duplicated driver loop the
    /// two sites previously hand-rolled.
    ///
    /// `ret` is the (already receiver-substituted, async-wrapped) return type;
    /// when the signature declares no own type params, `ret` is returned
    /// unchanged.
    pub(super) fn harvest_and_subst_generics(
        &self,
        signature: &crate::resolve::symbols::FnSignature,
        args: &[HirExpr],
        ret: &Ty,
    ) -> Ty {
        if signature.generic_params.is_empty() {
            return ret.clone();
        }
        let param_names: std::collections::HashSet<String> = signature
            .generic_params
            .iter()
            .map(|gp| gp.name.clone())
            .collect();
        let mut bindings: std::collections::HashMap<String, Ty> = std::collections::HashMap::new();
        for (arg, param) in args.iter().zip(&signature.params) {
            let actual = self.ctx.resolve(&arg.ty);
            Self::bind_type_params_from_args(&param_names, &param.ty, &actual, &mut bindings);
        }
        if bindings.is_empty() {
            ret.clone()
        } else {
            Self::subst_ty(ret, &bindings)
        }
    }
}

#[cfg(test)]
mod subst_ty_tests {
    use super::*;
    use crate::hir::types::Ty;
    use std::collections::HashMap;

    fn subst1(ty: Ty, name: &str, to: Ty) -> Ty {
        let mut m = HashMap::new();
        m.insert(name.to_string(), to);
        InferenceEngine::subst_ty(&ty, &m)
    }

    #[test]
    fn substitutes_through_named_lifetime_ref() {
        // &'a T  ->  &'a Int   (was a no-op before the map_inner migration: bug #3)
        let tp = Ty::TypeParam {
            name: "T".into(),
            bounds: vec![],
        };
        let got = subst1(Ty::RefLifetime("a".into(), Box::new(tp)), "T", Ty::Int);
        assert_eq!(got, Ty::RefLifetime("a".into(), Box::new(Ty::Int)));
    }

    #[test]
    fn substitutes_through_map_value() {
        // Map[String, T] -> Map[String, Int]   (was a no-op before: bug #3)
        let tp = Ty::TypeParam {
            name: "T".into(),
            bounds: vec![],
        };
        let got = subst1(Ty::Map(Box::new(Ty::String), Box::new(tp)), "T", Ty::Int);
        assert_eq!(got, Ty::Map(Box::new(Ty::String), Box::new(Ty::Int)));
    }

    #[test]
    fn preserves_existing_class_generic_substitution() {
        // Characterization: the behaviour subst_ty ALREADY had must not regress.
        let tp = Ty::TypeParam {
            name: "T".into(),
            bounds: vec![],
        };
        let got = subst1(
            Ty::Class {
                name: "MutexGuard".into(),
                generic_args: vec![tp],
            },
            "T",
            Ty::Int,
        );
        assert_eq!(
            got,
            Ty::Class {
                name: "MutexGuard".into(),
                generic_args: vec![Ty::Int],
            }
        );
    }
}
