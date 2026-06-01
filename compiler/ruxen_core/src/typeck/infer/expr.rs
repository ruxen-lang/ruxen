//! Expression-level type synthesis.
//!
//! The single `infer_expr` method is the giant dispatch over every
//! `HirExprKind`. Extracted from `mod.rs` so the per-variant logic is
//! easier to navigate.

use crate::diagnostics::Diagnostic;
use crate::hir::nodes::*;
use crate::hir::types::Ty;
use crate::resolve::symbols::DefKind;

use super::super::coerce::auto_deref;
use super::super::unify::unify;
use super::helpers::{
    collect_break_types, container_elem_ty, infer_user_enum_generic_args, map_kv_tys,
};
use super::InferenceEngine;

impl<'a> InferenceEngine<'a> {
    fn seed_closure_param_types(arg: &mut HirExpr, expected_ty: &Ty) {
        let HirExprKind::Closure { params, .. } = &mut arg.kind else {
            return;
        };
        let expected_params = match expected_ty {
            Ty::Fn { params, .. } | Ty::FnMut { params, .. } | Ty::FnOnce { params, .. } => params,
            Ty::Ref(inner)
            | Ty::RefMut(inner)
            | Ty::RefLifetime(_, inner)
            | Ty::RefMutLifetime(_, inner) => match inner.as_ref() {
                Ty::Fn { params, .. } | Ty::FnMut { params, .. } | Ty::FnOnce { params, .. } => {
                    params
                }
                _ => return,
            },
            _ => return,
        };
        for (param, expected) in params.iter_mut().zip(expected_params.iter()) {
            if param.ty.is_infer() {
                param.ty = expected.clone();
            }
        }
    }

