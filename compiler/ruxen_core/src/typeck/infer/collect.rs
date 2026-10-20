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

use super::super::unify::{can_coerce, unify};
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
            // Dyn-erased closure receivers (`any Fn(T) -> U`, `some Fn(...)`)
            // carry the function type in their `Fn` bound; without reading it
            // `let r = closure.(arg)` would leave `r` at `Ty::Infer` and every
            // later `r.field` / `r.method` would fail at codegen with an
            // unresolved `?T_<m>` symbol.
            if let Some((params, ret)) = obj_ty.peel_refs().callable_signature() {
                for (arg, param_ty) in args.iter().zip(params.iter()) {
                    let _ = unify(&arg.ty, param_ty, self.ctx, span);
                }
                return self.ctx.resolve(ret);
            }
        }

        // Handle built-in methods on known types
        if let Some(ret) = self.builtin_method_type(obj_ty, method_name, args, span) {
            return ret;
        }

        // Look up in trait resolver. This is the path a mixin-DEFAULT method
        // on a user class takes (`class Box include Thenable2` inheriting
        // `def my_then[U](f: Fn(&Self) -> U) -> U`) — it is not an own method,
        // so `select_class_method` never selects it. Run the SAME chain
        // `bridge_builtin_method` runs for builtin heads: receiver-generic
        // substitution (`Self` → the concrete class) THEN method-level generic
        // harvesting from the args, so `U` binds from the closure argument's
        // inferred return type. Without the harvest the call's type stays the
        // un-substituted `U`/`Array[U]` and any downstream use fails (`puts`
        // sees a bare `U`, an index yields `U`). Harvest is a no-op for a
        // method with no own `generic_params`, so ordinary trait-default
        // methods are unaffected.
        if let Some((sig, _)) =
            self.traits
                .lookup_method_with_args(obj_ty, method_name, args, self.symbols)
        {
            let raw = self.wrap_async_return(&sig);
            let receiver_subst = self.substitute_generics_in_return(obj_ty, &raw);
            return self.harvest_and_subst_generics(&sig, args, &receiver_subst);
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
            // The bridge `Future[T]` an `async def` call returns awaits to `T`.
            if let Ty::Class { name, generic_args } = obj_ty {
                if name == "Future" {
                    if let Some(output) = generic_args.first() {
                        return output.clone();
                    }
                }
            }
            return self.ctx.fresh_type_var();
        }

        // Method not found. For a scalar value-primitive receiver
        // (numeric, Bool, Char) — and for `String` / `&str`, whose entire
        // method surface is the `.rx` `class String` (already consulted via
        // `builtin_method_type` + `lookup_method_with_args` above) — an
        // unknown method is
        // definitively an error: these types have no further class shell and
        // no later-phase method surface, so the call would mangle to
        // `<Type>_<method>` (e.g. `Int_to_f`, `String_from`) with no matching
        // runtime symbol — a link error in AOT builds and a hard JIT panic in
        // the REPL. Emit a clean, source-spanned diagnostic instead, mirroring
        // the field-access path in `infer/expr.rs` so `a.bogus` and `a.bogus()`
        // fail identically. This is the seam that makes the DELETED
        // `String.from(...)` produce a clean "no method `from` on type
        // `String`" error rather than leaking a `?T…` symbol into codegen.
        let resolved = self.ctx.resolve(obj_ty);
        let is_str_head = matches!(resolved, Ty::String)
            || matches!(&resolved, Ty::Ref(inner) if matches!(**inner, Ty::String));
        if resolved.is_numeric() || matches!(resolved, Ty::Bool | Ty::Char) || is_str_head {
            self.error(
                format!(
                    "no method `{method_name}` on type `{resolved}`{}",
                    Self::predicate_suffix_hint(method_name)
                ),
                span,
            );
            return Ty::Error;
        }

        // The same holds for a receiver with a known home class: a nominal
        // class, struct or enum, or a builtin collection head whose methods
        // live in its `.rx` class. Its whole method surface was consulted
        // above, so an unknown name (`Array.from(123)`) is an error here, not
        // a fresh variable that unifies with anything and fails at link
        // time. A call without arguments may be a parenless member access,
        // so its message names fields too. The `.rx` spelling of a builtin
        // head (`self` inside `class Array[T]`, a `Ty::Class` named `Array`)
        // is excluded: its `self[i]` reaches here as `[]`, which only the
        // MIR index lowering knows.
        let known_home = match resolved.peel_refs() {
            Ty::Class { .. } => resolved.builtin_head().is_none(),
            Ty::Struct { .. }
            | Ty::Enum { .. }
            | Ty::Array(_)
            | Ty::Set(_)
            | Ty::Map(_, _)
            | Ty::Option(_)
            | Ty::Result(_, _) => true,
            _ => false,
        };
        // A name the home class does declare, at another arity, is a call
        // shape later phases still lower: the static spelling of an
        // instance method, `JoinHandle.join_raw(handle)`.
        let declared = known_home
            && self
                .symbols
                .methods_named(
                    &resolved.method_home(self.symbols.closure_shell_name()),
                    method_name,
                )
                .next()
                .is_some();
        if known_home && !declared {
            let member = if args.is_empty() {
                "field or method"
            } else {
                "method"
            };
            // A static call's receiver carries unbound fresh variables
            // (`Array[?T]`): name its home class instead.
            let shown = if resolved.any(&mut |t| t.is_infer()) {
                resolved.method_home(self.symbols.closure_shell_name())
            } else {
                resolved.to_string()
            };
            self.diagnostics.push(Diagnostic::error_with_code(
                format!(
                    "no {member} `{method_name}` on type `{shown}`{}",
                    Self::predicate_suffix_hint(method_name)
                ),
                span.clone(),
                "E0740",
            ));
            return Ty::Error;
        }

        // Method not found on a receiver with no home class (a function
        // value, a tuple): left to later phases.
        self.ctx.fresh_type_var()
    }

    /// The mixin default body a method call on `ty` resolves to, through
    /// the same lookups [`Self::resolve_method_call`] types it with: an
    /// included mixin's or the universal `Kernel` default
    /// (`lookup_method_with_args`), else a mixin bound of a type-parameter
    /// receiver (`lookup_method_on_bounds`). `None` when the call resolves to
    /// an own, impl or derived method, a bodiless requirement, nothing, or
    /// is a closure invocation. The caller records it in
    /// `MethodCall::resolved`, so every later stage reads this choice.
    pub(super) fn selected_default_method(
        &self,
        ty: &Ty,
        method_name: &str,
        args: &[HirExpr],
    ) -> Option<DefId> {
        let receiver = ty.peel_refs();
        if receiver.callable_signature().is_some() {
            return None;
        }
        if let Some((_, def)) =
            self.traits
                .lookup_method_with_args(receiver, method_name, args, self.symbols)
        {
            return def;
        }
        let bounds = self.receiver_bounds(receiver)?;
        self.traits
            .lookup_method_on_bounds(&bounds, method_name, args)
            .ok()
            .flatten()
            .and_then(|(_, def)| def)
    }

    /// The parameter types of the bodiless requirement a call on a
    /// mixin-bound receiver `ty` selects with `args`, the same choice
    /// `lookup_method_on_bounds` makes for typing it; `None` when the
    /// receiver has no bounds or the selected method has a default body
    /// (recorded as a def instead).
    pub(super) fn selected_bound_requirement(
        &self,
        ty: &Ty,
        method_name: &str,
        args: &[HirExpr],
    ) -> Option<Vec<Ty>> {
        let bounds = self.receiver_bounds(ty)?;
        match self
            .traits
            .lookup_method_on_bounds(&bounds, method_name, args)
        {
            Ok(Some((signature, None))) => {
                Some(signature.params.into_iter().map(|p| p.ty).collect())
            }
            _ => None,
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
        // e2e `2051_derived_ord_hashable_generic_bound`.
        let bounds = self.receiver_bounds(ty)?;
        if bounds.is_empty() {
            return None;
        }
        match self.traits.lookup_method_on_bounds(&bounds, name, args) {
            // A bound's `Self` (`mixin Clone; def clone -> Self`) is the
            // receiver's own type parameter.
            Ok(Some((sig, _))) => {
                let subst =
                    std::collections::HashMap::from([("Self".to_string(), ty.peel_refs().clone())]);
                Some(self.wrap_async_return(&sig).subst_params(&subst))
            }
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
        span: &Span,
    ) -> Option<Ty> {
        let (sig, _) = self
            .traits
            .lookup_method_with_args(ty, method, args, self.symbols)?;
        if sig.is_class_method {
            self.check_class_method_args(ty, method, &sig, args);
        }

        // Receiver-element bound seam: a `.rx` method whose `where` clause
        // bounds the receiver class's OWN generic (`class Array[T]`'s
        // `def sum -> Int where T: Add`) carries that bound as a synthetic
        // generic param in its signature (threaded by `resolve/funcs.rs`).
        // Bind those params to the receiver's concrete generic args
        // (`{T → Int}`) and run the SAME declared-bound enforcement the
        // construction / method-own-generic seams use. This is what closes
        // the historical `sum`/E0700 fork — `[1,2].sum` is fine, `["a"].sum`
        // emits E0700 (via `bound_diagnostic_code`'s `Add` arm). Only
        // bounded params fire, and the receiver class is the owner so the
        // preserved code is chosen correctly.
        if sig.generic_params.iter().any(|gp| !gp.bounds.is_empty()) {
            if let Some(bindings) = self.receiver_generic_bindings(ty) {
                let owner = Self::bound_owner_name(ty);
                self.check_generic_param_bounds(
                    &sig.generic_params,
                    &bindings,
                    owner.as_deref(),
                    span,
                );
            }
        }

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

    /// Each argument of a builtin head's class method must fit its declared
    /// parameter type, the receiver's generic args substituted
    /// (`Array.with_capacity("x")` is E0741). An argument may be passed
    /// through or to a borrow of its type, or coerce to it. Instance
    /// methods and user classes are not checked yet: the concurrency
    /// surface passes objects through `Int` slots (`arms.push(fut)` for
    /// `Race.any_of`, `Race.of(a, b)`).
    pub(super) fn check_class_method_args(
        &mut self,
        receiver: &Ty,
        method: &str,
        sig: &crate::resolve::symbols::FnSignature,
        args: &[HirExpr],
    ) {
        for (arg, param) in args.iter().zip(&sig.params) {
            let param_ty = self.substitute_generics_in_return(receiver, &param.ty);
            let arg_ty = self.ctx.resolve(&arg.ty);
            let fits = unify(&arg_ty, &param_ty, self.ctx, &arg.span).is_ok()
                || unify(
                    arg_ty.peel_refs(),
                    param_ty.peel_refs(),
                    self.ctx,
                    &arg.span,
                )
                .is_ok()
                || can_coerce(&arg_ty, &param_ty, self.ctx);
            if !fits {
                let param_ty = self.ctx.resolve(&param_ty);
                self.diagnostics.push(Diagnostic::error_with_code(
                    format!(
                        "argument `{}` of `{}` expects `{param_ty}`, found `{arg_ty}`",
                        param.name, method,
                    ),
                    arg.span.clone(),
                    "E0741",
                ));
            }
        }
    }

    pub(super) fn infer_index_ty(&self, obj_ty: &Ty) -> Ty {
        // References index like their target (e.g. `&map[k]`).
        match obj_ty.peel_refs() {
            Ty::FixedArray(elem, _) => *elem.clone(),
            // `m[&k]` panics on missing keys (mirrors Rust's `Index for
            // HashMap`); the runtime helper `ruxen_hash_index` returns the
            // raw value slot rather than an Option, so the surface type is
            // V directly. See lower.rs Index handler for the dispatch.
            Ty::Map(_, v) => *v.clone(),
            // An `Array`, in either spelling: `self[i]` INSIDE a migrated
            // `class Array[T]` stdlib method body types `self` as the
            // FFI-shell `Ty::Class { name: "Array", generic_args: [T] }`
            // (mirrors the MIR Index dispatch's `is_indexable_vec_ty`).
            // No index lowering otherwise (tuples included): the caller
            // reports E1128.
            other => match other.builtin_head() {
                Some(("Array", args)) => args.into_iter().next().unwrap_or(Ty::Error),
                _ => Ty::Error,
            },
        }
    }

    pub(super) fn lookup_field(&self, type_name: &str, field_name: &str) -> Option<(Ty, usize)> {
        for def in self.symbols.named(field_name) {
            if let DefKind::Field { parent, ty, index } = &def.kind {
                if let Some(parent_def) = self.symbols.get(*parent) {
                    if parent_def.name == type_name {
                        return Some((ty.clone(), *index));
                    }
                }
            }
        }
        None
    }

    /// Look up a field by name, also checking parent classes in the
    /// inheritance chain, and return its type plus its **absolute slot
    /// index** in `type_name`'s layout.
    ///
    /// Absolute, not class-local, is the whole point. A class is laid out
    /// ancestors-first — oldest ancestor's fields, then the next, then
    /// its own — which is what `alloc_size` (`mir/lower/emit.rs`) budgets
    /// and what `get_class_field_names` (`mir/lower/mod.rs`) already
    /// returns for `def init(@x:)` auto-assign. `DefKind::Field.index`,
    /// however, is CLASS-LOCAL: it restarts at 0 for every class. So
    /// returning it verbatim gave `class Derived < Base` — `Base { id }`,
    /// `Derived { tag }` — slot 0 for BOTH `id` and `tag`, and `@tag = t`
    /// silently overwrote the inherited `id` while the last slot stayed
    /// uninitialised. `16_inheritance.rx` did not catch it because `Cat <
    /// Animal` declares no field of its own.
    ///
    /// The header shift for a runtime-dispatch class is NOT applied here;
    /// every MIR consumer of `field_idx` adds it via
    /// `class_field_shift_for_ty`.
    pub(super) fn lookup_field_with_parents(
        &self,
        type_name: &str,
        field_name: &str,
    ) -> Option<(Ty, usize)> {
        // `(class_name, base_slot)` oldest ancestor first, so `base_slot`
        // is the running count of every field declared before that class.
        let mut chain: Vec<(String, usize)> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut cur = Some(type_name.to_string());
        while let Some(name) = cur.take() {
            // A malformed `class A < B` / `class B < A` cycle would spin
            // here forever. Resolve should reject one, but this walk must
            // terminate on its own rather than trust that.
            if !seen.insert(name.clone()) {
                break;
            }
            let Some(def) = self
                .symbols
                .named(&name)
                .find(|d| matches!(d.kind, DefKind::Class { .. }))
            else {
                break;
            };
            let DefKind::Class { info } = &def.kind else {
                break;
            };
            // Count only entries that really are fields — the same filter
            // `mir/lower/type_helpers.rs::class_field_layout` applies when
            // it assigns the slots. Counting `info.fields.len()` raw would
            // let the two disagree and shift every descendant's base.
            let field_count = info
                .fields
                .iter()
                .filter(|fid| {
                    matches!(
                        self.symbols.get(**fid).map(|d| &d.kind),
                        Some(DefKind::Field { .. })
                    )
                })
                .count();
            chain.push((name, field_count));
            cur = info
                .parent
                .and_then(|p| self.symbols.get(p))
                .map(|d| d.name.clone());
        }
        if chain.is_empty() {
            // Not a class (a struct, or an unknown name): slot index is
            // the declaration index, exactly as before.
            return self.lookup_field(type_name, field_name);
        }
        chain.reverse();
        let mut base = 0usize;
        for (_, count) in chain.iter_mut() {
            let own = *count;
            *count = base;
            base += own;
        }

        // Search newest class first so a subclass field shadows an
        // ancestor's of the same name, preserving the previous
        // self-before-parents order.
        for (name, base) in chain.iter().rev() {
            if let Some((ty, local)) = self.lookup_field(name, field_name) {
                return Some((ty, base + local));
            }
        }
        None
    }

    /// Look up a user-defined method on a class (or its parents) and return its
    /// return type. Overload-aware: among `method_name` and its
    /// `method_name__overload<N>` siblings declared on the nearest class that
    /// has any, the first that accepts `argc` arguments wins, else the first
    /// declared.
    pub(in crate::typeck) fn lookup_class_method_return(
        &self,
        type_name: &str,
        method_name: &str,
        argc: usize,
    ) -> Option<Ty> {
        let candidates = self.class_method_candidates(type_name, method_name);
        let selected = candidates
            .iter()
            .copied()
            .find(|candidate| self.method_accepts_arg_count(*candidate, argc))
            .or_else(|| candidates.first().copied());
        if let Some(selected) = selected {
            let Some(DefKind::Method { signature, .. }) =
                self.symbols.get(selected).map(|def| &def.kind)
            else {
                unreachable!("class_method_candidates yields only Method definitions");
            };
            return Some(self.ctx.resolve(&signature.return_ty));
        }
        let parent = self.parent_class_name(type_name)?;
        self.lookup_class_method_return(&parent, method_name, argc)
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
        // Q1 (do-block variant): a trailing `do…end` block fills the method's
        // LAST (closure) parameter, so an overload accepts the block only when
        // its final param is callable. Without this, `c.text do…end` (empty
        // `args`) matched the first-declared `text(&str)` overload by arity.
        if has_block {
            let last_is_callable = signature
                .params
                .last()
                .map(|p| self.ctx.resolve(&p.ty).is_callable())
                .unwrap_or(false);
            if !last_is_callable {
                return false;
            }
        }
        args.iter()
            .zip(signature.params.iter())
            .all(|(arg, param)| {
                let arg_ty = self.ctx.resolve(&arg.ty);
                let param_ty = self.ctx.resolve(&param.ty);
                // Q1: a CALLABLE argument — a closure literal (whose type may
                // still be `Infer` at overload-selection time), or any value
                // of `Fn`/`any Fn`/`some Fn` type — matches ONLY a callable
                // parameter, never a `&str`/other param. Without this, a
                // closure argument fell through to the `is_infer()` arm below
                // and the first-declared overload (`text(&str)`) was chosen,
                // mis-storing the closure pointer as a `String` → heap
                // corruption. Checking `arg.kind` (syntactic) makes selection
                // correct even before the closure's type is inferred.
                let arg_is_callable =
                    matches!(arg.kind, HirExprKind::Closure { .. }) || arg_ty.is_callable();
                if arg_is_callable {
                    return param_ty.is_callable();
                }
                arg_ty.is_infer()
                    || arg_ty.is_error()
                    || arg_ty == param_ty
                    // Owned arg vs borrowing param (`&T` param, `T` arg) — a
                    // bare `String` literal/value (now owned) satisfies a
                    // `&String` param via the call site's auto-ref. The old
                    // `&str`↔`String` arms are gone with the `&str` type.
                    || matches!(&param_ty, Ty::Ref(inner) | Ty::RefMut(inner) if **inner == arg_ty)
            })
    }

    fn class_method_candidates(&self, type_name: &str, method_name: &str) -> Vec<DefId> {
        self.symbols
            .methods_named(type_name, method_name)
            .map(|def| def.id)
            .collect()
    }

    fn find_class_def(&self, type_name: &str) -> Option<DefId> {
        self.symbols
            .named(type_name)
            .find(|def| matches!(def.kind, DefKind::Class { .. }))
            .map(|def| def.id)
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

    pub(super) fn default_ast_to_hir(
        &mut self,
        default: &ast::Expr,
        param_ty: &Ty,
    ) -> Option<HirExpr> {
        // A `nil` default — used by the optional `&block` slot (ADR D5) — is a
        // null value typed as the parameter's own type (a `Ty::Fn`), so the
        // block slot receives the null closure-pair-pointer sentinel (ADR D1).
        if matches!(default.kind, ast::ExprKind::NullLiteral) {
            return Some(HirExpr {
                kind: HirExprKind::NullLiteral,
                ty: param_ty.clone(),
                span: default.span.clone(),
            });
        }
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

    pub(super) fn append_method_default_args(
        &mut self,
        method_id: DefId,
        args: &mut Vec<HirExpr>,
        has_trailing_block: bool,
    ) {
        let defaults: Vec<(ast::Expr, Ty)> = self
            .symbols
            .get(method_id)
            .and_then(|def| match &def.kind {
                DefKind::Method { signature, .. } => Some(signature),
                _ => None,
            })
            .map(|signature| {
                // A trailing block fills the LAST parameter slot; reserve it so
                // its `nil` default is not also emitted (Ruby-block ADR D5).
                let take = signature
                    .params
                    .len()
                    .saturating_sub(usize::from(has_trailing_block));
                signature
                    .params
                    .iter()
                    .take(take)
                    .skip(args.len())
                    // Pair each default with its param type so a `nil` default
                    // (the optional `&block` slot, Ruby-block-semantics ADR D5)
                    // is materialized as a null value of the param's `Ty::Fn`.
                    .filter_map(|p| p.default.clone().map(|d| (d, p.ty.clone())))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for (default, param_ty) in defaults {
            if let Some(hir) = self.default_ast_to_hir(&default, &param_ty) {
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
        &mut self,
        class_name: &str,
        args: &[HirExpr],
    ) -> Option<Vec<Ty>> {
        // Declared generic params of the class OR struct.
        let generic_params = self.type_generic_param_names(class_name);
        if generic_params.is_empty() {
            return None;
        }

        // Constructor parameter types: the `init` method's params when one
        // exists, otherwise the type's declared fields in order (the implicit
        // positional `.new`). Structs typically take the field path.
        let ctor_params = self.constructor_param_types(class_name);

        // Walk the parameters and capture TypeParam positions.
        let mut bindings: std::collections::HashMap<String, Ty> = std::collections::HashMap::new();
        for (param_ty, arg) in ctor_params.iter().zip(args.iter()) {
            let actual = self.ctx.resolve(&arg.ty);
            Self::collect_typeparam_bindings(param_ty, &actual, &mut bindings);
        }

        // Assemble generic args in declaration order. A param not determined
        // by any constructor argument (a "phantom" param like `Sig[T]` with
        // `T` unused in fields) becomes a FRESH inference var rather than
        // collapsing the head to the bare `Sig` — the call's expected type
        // (e.g. a declared `-> Sig[T]` return) then binds it via unification
        // (Q21). Previously this returned `None`, yielding `Sig` with no
        // generic args, which failed to unify with `Sig[T]`.
        let mut out = Vec::with_capacity(generic_params.len());
        for gp in &generic_params {
            match bindings.get(gp) {
                Some(ty) => out.push(ty.clone()),
                None => out.push(self.ctx.fresh_type_var()),
            }
        }
        Some(out)
    }

    /// Declared generic-parameter names of a class or struct by name, in
    /// declaration order. Empty when the type is non-generic or unknown.
    fn type_generic_param_names(&self, type_name: &str) -> Vec<String> {
        for def in self.symbols.named(type_name) {
            let params = match &def.kind {
                DefKind::Class { info } => Some(&info.generic_params),
                DefKind::Struct { info } => Some(&info.generic_params),
                _ => None,
            };
            if let Some(params) = params {
                return params.iter().map(|gp| gp.name.clone()).collect();
            }
        }
        Vec::new()
    }

    /// The positional constructor parameter types for a class/struct: the
    /// `init` method's params if declared, else a declared `self.new`'s
    /// params (an FFI constructor such as `Mutex.new(initial: T)`), else the
    /// type's declared field types (the implicit all-fields `.new`). Used to
    /// harvest generic-param bindings from constructor arguments.
    pub(super) fn constructor_param_types(&self, type_name: &str) -> Vec<Ty> {
        // Prefer an explicit `init` method's params, then a declared `new`.
        for ctor in ["init", "new"] {
            if let Some(params) = self.own_method_param_types(type_name, ctor) {
                return params;
            }
        }
        // Fall back to the declared fields (positional `.new`).
        for def in self.symbols.named(type_name) {
            let field_ids = match &def.kind {
                DefKind::Class { info } => Some(info.fields.clone()),
                DefKind::Struct { info } => Some(info.fields.clone()),
                _ => None,
            };
            if let Some(field_ids) = field_ids {
                return field_ids
                    .iter()
                    .filter_map(|fid| match self.symbols.get(*fid).map(|d| &d.kind) {
                        Some(DefKind::Field { ty, .. }) => Some(ty.clone()),
                        _ => None,
                    })
                    .collect();
            }
        }
        Vec::new()
    }

    /// The parameter types of the method `method` declared directly on
    /// `type_name`, or `None` when the type declares no such method.
    fn own_method_param_types(&self, type_name: &str, method: &str) -> Option<Vec<Ty>> {
        self.symbols.named(method).find_map(|def| match &def.kind {
            DefKind::Method { parent, signature }
                if self
                    .symbols
                    .get(*parent)
                    .is_some_and(|p| p.name == type_name) =>
            {
                Some(signature.params.iter().map(|p| p.ty.clone()).collect())
            }
            _ => None,
        })
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
        // Bind the receiver's class generics (`T`/`K`/`V`) and, for a
        // mixin-default method that returns `Self`/`&Self` (e.g.
        // `Comparable#clamp`), bind `Self` to the receiver's own concrete
        // type so the result is the concrete class rather than the abstract
        // `Self: Bound` type param. A `Self`-free return is unaffected: the
        // substitution is a structural no-op when the return names neither
        // `Self` nor a receiver generic.
        let mut subst = self.receiver_generic_bindings(obj_ty).unwrap_or_default();
        subst
            .entry("Self".to_string())
            .or_insert_with(|| obj_ty.peel_refs().clone());
        let ret = ret_ty.subst_params(&subst);
        // A receiver that is no nominal type dispatches to a shell class
        // (the `layout closure` class for a `Proc[...]`, `Ty::method_home`); inside the shell
        // `Self` is the shell, so its own type in a return stands for the
        // receiver's type (`p.clone` is a `Proc[...]` again).
        let receiver = obj_ty.peel_refs();
        if matches!(
            receiver,
            Ty::Class { .. } | Ty::Struct { .. } | Ty::Enum { .. }
        ) {
            return ret;
        }
        let home = receiver.method_home(self.symbols.closure_shell_name());
        Self::replace_shell_class(&ret, &home, receiver)
    }

    /// `ty` with every non-generic `Class { name: home }` replaced by
    /// `receiver`.
    fn replace_shell_class(ty: &Ty, home: &str, receiver: &Ty) -> Ty {
        match ty {
            Ty::Class { name, generic_args } if name == home && generic_args.is_empty() => {
                receiver.clone()
            }
            other => other
                .clone()
                .map_inner(&mut |child| Self::replace_shell_class(child, home, receiver)),
        }
    }

    /// Build the `{class-param-name → receiver-concrete-arg}` map for a
    /// builtin/user generic receiver — e.g. `Array[Int]` → `{T: Int}`,
    /// `Hash[K, V]` → `{K: …, V: …}`. Returns `None` when the receiver
    /// is non-generic, or the declared arity doesn't match (caller falls
    /// back to no substitution). This is the single mapping consumed by
    /// both return-type substitution and the receiver-element bound seam
    /// (`bridge_builtin_method`'s `where T: Add` check).
    pub(super) fn receiver_generic_bindings(
        &self,
        obj_ty: &Ty,
    ) -> Option<std::collections::HashMap<String, Ty>> {
        // Zero-Rust-stdlib bridge (Phase B / M3): a builtin generic head
        // (`Array`, `Set`, `Hash`, `Option`, `Result`, either spelling) maps
        // to its `.rx` method-home class with the head's generic args, so the
        // class's declared type params (`class Array[T]` → `[T]`, `enum
        // Option[T]`) substitute against the receiver's concrete element type
        // (`Option[Int].unwrap` yields `Int`). `Ty::builtin_head` peels
        // references, so `(&Array[Int]).pop` substitutes like `Array[Int].pop`.
        let (name, generic_args): (String, Vec<Ty>) = match obj_ty.builtin_head() {
            Some((home, args)) => (home.to_string(), args),
            None => match obj_ty.peel_refs() {
                Ty::Class { name, generic_args } | Ty::Struct { name, generic_args }
                    if !generic_args.is_empty() =>
                {
                    (name.clone(), generic_args.clone())
                }
                _ => return None,
            },
        };

        // Build a name→type map using the class's declared generic params.
        let class_params: Vec<String> = {
            let mut out = Vec::new();
            for def in self.symbols.named(&name) {
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
            out
        };
        if class_params.len() != generic_args.len() {
            return None;
        }
        let subst: std::collections::HashMap<String, Ty> = class_params
            .into_iter()
            .zip(generic_args.iter().cloned())
            .collect();
        Some(subst)
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
                    if bound.is_fn_family() {
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
                if bound.is_fn_family() {
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
    /// [`Self::bind_type_params_from_args`], and `Ty::subst_params` the result into
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
        self.harvest_and_subst_generics_bindings(signature, args, ret)
            .0
    }

    /// Same as [`Self::harvest_and_subst_generics`] but ALSO returns the
    /// `{param_name → concrete}` bindings it harvested, so the caller can
    /// run the declared-bound enforcement seam (`check_generic_param_bounds`)
    /// against them. Feature B (trait-bound enforcement): the bindings are
    /// exactly the `{T → concrete}` map a `[T: Bound]` declaration must be
    /// checked against, computed once here rather than re-derived at the
    /// call site.
    pub(super) fn harvest_and_subst_generics_bindings(
        &self,
        signature: &crate::resolve::symbols::FnSignature,
        args: &[HirExpr],
        ret: &Ty,
    ) -> (Ty, std::collections::HashMap<String, Ty>) {
        if signature.generic_params.is_empty() {
            return (ret.clone(), std::collections::HashMap::new());
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
        let out = if bindings.is_empty() {
            ret.clone()
        } else {
            ret.subst_params(&bindings)
        };
        (out, bindings)
    }
}
