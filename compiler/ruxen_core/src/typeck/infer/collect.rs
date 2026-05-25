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

        // Method not found — but don't error for common chaining patterns
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
        loop {
            match cur {
                Ty::Ref(inner)
                | Ty::RefMut(inner)
                | Ty::RefLifetime(_, inner)
                | Ty::RefMutLifetime(_, inner) => cur = inner.as_ref(),
                _ => break,
            }
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

    pub(super) fn infer_index_ty(&self, obj_ty: &Ty) -> Ty {
        match obj_ty {
            Ty::Array(elem) => *elem.clone(),
            Ty::FixedArray(elem, _) => *elem.clone(),
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
    pub(super) fn lookup_class_method_return(
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

    fn method_accepts_args(&self, method_id: DefId, args: &[HirExpr]) -> bool {
        let Some(def) = self.symbols.get(method_id) else {
            return false;
        };
        let DefKind::Method { signature, .. } = &def.kind else {
            return false;
        };
        if !self.method_accepts_arg_count(method_id, args.len()) {
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

    fn select_method_candidate(&self, candidates: &[DefId], args: &[HirExpr]) -> Option<DefId> {
        candidates
            .iter()
            .copied()
            .filter(|candidate| self.method_accepts_args(*candidate, args))
            .find(|candidate| {
                self.symbols
                    .get(*candidate)
                    .and_then(|def| match &def.kind {
                        DefKind::Method { signature, .. } => {
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
                    .find(|candidate| self.method_accepts_args(*candidate, args))
            })
            .or_else(|| {
                candidates
                    .iter()
                    .copied()
                    .find(|candidate| self.method_accepts_arg_count(*candidate, args.len()))
            })
    }

    pub(super) fn select_class_method(
        &self,
        type_name: &str,
        method_name: &str,
        args: &[HirExpr],
    ) -> Option<DefId> {
        let candidates = self.class_method_candidates(type_name, method_name);
        if let Some(selected) = self.select_method_candidate(&candidates, args) {
            return Some(selected);
        }
        let parent = self.parent_class_name(type_name)?;
        self.select_class_method(&parent, method_name, args)
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
        let (name, generic_args) = match obj_ty {
            Ty::Class { name, generic_args } | Ty::Struct { name, generic_args }
                if !generic_args.is_empty() =>
            {
                (name, generic_args)
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

    fn subst_ty(ty: &Ty, subst: &std::collections::HashMap<String, Ty>) -> Ty {
        match ty {
            Ty::TypeParam { name, .. } => subst.get(name).cloned().unwrap_or_else(|| ty.clone()),
            Ty::Ref(inner) => Ty::Ref(Box::new(Self::subst_ty(inner, subst))),
            Ty::RefMut(inner) => Ty::RefMut(Box::new(Self::subst_ty(inner, subst))),
            Ty::Option(inner) => Ty::Option(Box::new(Self::subst_ty(inner, subst))),
            Ty::Array(inner) => Ty::Array(Box::new(Self::subst_ty(inner, subst))),
            // Typed FFI returns (docs/specs/types/typed_ffi_returns.spec.md):
            // recurse into nominal types so `MutexGuard[T]` substitutes to
            // `MutexGuard[Int]` when T is bound to Int from the receiver
            // (`m: Mutex[Int]; m.lock_raw -> MutexGuard[T]`). Without
            // this recursion, generic args inside `Ty::Class` /
            // `Ty::Struct` / `Ty::Enum` / `Ty::Result` / `Ty::Tuple`
            // pass through verbatim and the chain `g.get` reports
            // `T` instead of `Int`.
            Ty::Class { name, generic_args } => Ty::Class {
                name: name.clone(),
                generic_args: generic_args
                    .iter()
                    .map(|a| Self::subst_ty(a, subst))
                    .collect(),
            },
            Ty::Struct { name, generic_args } => Ty::Struct {
                name: name.clone(),
                generic_args: generic_args
                    .iter()
                    .map(|a| Self::subst_ty(a, subst))
                    .collect(),
            },
            Ty::Enum { name, generic_args } => Ty::Enum {
                name: name.clone(),
                generic_args: generic_args
                    .iter()
                    .map(|a| Self::subst_ty(a, subst))
                    .collect(),
            },
            Ty::Result(ok, err) => Ty::Result(
                Box::new(Self::subst_ty(ok, subst)),
                Box::new(Self::subst_ty(err, subst)),
            ),
            Ty::Tuple(elems) => Ty::Tuple(elems.iter().map(|e| Self::subst_ty(e, subst)).collect()),
            _ => ty.clone(),
        }
    }
}