    /// Infer and resolve the type of an expression (synthesis mode).
    pub fn infer_expr(&mut self, expr: &mut HirExpr) {
        match &mut expr.kind {
            // Literals — already typed during resolution
            HirExprKind::IntLiteral(_)
            | HirExprKind::FloatLiteral(_)
            | HirExprKind::StringLiteral(_)
            | HirExprKind::BoolLiteral(_)
            | HirExprKind::CharLiteral(_)
            | HirExprKind::UnitLiteral
            | HirExprKind::Error => {}

            // `/pat/flags` regex literal. Type was set by resolve to
            // `Ty::Class { name: "Regex" }`; here we validate the
            // pattern at compile time and emit E1704 if it doesn't
            // parse.
            HirExprKind::RegexLiteral { pattern, flags } => {
                use regex_syntax::ParserBuilder;
                let mut builder = ParserBuilder::new();
                // Map Ruxen flag chars onto regex-syntax compile
                // options. `g` is accepted as a no-op at lex time and
                // has no compile-side effect, so it's omitted here.
                if flags.contains('i') {
                    builder.case_insensitive(true);
                }
                if flags.contains('m') {
                    builder.multi_line(true);
                }
                if flags.contains('s') {
                    builder.dot_matches_new_line(true);
                }
                if flags.contains('x') {
                    builder.ignore_whitespace(true);
                }
                if let Err(err) = builder.build().parse(pattern) {
                    self.diagnostics.push(Diagnostic::error_with_code(
                        format!("invalid regex pattern: {}", err),
                        expr.span.clone(),
                        "E1704",
                    ));
                }
            }

            HirExprKind::VarRef(def_id) => {
                if let Some(ty) = self.symbols.def_ty(*def_id) {
                    let resolved = self.ctx.resolve(&ty);
                    expr.ty = resolved;
                }
            }

            HirExprKind::FieldAccess {
                object,
                field_name,
                field_idx,
            } => {
                self.infer_expr(object);
                let obj_ty = self.ctx.resolve(&object.ty);
                let (_, derefed) = auto_deref(&obj_ty, self.ctx);

                match &derefed {
                    Ty::Class { name, .. } | Ty::Struct { name, .. } => {
                        // Look up field in symbol table (including parent classes)
                        if let Some((field_ty, idx)) =
                            self.lookup_field_with_parents(name, field_name)
                        {
                            expr.ty = self.substitute_generics_in_return(&derefed, &field_ty);
                            *field_idx = idx;
                        } else if let Some(ret) =
                            self.builtin_method_type(&derefed, field_name, &[], &expr.span)
                        {
                            // Try method resolution as fallback — parser sometimes
                            // produces FieldAccess for no-arg method calls
                            expr.ty = self.substitute_generics_in_return(&derefed, &ret);
                        } else if let Some(sig) =
                            self.traits
                                .lookup_method(&derefed, field_name, self.symbols)
                        {
                            let raw = self.ctx.resolve(&sig.return_ty);
                            expr.ty = self.substitute_generics_in_return(&derefed, &raw);
                        } else {
                            // Last resort: try looking up as a user-defined method on this class
                            if let Some(ret) = self.lookup_class_method_return(name, field_name) {
                                expr.ty = self.substitute_generics_in_return(&derefed, &ret);
                            } else {
                                self.error(
                                    format!("no field `{}` on type `{}`", field_name, name),
                                    &expr.span,
                                );
                                expr.ty = Ty::Error;
                            }
                        }
                    }
                    Ty::Tuple(elems) => {
                        // Tuple field access by index: tuple.0, tuple.1
                        if let Ok(idx) = field_name.parse::<usize>() {
                            if idx < elems.len() {
                                expr.ty = elems[idx].clone();
                                *field_idx = idx;
                            } else {
                                self.error(
                                    format!(
                                        "tuple index {} out of range (tuple has {} elements)",
                                        idx,
                                        elems.len()
                                    ),
                                    &expr.span,
                                );
                                expr.ty = Ty::Error;
                            }
                        } else {
                            self.error(
                                format!("no field `{}` on tuple type", field_name),
                                &expr.span,
                            );
                            expr.ty = Ty::Error;
                        }
                    }
                    Ty::Newtype { name, inner } => {
                        // Newtype wrappers expose the inner value via `.0`.
                        if field_name == "0" {
                            expr.ty = (**inner).clone();
                            *field_idx = 0;
                        } else {
                            self.error(
                                format!("no field `{}` on newtype `{}`", field_name, name),
                                &expr.span,
                            );
                            expr.ty = Ty::Error;
                        }
                    }
                    Ty::Enum { name, .. } => {
                        // Try method resolution as fallback (e.g. .to_display, .weight)
                        if let Some(ret) =
                            self.builtin_method_type(&derefed, field_name, &[], &expr.span)
                        {
                            expr.ty = ret;
                        } else if let Some(sig) =
                            self.traits
                                .lookup_method(&derefed, field_name, self.symbols)
                        {
                            expr.ty = self.ctx.resolve(&sig.return_ty);
                        } else if let Some(ret) =
                            // ruby-naming.spec.md §3.4a: inline methods in
                            // the enum body lower with the same mangling
                            // class methods do, and the resolver registers
                            // them as `DefKind::Method` with the enum as
                            // parent — so the class-method lookup also
                            // finds them.
                            self.lookup_class_method_return(name, field_name)
                        {
                            expr.ty = ret;
                        } else {
                            self.error(
                                format!(
                                    "cannot access field `{}` on enum `{}`",
                                    field_name, derefed
                                ),
                                &expr.span,
                            );
                            expr.ty = Ty::Error;
                        }
                    }
                    // Option[T]: safe navigation — unwrap the Option and access
                    // the field on the inner type. The result is Option[FieldType].
                    Ty::Option(inner) => {
                        let inner_ty = self.ctx.resolve(inner);
                        let (_, inner_derefed) = auto_deref(&inner_ty, self.ctx);
                        // Try to resolve the field on the inner type
                        let field_ty = match &inner_derefed {
                            Ty::Class { name, .. } | Ty::Struct { name, .. } => {
                                if let Some((ft, idx)) =
                                    self.lookup_field_with_parents(name, field_name)
                                {
                                    *field_idx = idx;
                                    Some(ft)
                                } else if let Some(ret) = self.builtin_method_type(
                                    &inner_derefed,
                                    field_name,
                                    &[],
                                    &expr.span,
                                ) {
                                    Some(ret)
                                } else {
                                    self.lookup_class_method_return(name, field_name)
                                }
                            }
                            _ => self.builtin_method_type(
                                &inner_derefed,
                                field_name,
                                &[],
                                &expr.span,
                            ),
                        };
                        if let Some(ft) = field_ty {
                            // Wrap the field type in Option for safe navigation
                            expr.ty = Ty::Option(Box::new(ft));
                        } else {
                            self.error(
                                format!("no field `{}` on type `{}`", field_name, inner_derefed),
                                &expr.span,
                            );
                            expr.ty = Ty::Error;
                        }
                    }
                    _ if derefed.is_error() || derefed.is_infer() => {
                        // Can't resolve yet — leave as infer
                    }
                    _ => {
                        // Try method resolution as fallback for FieldAccess on types
                        // like Vec, String, &str, Option, Result, Class, etc.
                        // The parser sometimes produces FieldAccess for no-arg method calls.
                        if let Some(ret) =
                            self.builtin_method_type(&derefed, field_name, &[], &expr.span)
                        {
                            expr.ty = ret;
                        } else if let Some(sig) =
                            self.traits
                                .lookup_method(&derefed, field_name, self.symbols)
                        {
                            expr.ty = self.ctx.resolve(&sig.return_ty);
                        } else if let Some(ret) =
                            self.lookup_on_type_param_bounds(&derefed, field_name, &[], &expr.span)
                        {
                            expr.ty = ret;
                        } else {
                            self.error(
                                format!("no field `{}` on type `{}`", field_name, derefed),
                                &expr.span,
                            );
                            expr.ty = Ty::Error;
                        }
                    }
                }
            }

            HirExprKind::MethodCall {
                object,
                method,
                method_name,
                generic_args,
                args,
                block,
            } => {
                // ── Phase 2 stdlib (#04): HashMap.entry(K).or_insert(V) /
                //    .or_insert_with(closure) chain.
                //
                // The chain is the v1 surface for the prompt-04 entry API.
                // It is detected and inlined as a single MIR unit; there
                // is no real `Entry[K,V]` runtime value. Reject any use of
                // `or_insert` / `or_insert_with` whose receiver is not an
                // immediate `.entry(K)` call, so users get a clear error
                // rather than a silent fall-through into the lenient
                // unknown-method path.
                if method_name == "or_insert" || method_name == "or_insert_with" {
                    let receiver_is_entry_chain = matches!(
                        &object.kind,
                        HirExprKind::MethodCall { method_name: m, .. } if m == "entry"
                    );
                    if !receiver_is_entry_chain {
                        self.error(
                            format!(
                                "`{}` requires an immediate `.entry(K)` \
                                 receiver — write `m.entry(k).{}(...)`",
                                method_name, method_name,
                            ),
                            &expr.span,
                        );
                        // Recurse so the rest of the program still type-
                        // checks; we just want to surface this diagnostic.
                        self.infer_expr(object);
                        for arg in args.iter_mut() {
                            self.infer_expr(arg);
                        }
                        if let Some(ref mut blk) = block {
                            self.infer_expr(blk);
                        }
                        expr.ty = Ty::Unit;
                        return;
                    }
                    // Valid chain. Type-check the inner pieces and seed
                    // the closure return type for or_insert_with from V.
                    let map_ty: Option<Ty> = if let HirExprKind::MethodCall {
                        object: entry_recv,
                        args: entry_args,
                        ..
                    } = &mut object.kind
                    {
                        self.infer_expr(entry_recv);
                        for arg in entry_args.iter_mut() {
                            self.infer_expr(arg);
                        }
                        let recv_ty = self.ctx.resolve(&entry_recv.ty);
                        let (_, derefed) = auto_deref(&recv_ty, self.ctx);
                        if !matches!(&derefed, Ty::Map(_, _)) {
                            self.error(
                                format!(
                                    "`.entry(K)` is only defined on \
                                     `Map[K, V]`; got `{}`",
                                    derefed
                                ),
                                &entry_recv.span,
                            );
                        }
                        Some(derefed)
                    } else {
                        None
                    };
                    // Pin the entry-call's own type to Unit so it doesn't
                    // leak as an unresolved fresh var into later passes.
                    object.ty = Ty::Unit;

                    for arg in args.iter_mut() {
                        self.infer_expr(arg);
                    }
                    if let Some(ref mut blk) = block {
                        self.infer_expr(blk);
                    }

                    // Pin: for `or_insert_with`, the closure body's
                    // inferred type must match V. For `or_insert(v)`,
                    // arg[0]'s type must match V.
                    if let Some(Ty::Map(_, v_ty)) = map_ty.as_ref() {
                        if method_name == "or_insert_with" {
                            if let Some(blk) = block.as_ref() {
                                if let HirExprKind::Closure { body, .. } = &blk.kind {
                                    let body_ty = self.ctx.resolve(&body.ty);
                                    let _ = unify(&body_ty, v_ty, self.ctx, &blk.span);
                                }
                            }
                        } else if let Some(arg0) = args.first() {
                            let arg_ty = self.ctx.resolve(&arg0.ty);
                            let _ = unify(&arg_ty, v_ty, self.ctx, &arg0.span);
                        }
                    }

                    expr.ty = Ty::Unit;
                    return;
                }

                self.infer_expr(object);
                for arg in args.iter_mut() {
                    self.infer_expr(arg);
                }
                for targ in generic_args.iter_mut() {
                    *targ = self.ctx.resolve(targ);
                }

                if method_name == "collect" {
                    let obj_ty = self.ctx.resolve(&object.ty);
                    let (_, derefed) = auto_deref(&obj_ty, self.ctx);
                    expr.ty =
                        self.infer_iter_collect_type(&derefed, generic_args.as_slice(), &expr.span);
                    return;
                }

                // Seed the block's closure parameter type from the object's
                // element type before inferring the block body. E.g.
                // `opt.map { |n| n * 2 }` on `Option[Int]` unifies `n`'s
                // fresh type variable with `Int` so the body's return type
                // can be inferred concretely. Without this, the closure
                // parameter is an unresolved `Infer` and the enclosing
                // function's return type (`Option[α]`) keeps its free var,
                // which later fails `is_fully_resolved`.
                //
                // Limited to `map` for now. A broader seeding (e.g. `each`,
                // `filter`) would force concrete types where current codegen
                // relies on the closure param staying as an `Infer` that the
                // mangled-symbol suffix-matcher resolves at link time — and
                // generic trait-bound element types (`Vec[T: Displayable]`)
                // would start emitting literal "T: Displayable_method"
                // symbols the linker cannot resolve.
                if let Some(ref mut blk) = block {
                    if matches!(
                        method_name.as_str(),
                        "map" | "filter" | "find" | "all" | "any"
                    ) {
                        let obj_ty_pre = self.ctx.resolve(&object.ty);
                        let (_, derefed_pre) = auto_deref(&obj_ty_pre, self.ctx);
                        let elem_ty: Option<Ty> = match &derefed_pre {
                            Ty::Option(inner) => Some((**inner).clone()),
                            Ty::Array(inner) => Some((**inner).clone()),
                            Ty::Result(ok, _) => Some((**ok).clone()),
                            Ty::Class { name, generic_args } if name.ends_with("Iter") => {
                                generic_args.first().cloned()
                            }
                            _ => None,
                        };
                        if let (Some(elem_ty), HirExprKind::Closure { params, .. }) =
                            (elem_ty, &blk.kind)
                        {
                            if let Some(param) = params.first() {
                                let expected = if method_name == "find" {
                                    Ty::Ref(Box::new(elem_ty))
                                } else {
                                    elem_ty
                                };
                                let _ = unify(&param.ty, &expected, self.ctx, &expr.span);
                            }
                        }
                    }
                    self.infer_expr(blk);
                }

                let obj_ty = self.ctx.resolve(&object.ty);
                let (_, derefed) = auto_deref(&obj_ty, self.ctx);
                let selected_method = match &derefed {
                    Ty::Class { name, .. } => self
                        .select_class_method(name, method_name, args)
                        .inspect(|selected| {
                            *method = *selected;
                        }),
                    _ => None,
                };
                if let Some(selected) = selected_method {
                    self.append_method_default_args(selected, args);
                    let signature = self.symbols.get(selected).and_then(|def| match &def.kind {
                        DefKind::Method { signature, .. } => Some(signature.clone()),
                        _ => None,
                    });
                    if let Some(signature) = &signature {
                        for (arg, param) in args.iter_mut().zip(&signature.params) {
                            let param_ty = self.substitute_generics_in_return(&derefed, &param.ty);
                            Self::seed_closure_param_types(arg, &param_ty);
                            self.infer_expr(arg);
                        }
                    } else {
                        for arg in args.iter_mut() {
                            self.infer_expr(arg);
                        }
                    }
                }

                // Constructor calls on a generic class: infer the class's
                // generic arguments from the types of the constructor args.
                // This turns `Pair.new(42, "hi")` into `Pair[Int, String]`.
                let builtin_ret = if method_name == "new" {
                    None
                } else {
                    self.builtin_method_type(&derefed, method_name, args, &expr.span)
                };
                let ret_ty = if let Some(ret) = builtin_ret {
                    ret
                } else if method_name == "new" {
                    if let Ty::Class { name, generic_args } = &derefed {
                        if generic_args.is_empty() {
                            if let Some(inferred) = self.infer_class_generics(name, args) {
                                Ty::Class {
                                    name: name.clone(),
                                    generic_args: inferred,
                                }
                            } else {
                                self.resolve_method_call(&derefed, method_name, args, &expr.span)
                            }
                        } else {
                            self.resolve_method_call(&derefed, method_name, args, &expr.span)
                        }
                    } else {
                        self.resolve_method_call(&derefed, method_name, args, &expr.span)
                    }
                } else if let Some(selected) = selected_method {
                    let sig_opt = self.symbols.get(selected).and_then(|def| match &def.kind {
                        DefKind::Method { signature, .. } => Some(signature.clone()),
                        _ => None,
                    });
                    if let Some(signature) = sig_opt {
                        for (arg, param) in args.iter().zip(&signature.params) {
                            let param_ty = self.substitute_generics_in_return(&derefed, &param.ty);
                            let _ = unify(&arg.ty, &param_ty, self.ctx, &expr.span);
                            self.check_concurrency_bounds(&param_ty, &arg.ty, &arg.span);
                        }
                        let ret = self.wrap_async_return(&signature);
                        // First substitute the receiver's generic args
                        // (handles `MutexGuard[T]` from `Mutex[Int].lock_raw`).
                        let mut ret = self.substitute_generics_in_return(&derefed, &ret);
                        // Then, for a method that declares its OWN type params
                        // (`def expect[T](actual: T) -> Matcher[T]`), harvest
                        // `{T → concrete}` from the actual argument types and
                        // substitute into the return type — the same binding
                        // the FnCall path does for free generic functions.
                        // The receiver-based substitution above can't bind the
                        // method's own params, so `expect(aString)` previously
                        // returned `Matcher[T]` instead of `Matcher[String]`.
                        if !signature.generic_params.is_empty() {
                            let param_names: std::collections::HashSet<String> = signature
                                .generic_params
                                .iter()
                                .map(|gp| gp.name.clone())
                                .collect();
                            let mut bindings: std::collections::HashMap<String, Ty> =
                                std::collections::HashMap::new();
                            for (arg, param) in args.iter().zip(&signature.params) {
                                let actual = self.ctx.resolve(&arg.ty);
                                Self::bind_type_params_from_args(
                                    &param_names,
                                    &param.ty,
                                    &actual,
                                    &mut bindings,
                                );
                            }
                            if !bindings.is_empty() {
                                ret = Self::subst_ty(&ret, &bindings);
                            }
                        }
                        ret
                    } else {
                        self.ctx.fresh_type_var()
                    }
                } else {
                    // Regular method call — substitute TypeParam in the
                    // return type using the object's generic args.
                    let raw = self.resolve_method_call(&derefed, method_name, args, &expr.span);
                    self.substitute_generics_in_return(&derefed, &raw)
                };

                // For block-consuming combinators whose return type carries
                // a fresh inference variable (e.g. `map` on Option/Vec/
                // Result), unify that variable with the closure body's
                // inferred type so the container's element type is concrete.
                if let Some(ref blk) = block {
                    if method_name == "map" {
                        if let HirExprKind::Closure { body, .. } = &blk.kind {
                            let body_ty = self.ctx.resolve(&body.ty);
                            match &ret_ty {
                                Ty::Option(inner) | Ty::Array(inner) | Ty::Result(inner, _) => {
                                    let _ = unify(inner, &body_ty, self.ctx, &expr.span);
                                }
                                Ty::Class { name, generic_args } if name.ends_with("Iter") => {
                                    if let Some(inner) = generic_args.first() {
                                        let _ = unify(inner, &body_ty, self.ctx, &expr.span);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }

                expr.ty = self.ctx.resolve(&ret_ty);
            }

            HirExprKind::FnCall {
                callee,
                callee_name,
                args,
            } => {
                for arg in args.iter_mut() {
                    self.infer_expr(arg);
                }

                // Emit a friendly diagnostic when one of the built-in I/O
                // functions (`puts`, `eputs`, `print`, `println`, `eprintln`) is called with a
                // non-string argument. Without this check, the argument is
                // silently passed through unify (which allows arbitrary
                // integers or function references to reach the runtime),
                // and the resulting binary crashes or prints `(nil)`.
                if matches!(
                    callee_name.as_str(),
                    "puts" | "eputs" | "print" | "println" | "eprintln"
                ) && args.len() == 1
                {
                    let arg_ty = self.ctx.resolve(&args[0].ty);
                    if !Self::is_puts_compatible(&arg_ty) {
                        self.error(
                            format!(
                                "`{}` expects String or &str, found `{}`; use string interpolation: {} \"#{{expr}}\"",
                                callee_name, arg_ty, callee_name,
                            ),
                            &expr.span,
                        );
                    }
                }

                if *callee != UNRESOLVED_DEF {
                    // Clone the signature out to avoid borrow conflict
                    let sig_opt = self.symbols.get(*callee).and_then(|def| match &def.kind {
                        DefKind::Function { signature } | DefKind::Method { signature, .. } => {
                            Some(signature.clone())
                        }
                        DefKind::OverloadSet { candidates } => candidates
                            .iter()
                            .find_map(|id| self.symbols.get(*id))
                            .and_then(|def| match &def.kind {
                                DefKind::Function { signature }
                                | DefKind::Method { signature, .. } => Some(signature.clone()),
                                _ => None,
                            }),
                        _ => None,
                    });
                    if let Some(signature) = sig_opt {
                        // super() is variadic — skip argument count check
                        if callee_name == "super" {
                            // No arity check for super; arguments are forwarded to parent init
                        } else if args.len() != signature.params.len() {
                            self.error(
                                format!(
                                    "function `{}` expects {} arguments, got {}",
                                    callee_name,
                                    signature.params.len(),
                                    args.len()
                                ),
                                &expr.span,
                            );
                        } else {
                            for (arg, param) in args.iter().zip(&signature.params) {
                                let _ = unify(&arg.ty, &param.ty, self.ctx, &expr.span);
                                self.check_concurrency_bounds(&param.ty, &arg.ty, &arg.span);
                            }
                        }
                        // Generic free-function call inference: when the
                        // function declares its own type params (e.g.
                        // `expect[T](actual: T) -> Matcher[T]`), harvest
                        // `{T → concrete}` bindings by matching each formal
                        // param type against the resolved actual arg type,
                        // then substitute into the declared return type so
                        // the call expression's type becomes concrete
                        // (`Matcher[String]`). Without this the return type
                        // keeps `TypeParam{T}` and the concrete type never
                        // reaches MIR, so binop / Display over a `T` field
                        // can't dispatch. Type params NOT determined by an
                        // argument (return-only / turbofish) stay free —
                        // `bind_type_params_from_args` skips Infer/TypeParam
                        // actuals — preserving existing behaviour.
                        let mut base_ty = self.wrap_async_return(&signature);
                        if !signature.generic_params.is_empty() {
                            let param_names: std::collections::HashSet<String> = signature
                                .generic_params
                                .iter()
                                .map(|gp| gp.name.clone())
                                .collect();
                            let mut bindings: std::collections::HashMap<String, Ty> =
                                std::collections::HashMap::new();
                            for (arg, param) in args.iter().zip(&signature.params) {
                                let actual = self.ctx.resolve(&arg.ty);
                                Self::bind_type_params_from_args(
                                    &param_names,
                                    &param.ty,
                                    &actual,
                                    &mut bindings,
                                );
                            }
                            if !bindings.is_empty() {
                                base_ty = Self::subst_ty(&base_ty, &bindings);
                            }
                        }
                        expr.ty = base_ty;
                    }
                }
            }

            HirExprKind::BinaryOp { op, left, right } => {
                self.infer_expr(left);
                self.infer_expr(right);
                let left_ty = self.ctx.resolve(&left.ty);
                let right_ty = self.ctx.resolve(&right.ty);
                expr.ty = self.infer_binop(*op, &left_ty, &right_ty, &expr.span);
            }

            HirExprKind::UnaryOp { op, operand } => {
                self.infer_expr(operand);
                let operand_ty = self.ctx.resolve(&operand.ty);
                expr.ty = self.infer_unaryop(*op, &operand_ty, &expr.span);
            }

            HirExprKind::Borrow {
                mutable,
                expr: inner,
            } => {
                self.infer_expr(inner);
                let inner_ty = self.ctx.resolve(&inner.ty);
                expr.ty = if *mutable {
                    Ty::RefMut(Box::new(inner_ty))
                } else {
                    Ty::Ref(Box::new(inner_ty))
                };
            }

            HirExprKind::Block(stmts, tail) => {
                for stmt in stmts.iter_mut() {
                    self.infer_statement(stmt);
                }
                if let Some(ref mut tail_expr) = tail {
                    self.infer_expr(tail_expr);
                    expr.ty = self.ctx.resolve(&tail_expr.ty);
                } else {
                    expr.ty = Ty::Unit;
                }
            }

            HirExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.infer_expr(cond);
                // Condition must be Bool
                let cond_ty = self.ctx.resolve(&cond.ty);
                if cond_ty != Ty::Bool && !cond_ty.is_infer() && !cond_ty.is_error() {
                    self.error(
                        format!("if condition must be Bool, found `{}`", cond_ty),
                        &cond.span,
                    );
                }

                self.infer_expr(then_branch);
                if let Some(ref mut else_br) = else_branch {
                    self.infer_expr(else_br);
                    // Unify then and else branch types
                    let then_ty = self.ctx.resolve(&then_branch.ty);
                    let else_ty = self.ctx.resolve(&else_br.ty);
                    match unify(&then_ty, &else_ty, self.ctx, &expr.span) {
                        Ok(unified) => expr.ty = unified,
                        Err(_) => {
                            // Branches have different types — that's ok if one is Never
                            if then_ty.is_never() {
                                expr.ty = else_ty;
                            } else if else_ty.is_never() {
                                expr.ty = then_ty;
                            } else {
                                expr.ty = then_ty; // prefer then branch type
                            }
                        }
                    }
                } else {
                    // No else branch — type is Unit
                    expr.ty = Ty::Unit;
                }
            }

            HirExprKind::Match { scrutinee, arms } => {
                self.infer_expr(scrutinee);
                let scrutinee_ty = self.ctx.resolve(&scrutinee.ty);
                // Write the resolved type back onto the scrutinee's
                // HIR node — MIR's match-arm lowering reads
                // `scrutinee.ty` directly when deriving binding
                // types for `Some(x)` / `Ok(v)` / etc.  Without this
                // writeback, the binding's MIR local is allocated
                // with the unresolved `Ty::Option(Ty::Infer(_))`,
                // which lowers to `Ty::Int` and surfaces as
                // pointer-printed garbage at interpolation sites.
                scrutinee.ty = scrutinee_ty.clone();
                let mut result_ty: Option<Ty> = None;

                for arm in arms.iter_mut() {
                    // Propagate the scrutinee type into the arm's
                    // pattern bindings BEFORE inferring the body —
                    // otherwise `Some(t) -> t.describe` reads `t` as
                    // `Ty::Infer` and codegen emits `?T_describe`.
                    super::helpers::propagate_pattern_types(
                        &arm.pattern,
                        &scrutinee_ty,
                        self.symbols,
                    );

                    // Type check guard if present
                    if let Some(ref mut guard) = arm.guard {
                        self.infer_expr(guard);
                    }
                    self.infer_expr(&mut arm.body);
                    let arm_ty = self.ctx.resolve(&arm.body.ty);

                    if let Some(ref prev_ty) = result_ty {
                        if !arm_ty.is_never() {
                            let _ = unify(prev_ty, &arm_ty, self.ctx, &arm.span);
                        }
                    } else if !arm_ty.is_never() {
                        result_ty = Some(arm_ty);
                    }
                }

                expr.ty = result_ty.unwrap_or(Ty::Unit);
            }

            HirExprKind::While { condition, body } => {
                self.infer_expr(condition);
                self.infer_expr(body);
                expr.ty = Ty::Unit;
            }

            HirExprKind::Loop { body } => {
                self.infer_expr(body);
                // The loop expression's type is whatever the `break VALUE`s
                // in its body carry. Walk the body, stopping at nested
                // loops (those own their own breaks), and unify every
                // break-value type. A bare `break` contributes Unit.
                let mut break_ty: Option<Ty> = None;
                collect_break_types(body, self.ctx, &mut break_ty, &expr.span);
                expr.ty = break_ty.unwrap_or(Ty::Unit);
            }

            HirExprKind::For { iterable, body, .. } => {
                self.infer_expr(iterable);
                self.infer_expr(body);
                expr.ty = Ty::Unit;
            }

            HirExprKind::Assign {
                target,
                value,
                semantics,
            } => {
                self.infer_expr(target);
                self.infer_expr(value);
                let target_ty = self.ctx.resolve(&target.ty);
                let value_ty = self.ctx.resolve(&value.ty);
                let _ = unify(&target_ty, &value_ty, self.ctx, &expr.span);

                // Determine copy/move semantics
                let resolved = self.ctx.resolve(&value_ty);
                *semantics = resolved.move_semantics();
                expr.ty = Ty::Unit;
            }

            HirExprKind::CompoundAssign {
                target,
                op: _,
                value,
            } => {
                self.infer_expr(target);
                self.infer_expr(value);
                expr.ty = Ty::Unit;
            }

            HirExprKind::Return(value) => {
                if let Some(ref mut val) = value {
                    self.infer_expr(val);
                    if let Some(ref ret_ty) = self.current_return_ty {
                        let _ = unify(ret_ty, &val.ty, self.ctx, &expr.span);
                    }
                }
                expr.ty = Ty::Never;
            }

            HirExprKind::Break(value) => {
                if let Some(ref mut val) = value {
                    self.infer_expr(val);
                }
                expr.ty = Ty::Never;
            }

            HirExprKind::Continue => {
                expr.ty = Ty::Never;
            }

            HirExprKind::Closure { params, body, .. } => {
                self.infer_expr(body);
                let param_tys: Vec<Ty> = params.iter().map(|p| self.ctx.resolve(&p.ty)).collect();
                let ret_ty = self.ctx.resolve(&body.ty);
                expr.ty = Ty::Fn {
                    params: param_tys,
                    ret: Box::new(ret_ty),
                };
            }

            HirExprKind::Construct {
                fields,
                type_name: _,
                ..
            } => {
                for (_, field_expr) in fields.iter_mut() {
                    self.infer_expr(field_expr);
                }
                // Type was set during resolution
                expr.ty = self.ctx.resolve(&expr.ty);
            }

            HirExprKind::EnumVariant {
                fields,
                type_name,
                variant_name,
                type_def,
                variant_idx,
                ..
            } => {
                for (_, field_expr) in fields.iter_mut() {
                    self.infer_expr(field_expr);
                }
                // For Option/Result, construct the proper parameterized type
                // instead of a bare Ty::Enum
                if type_name == "Option" {
                    match variant_name.as_str() {
                        "Some" => {
                            let inner_ty = fields
                                .first()
                                .map(|(_, e)| self.ctx.resolve(&e.ty))
                                .unwrap_or(Ty::Error);
                            expr.ty = Ty::Option(Box::new(inner_ty));
                        }
                        "None" => {
                            // None — use the expected type if we have one, otherwise
                            // use an inference variable
                            let inner = self
                                .current_return_ty
                                .as_ref()
                                .and_then(|ret| match ret {
                                    Ty::Option(inner) => Some(*inner.clone()),
                                    _ => None,
                                })
                                .unwrap_or_else(|| self.ctx.fresh_type_var());
                            expr.ty = Ty::Option(Box::new(inner));
                        }
                        _ => {
                            expr.ty = Ty::Enum {
                                name: type_name.clone(),
                                generic_args: vec![],
                            };
                        }
                    }
                } else if type_name == "Result" {
                    match variant_name.as_str() {
                        "Ok" => {
                            let ok_ty = fields
                                .first()
                                .map(|(_, e)| self.ctx.resolve(&e.ty))
                                .unwrap_or(Ty::Unit);
                            // Try to get the error type from the function return type
                            let err_ty = self
                                .current_return_ty
                                .as_ref()
                                .and_then(|ret| match ret {
                                    Ty::Result(_, err) => Some(*err.clone()),
                                    _ => None,
                                })
                                .unwrap_or_else(|| self.ctx.fresh_type_var());
                            expr.ty = Ty::Result(Box::new(ok_ty), Box::new(err_ty));
                        }
                        "Err" => {
                            let err_ty = fields
                                .first()
                                .map(|(_, e)| self.ctx.resolve(&e.ty))
                                .unwrap_or(Ty::Error);
                            // Try to get the ok type from the function return type
                            let ok_ty = self
                                .current_return_ty
                                .as_ref()
                                .and_then(|ret| match ret {
                                    Ty::Result(ok, _) => Some(*ok.clone()),
                                    _ => None,
                                })
                                .unwrap_or_else(|| self.ctx.fresh_type_var());
                            expr.ty = Ty::Result(Box::new(ok_ty), Box::new(err_ty));
                        }
                        _ => {
                            expr.ty = Ty::Enum {
                                name: type_name.clone(),
                                generic_args: vec![],
                            };
                        }
                    }
                } else {
                    // User-defined enum. If the enum is generic, build
                    // `generic_args` by matching each declared generic
                    // parameter name to the concrete arg type at the
                    // corresponding payload slot.  Fall back to the
                    // expected return type (for bare unit variants like
                    // `MyOpt.None`) or a fresh inference variable.
                    let generic_args = infer_user_enum_generic_args(
                        self,
                        *type_def,
                        *variant_idx,
                        fields,
                        type_name,
                    );
                    expr.ty = Ty::Enum {
                        name: type_name.clone(),
                        generic_args,
                    };
                }
            }

            HirExprKind::Tuple(elems) => {
                for elem in elems.iter_mut() {
                    self.infer_expr(elem);
                }
                let tys: Vec<Ty> = elems.iter().map(|e| self.ctx.resolve(&e.ty)).collect();
                expr.ty = Ty::Tuple(tys);
            }

            HirExprKind::Index { object, index } => {
                self.infer_expr(object);
                self.infer_expr(index);
                let obj_ty = self.ctx.resolve(&object.ty);
                expr.ty = self.infer_index_ty(&obj_ty);
            }

            HirExprKind::Cast {
                expr: inner,
                target,
            } => {
                self.infer_expr(inner);
                expr.ty = target.clone();
            }

            HirExprKind::ArrayLiteral(elems) => {
                let mut elem_ty = self.ctx.fresh_type_var();
                for e in elems.iter_mut() {
                    self.infer_expr(e);
                    if let Ok(unified) = unify(&elem_ty, &e.ty, self.ctx, &expr.span) {
                        elem_ty = unified;
                    }
                }
                expr.ty = Ty::Array(Box::new(self.ctx.resolve(&elem_ty)));
            }

            HirExprKind::MapLiteral(entries) => {
                let mut k_ty = self.ctx.fresh_type_var();
                let mut v_ty = self.ctx.fresh_type_var();
                for (k, v) in entries.iter_mut() {
                    self.infer_expr(k);
                    self.infer_expr(v);
                    if let Ok(uk) = unify(&k_ty, &k.ty, self.ctx, &expr.span) {
                        k_ty = uk;
                    }
                    if let Ok(uv) = unify(&v_ty, &v.ty, self.ctx, &expr.span) {
                        v_ty = uv;
                    }
                }
                expr.ty = Ty::Map(
                    Box::new(self.ctx.resolve(&k_ty)),
                    Box::new(self.ctx.resolve(&v_ty)),
                );
            }

            HirExprKind::ArrayFill { value, .. } => {
                self.infer_expr(value);
                // Keep the Array type set during resolution
            }

            HirExprKind::Range { start, end, .. } => {
                if let Some(ref mut s) = start {
                    self.infer_expr(s);
                }
                if let Some(ref mut e) = end {
                    self.infer_expr(e);
                }
                // Range type is opaque for now
                expr.ty = self.ctx.resolve(&expr.ty);
            }

            HirExprKind::Interpolation { parts } => {
                for part in parts.iter_mut() {
                    if let HirInterpolationPart::Expr {
                        expr: ref mut e, ..
                    } = part
                    {
                        self.infer_expr(e);
                    }
                }
                expr.ty = Ty::String;
            }

            HirExprKind::MacroCall { name, args } => {
                for arg in args.iter_mut() {
                    self.infer_expr(arg);
                }
                // ruby-naming.spec.md §10a macros (`array!` / `vec!`,
                // `map!` / `hash!`, `set!`) compute their result type
                // at resolve time from `args_hir[0].ty`. That field is
                // a fresh inference variable at resolve time (the
                // resolver assigns `Ty::Infer(N)` to method calls,
                // function calls, etc.), and the typeck reassigns the
                // CALL expression's `.ty` to the resolved return type
                // without unifying with the original Infer var. So the
                // macro's stored element type stays unbound. Re-unify
                // here against each arg's freshly-inferred type so the
                // container's element / key / value types resolve.
                match name.as_str() {
                    "vec" | "array" | "set" => {
                        if let Some(elem_ty) = container_elem_ty(&expr.ty) {
                            for arg in args.iter() {
                                let arg_ty = self.ctx.resolve(&arg.ty);
                                let _ = unify(&elem_ty, &arg_ty, self.ctx, &expr.span);
                            }
                        }
                    }
                    "hash" | "map" => {
                        if let Some((k_ty, v_ty)) = map_kv_tys(&expr.ty) {
                            let mut it = args.iter();
                            while let (Some(k), Some(v)) = (it.next(), it.next()) {
                                let k_arg = self.ctx.resolve(&k.ty);
                                let v_arg = self.ctx.resolve(&v.ty);
                                let _ = unify(&k_ty, &k_arg, self.ctx, &expr.span);
                                let _ = unify(&v_ty, &v_arg, self.ctx, &expr.span);
                            }
                        }
                    }
                    _ => {}
                }
                expr.ty = self.ctx.resolve(&expr.ty);
            }

            HirExprKind::UnsafeBlock(stmts, tail) => {
                for stmt in stmts.iter_mut() {
                    self.infer_statement(stmt);
                }
                if let Some(tail_expr) = tail {
                    self.infer_expr(tail_expr);
                    expr.ty = tail_expr.ty.clone();
                } else {
                    expr.ty = Ty::Unit;
                }
            }

            HirExprKind::NullLiteral => {
                // ruby-naming.spec.md §3.10: `nil` is polymorphic across
                // three contexts — `Option[T]` absence, raw-pointer null,
                // and the comparison form `x == nil`. Type it as a fresh
                // inference variable so the surrounding context (a let
                // annotation, an argument type, a return type) drives
                // resolution via unification. If no context fixes it,
                // the variable resolves to `UInt64` for backwards
                // compatibility with the legacy raw-pointer behaviour.
                expr.ty = self.ctx.fresh_type_var();
            }
        }
    }
}
