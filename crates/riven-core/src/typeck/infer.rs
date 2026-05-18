//! Bidirectional type inference engine.
//!
//! Two modes of operation:
//! - **Synthesis (forward):** Given an expression, compute its type.
//! - **Checking (backward):** Given an expression and an expected type, verify compatibility.
//!
//! The inference engine walks the type-checked HIR and resolves all
//! inference variables to concrete types.

use crate::diagnostics::Diagnostic;
use crate::hir::context::TypeContext;
use crate::hir::nodes::*;
use crate::hir::types::Ty;
use crate::lexer::token::Span;
use crate::parser::ast::{BinOp, UnaryOp, Visibility};
use crate::resolve::symbols::{DefKind, SymbolTable};

use super::coerce::auto_deref;
use super::mixins::MixinResolver;
use super::unify::{can_coerce, unify, TypeError};

/// The type inference engine — walks HIR and resolves all types.
pub struct InferenceEngine<'a> {
    pub ctx: &'a mut TypeContext,
    pub symbols: &'a mut SymbolTable,
    pub traits: &'a MixinResolver,
    pub diagnostics: Vec<Diagnostic>,
    current_return_ty: Option<Ty>,
}

impl<'a> InferenceEngine<'a> {
    pub fn new(
        ctx: &'a mut TypeContext,
        symbols: &'a mut SymbolTable,
        traits: &'a MixinResolver,
    ) -> Self {
        Self {
            ctx,
            symbols,
            traits,
            diagnostics: Vec::new(),
            current_return_ty: None,
        }
    }

    /// Try unification first; if it fails, try coercion (directional).
    /// Used for contexts where implicit conversions are allowed:
    /// - Let binding (value → annotated type)
    /// - Function return (body → declared return type)
    /// - Function argument (arg → param type)
    fn unify_or_coerce(&mut self, expected: &Ty, found: &Ty, span: &Span) -> Result<Ty, TypeError> {
        match unify(expected, found, self.ctx, span) {
            Ok(ty) => Ok(ty),
            Err(_) => {
                // Try directional coercions
                let exp = self.ctx.resolve(expected);
                let fnd = self.ctx.resolve(found);

                // &str → String (string literal in String context)
                if exp == Ty::String && fnd == Ty::Str {
                    return Ok(Ty::String);
                }
                // Int → Float (integer literal in Float context)
                if exp.is_float() && fnd == Ty::Int {
                    return Ok(exp);
                }
                // &mut T → &T
                if let (Ty::Ref(_), Ty::RefMut(_)) = (&exp, &fnd) {
                    if can_coerce(&fnd, &exp, self.ctx) {
                        return Ok(exp);
                    }
                }
                // General coercion check
                if can_coerce(&fnd, &exp, self.ctx) {
                    return Ok(exp);
                }

                Err(TypeError::mismatch(expected, found, span))
            }
        }
    }

    pub(super) fn future_ty(output: Ty) -> Ty {
        Ty::Class {
            name: "Future".to_string(),
            generic_args: vec![output],
        }
    }

    pub(super) fn result_ty(ok: Ty, err_name: &str) -> Ty {
        Ty::Result(
            Box::new(ok),
            Box::new(Ty::Class {
                name: err_name.to_string(),
                generic_args: vec![],
            }),
        )
    }

    pub(super) fn option_ty(inner: Ty) -> Ty {
        Ty::Option(Box::new(inner))
    }

    pub(super) fn class_ty(name: &str, generic_args: Vec<Ty>) -> Ty {
        Ty::Class {
            name: name.to_string(),
            generic_args,
        }
    }

    pub(super) fn callable_return_ty(ty: &Ty) -> Option<Ty> {
        match ty {
            Ty::Fn { ret, .. } | Ty::FnMut { ret, .. } | Ty::FnOnce { ret, .. } => {
                Some((**ret).clone())
            }
            Ty::Ref(inner)
            | Ty::RefMut(inner)
            | Ty::RefLifetime(_, inner)
            | Ty::RefMutLifetime(_, inner) => Self::callable_return_ty(inner),
            _ => None,
        }
    }

    fn wrap_async_return(&self, signature: &crate::resolve::symbols::FnSignature) -> Ty {
        if signature.is_async {
            Self::future_ty(self.ctx.resolve(&signature.return_ty))
        } else {
            self.ctx.resolve(&signature.return_ty)
        }
    }

    /// Run type inference on the entire program.
    pub fn infer_program(&mut self, program: &mut HirProgram) {
        for item in &mut program.items {
            self.infer_item(item);
        }
    }

    fn infer_item(&mut self, item: &mut HirItem) {
        match item {
            HirItem::Class(class) => self.infer_class(class),
            HirItem::Struct(_) => {} // struct fields already have types
            HirItem::Enum(_) => {}   // enum variants already have types
            HirItem::Mixin(t) => {
                // Default method bodies need inference so that expressions
                // like `self.name` acquire a concrete return type (e.g.
                // String), otherwise interpolation later falls back to
                // integer-printing on the raw pointer value.
                for ti in &mut t.items {
                    if let HirMixinItem::DefaultMethod(m) = ti {
                        self.infer_func(m);
                    }
                }
            }
            HirItem::Impl(imp) => self.infer_impl(imp),
            HirItem::Function(func) => self.infer_func(func),
            HirItem::Module(m) => {
                for sub_item in &mut m.items {
                    self.infer_item(sub_item);
                }
            }
            HirItem::Const(c) => {
                self.infer_expr(&mut c.value);
                let val_ty = self.ctx.resolve(&c.value.ty);
                if let Err(e) = unify(&c.ty, &val_ty, self.ctx, &c.span) {
                    self.type_error(e);
                }
            }
            HirItem::TypeAlias(_) | HirItem::Newtype(_) => {}
        }
    }

    fn infer_class(&mut self, class: &mut HirClassDef) {
        for method in &mut class.methods {
            self.infer_func(method);
        }
        for imp in &mut class.impl_blocks {
            self.infer_impl(imp);
        }
    }

    fn infer_impl(&mut self, imp: &mut HirImplBlock) {
        for item in &mut imp.items {
            if let HirImplItem::Method(method) = item {
                self.infer_func(method);
            }
        }
    }

    fn infer_func(&mut self, func: &mut HirFuncDef) {
        // Check: public functions must have explicit type annotations
        if func.visibility == Visibility::Public {
            if func.return_ty.is_infer() {
                // For mut methods (RefMut self mode) or void-like methods
                // (display, display_all, etc.), default to Unit instead of erroring
                let is_mut_method = func.self_mode == Some(HirSelfMode::RefMut);
                let is_void_method = matches!(
                    func.name.as_str(),
                    "display" | "display_all" | "init" | "drop"
                );
                if is_mut_method || is_void_method {
                    func.return_ty = Ty::Unit;
                } else {
                    self.error(
                        format!(
                            "public function `{}` must have an explicit return type annotation",
                            func.name
                        ),
                        &func.span,
                    );
                }
            }
            for param in &func.params {
                if param.ty.is_infer() {
                    self.error(
                        format!(
                            "public function `{}` parameter `{}` must have an explicit type annotation",
                            func.name, param.name
                        ),
                        &param.span,
                    );
                }
            }
        }

        let old_return_ty = self.current_return_ty.replace(func.return_ty.clone());
        self.infer_expr(&mut func.body);

        // Check function body type against declared return type (with coercion)
        let body_ty = self.ctx.resolve(&func.body.ty);
        // Auto-ref for fluent/builder methods: a body whose tail expression
        // is `self` (typed as the receiver class `T`) must satisfy a
        // declared return type of `&T` or `&mut T`. Inside a `mut` method
        // `self` is typed as the class itself, not a reference, so without
        // this accommodation every builder declared `-> &mut Self` that
        // ends in `self` fails type-checking.
        let declared_ret = self.ctx.resolve(&func.return_ty);
        let auto_ref_ok = match (&declared_ret, &body_ty) {
            (Ty::Ref(inner), other) | (Ty::RefMut(inner), other) => {
                unify(inner, other, self.ctx, &func.span).is_ok()
            }
            _ => false,
        };
        if !auto_ref_ok {
            if let Err(e) = self.unify_or_coerce(&func.return_ty, &body_ty, &func.span) {
                // Don't error if the body type is Unit and the return type is an infer variable
                // (implicit unit return)
                if !func.return_ty.is_infer() || body_ty != Ty::Unit {
                    self.type_error(e);
                }
            }
        }
        self.check_concurrency_bounds(&func.return_ty, &body_ty, &func.span);

        // Resolve the return type now
        func.return_ty = self.ctx.resolve(&func.return_ty);

        // If the function was declared without an explicit return type
        // (so the resolver assigned a fresh inference variable) and body
        // typing didn't pin it to anything concrete, default to Unit.
        // Otherwise validation later reports "could not infer return type"
        // for perfectly ordinary void functions — especially those that
        // take `&mut T` parameters and end in a statement-expression
        // whose type was never materialised (e.g. `s.push('!')`).
        if func.return_ty.is_infer() {
            func.return_ty = Ty::Unit;
        }

        self.current_return_ty = old_return_ty;
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
                            self.lookup_on_type_param_bounds(&derefed, field_name, &expr.span)
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
                method_name,
                generic_args,
                args,
                block,
                ..
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
                    if method_name == "map" || method_name == "filter" {
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
                                let _ = unify(&param.ty, &elem_ty, self.ctx, &expr.span);
                            }
                        }
                    }
                    self.infer_expr(blk);
                }

                let obj_ty = self.ctx.resolve(&object.ty);
                let (_, derefed) = auto_deref(&obj_ty, self.ctx);

                // Constructor calls on a generic class: infer the class's
                // generic arguments from the types of the constructor args.
                // This turns `Pair.new(42, "hi")` into `Pair[Int, String]`.
                let ret_ty = if method_name == "new" {
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
                        expr.ty = self.wrap_async_return(&signature);
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
                let mut result_ty: Option<Ty> = None;

                for arm in arms.iter_mut() {
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

    fn infer_statement(&mut self, stmt: &mut HirStatement) {
        match stmt {
            HirStatement::Let {
                ty, value, def_id, ..
            } => {
                if let Some(ref mut val) = value {
                    self.infer_expr(val);
                    // Coerce a `[e1, e2, ..., eN]` array literal (which the
                    // resolver types as `Vec[T]`) into a fixed array when the
                    // binding has an explicit `[T; N]` annotation. The element
                    // count must match the annotation.
                    self.coerce_array_literal_to_fixed(ty, val);
                    let val_ty = self.ctx.resolve(&val.ty);
                    if let Err(e) = self.unify_or_coerce(ty, &val_ty, &val.span) {
                        self.type_error(e);
                    }
                    self.check_concurrency_bounds(ty, &val_ty, &val.span);
                }
                let resolved = self.ctx.resolve(ty);
                *ty = resolved.clone();
                // Update the symbol table with the resolved type
                self.symbols.update_ty(*def_id, resolved);
            }
            HirStatement::Expr(expr) => {
                self.infer_expr(expr);
            }
        }
    }

    /// If `expected` is a fixed-size array type `[T; N]` and `val` is an
    /// `ArrayLiteral` currently typed as `Vec[T]` (the resolver's default
    /// for bracket-syntax literals), rewrite `val` in place so its type
    /// becomes `[T; N]`.  Reports a compile error when the literal's
    /// element count differs from the annotation.
    fn coerce_array_literal_to_fixed(&mut self, expected: &Ty, val: &mut HirExpr) {
        let expected_resolved = self.ctx.resolve(expected);
        let (elem_ty, expected_len) = match &expected_resolved {
            Ty::FixedArray(elem, n) => match n.as_lit() {
                Some(v) => ((**elem).clone(), v as usize),
                // Unresolved const-param size — skip the literal-len
                // check (S5+ will compare structurally instead).
                None => return,
            },
            _ => return,
        };
        if let HirExprKind::ArrayLiteral(elems) = &val.kind {
            if elems.len() != expected_len {
                self.error(
                    format!(
                        "array literal has {} element{}, but the annotation expects {}",
                        elems.len(),
                        if elems.len() == 1 { "" } else { "s" },
                        expected_len,
                    ),
                    &val.span,
                );
                return;
            }
            val.ty = Ty::FixedArray(
                Box::new(elem_ty),
                crate::hir::types::ConstExpr::Lit(expected_len as u64),
            );
        }
    }

    fn check_concurrency_bounds(&mut self, expected: &Ty, found: &Ty, span: &Span) {
        let expected = self.ctx.resolve(expected);
        let found = self.ctx.resolve(found);

        let bounds: &[crate::hir::types::MixinRef] = match &expected {
            Ty::TypeParam { bounds, .. } | Ty::SomeMixin(bounds) | Ty::AnyMixin(bounds) => {
                bounds.as_slice()
            }
            _ => return,
        };

        for bound in bounds {
            match bound.name.as_str() {
                "Send" if !found.is_send_with(self.symbols) => {
                    self.diagnostics.push(Diagnostic::error_with_code(
                        format!("type `{}` does not satisfy `Send`", found),
                        span.clone(),
                        "E1011",
                    ));
                }
                "Sync" if !found.is_sync_with(self.symbols) => {
                    self.diagnostics.push(Diagnostic::error_with_code(
                        format!("type `{}` does not satisfy `Sync`", found),
                        span.clone(),
                        "E1012",
                    ));
                }
                _ => {}
            }
        }
    }

    // ─── Binary Operation Type Inference ────────────────────────────

    fn infer_binop(&mut self, op: BinOp, left: &Ty, right: &Ty, span: &Span) -> Ty {
        match op {
            // Arithmetic: both sides must be numeric, result is same type
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
                // String concatenation: any combination of `String`/`&str`
                // on both sides produces a newly-allocated `String`. This
                // has to be checked before the numeric path because `&str`
                // (Ty::Str) is not numeric but will happily unify with
                // itself in the generic fallback below, yielding the wrong
                // type.
                if op == BinOp::Add
                    && matches!(*left, Ty::String | Ty::Str)
                    && matches!(*right, Ty::String | Ty::Str)
                {
                    return Ty::String;
                }

                // Phase 2 stdlib (#06.5 T4): Duration / Instant operator
                // overloads. The mir/lower/expr/binops.rs special-case
                // routes the actual call to `riven_duration_add` /
                // `riven_duration_sub` / `riven_instant_sub`; here we
                // only need typeck to assign the right result Ty so
                // downstream `.as_nanos()` / `.as_secs()` method
                // resolution can find the Duration instance methods.
                //
                // `Duration + Duration` -> Duration
                // `Duration - Duration` -> Duration (saturating in runtime)
                // `Instant - Instant`   -> Duration (duration_since semantics)
                fn class_named(ty: &Ty, target: &str) -> bool {
                    match ty {
                        Ty::Class { name, .. } => name == target,
                        Ty::Ref(inner)
                        | Ty::RefMut(inner)
                        | Ty::RefLifetime(_, inner)
                        | Ty::RefMutLifetime(_, inner) => class_named(inner, target),
                        _ => false,
                    }
                }
                let duration_ty = || Ty::Class {
                    name: "Duration".to_string(),
                    generic_args: vec![],
                };
                if matches!(op, BinOp::Add | BinOp::Sub)
                    && class_named(left, "Duration")
                    && class_named(right, "Duration")
                {
                    return duration_ty();
                }
                if op == BinOp::Sub
                    && class_named(left, "Instant")
                    && class_named(right, "Instant")
                {
                    return duration_ty();
                }

                if left.is_numeric() && right.is_numeric() {
                    // Unify the two sides
                    match unify(left, right, self.ctx, span) {
                        Ok(unified) => unified,
                        Err(_) => {
                            // String + String = String (concatenation)
                            if *left == Ty::String && *right == Ty::String && op == BinOp::Add {
                                return Ty::String;
                            }
                            left.clone()
                        }
                    }
                } else if *left == Ty::String && op == BinOp::Add {
                    Ty::String
                } else {
                    match unify(left, right, self.ctx, span) {
                        Ok(unified) => unified,
                        Err(_) => left.clone(),
                    }
                }
            }

            // Comparison: both sides same type, result is Bool
            BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::Gt | BinOp::LtEq | BinOp::GtEq => {
                let _ = unify(left, right, self.ctx, span);
                Ty::Bool
            }

            // Logical: both sides Bool, result is Bool
            BinOp::And | BinOp::Or => Ty::Bool,

            // Bitwise: both sides integer, result is same type
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor | BinOp::Shl | BinOp::Shr => {
                match unify(left, right, self.ctx, span) {
                    Ok(unified) => unified,
                    Err(_) => left.clone(),
                }
            }
        }
    }

    fn infer_unaryop(&mut self, op: UnaryOp, operand: &Ty, _span: &Span) -> Ty {
        match op {
            UnaryOp::Neg => operand.clone(),
            UnaryOp::Not => {
                if *operand == Ty::Bool {
                    Ty::Bool
                } else {
                    operand.clone() // bitwise not
                }
            }
            UnaryOp::Deref => {
                // `*x` strips one level of reference.
                let resolved = self.ctx.resolve(operand);
                match resolved {
                    crate::hir::types::Ty::Ref(inner) | crate::hir::types::Ty::RefMut(inner) => {
                        *inner
                    }
                    // Not a reference — pass through (auto-deref is a no-op).
                    other => other,
                }
            }
        }
    }

    // ─── Method Call Resolution ─────────────────────────────────────

    fn resolve_method_call(
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
        }

        // Handle built-in methods on known types
        if let Some(ret) = self.builtin_method_type(obj_ty, method_name, args, span) {
            return ret;
        }

        // Look up in trait resolver
        if let Some(sig) = self.traits.lookup_method(obj_ty, method_name, self.symbols) {
            return self.wrap_async_return(&sig);
        }

        // Method call on a generic type parameter `T: Trait + Trait`
        // or `impl Trait` / `dyn Trait`: search the trait bounds for the
        // declaring trait and report ambiguity when multiple bounds match.
        if let Some(ret) = self.lookup_on_type_param_bounds(obj_ty, method_name, span) {
            return ret;
        }

        // For inference variables, we can't resolve yet — return a fresh var
        if obj_ty.is_infer() || obj_ty.is_error() {
            return self.ctx.fresh_type_var();
        }

        // Method not found — but don't error for common chaining patterns
        self.ctx.fresh_type_var()
    }

    fn infer_iter_collect_type(&mut self, obj_ty: &Ty, generic_args: &[Ty], span: &Span) -> Ty {
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

    fn iter_item_ty(&self, ty: &Ty) -> Option<Ty> {
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

    fn collect_target_compatible(&mut self, target: &Ty, item_ty: &Ty, span: &Span) -> bool {
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
    fn lookup_on_type_param_bounds(&mut self, ty: &Ty, name: &str, span: &Span) -> Option<Ty> {
        let bounds: &[crate::hir::types::MixinRef] = match ty {
            Ty::TypeParam { bounds, .. } | Ty::SomeMixin(bounds) | Ty::AnyMixin(bounds) => {
                bounds.as_slice()
            }
            _ => return None,
        };
        if bounds.is_empty() {
            return None;
        }
        match self.traits.lookup_method_on_bounds(bounds, name) {
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

    fn builtin_method_type(
        &mut self,
        ty: &Ty,
        method: &str,
        args: &[HirExpr],
        span: &Span,
    ) -> Option<Ty> {
        super::method_resolvers::builtin_method_type(self, ty, method, args, span)
    }
    fn infer_index_ty(&self, obj_ty: &Ty) -> Ty {
        match obj_ty {
            Ty::Array(elem) => *elem.clone(),
            Ty::FixedArray(elem, _) => *elem.clone(),
            // `m[&k]` panics on missing keys (mirrors Rust's `Index for
            // HashMap`); the runtime helper `riven_hash_index` returns the
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

    fn lookup_field(&self, type_name: &str, field_name: &str) -> Option<(Ty, usize)> {
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
    fn lookup_field_with_parents(&self, type_name: &str, field_name: &str) -> Option<(Ty, usize)> {
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
    fn lookup_class_method_return(&self, type_name: &str, method_name: &str) -> Option<Ty> {
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

    fn error(&mut self, message: String, span: &Span) {
        self.diagnostics
            .push(Diagnostic::error(message, span.clone()));
    }

    /// Returns `true` if the type is acceptable as an argument to `puts`,
    /// `eputs`, or `print`.  Strings in any common form (`String`, `&str`,
    /// `&String`, `&&str`) qualify, as do zero-arg functions that return
    /// such a type (MIR auto-invokes them). `Infer`, `Error`, and `Never`
    /// are permitted to avoid cascading diagnostics.
    fn is_puts_compatible(ty: &Ty) -> bool {
        match ty {
            Ty::String | Ty::Str | Ty::Infer(_) | Ty::Error | Ty::Never => true,
            Ty::Ref(inner)
            | Ty::RefMut(inner)
            | Ty::RefLifetime(_, inner)
            | Ty::RefMutLifetime(_, inner) => Self::is_puts_compatible(inner),
            Ty::Fn { params, ret } if params.is_empty() => Self::is_puts_compatible(ret),
            _ => false,
        }
    }

    fn type_error(&mut self, err: TypeError) {
        self.diagnostics
            .push(Diagnostic::error(err.message, err.span));
    }

    /// Infer the generic arguments of a class from the concrete types of a
    /// constructor call's arguments.  Walks the init method parameters and
    /// matches each TypeParam position with the corresponding argument's
    /// type. Returns `None` if the class has no generic params or if
    /// inference cannot cover every parameter.
    fn infer_class_generics(&self, class_name: &str, args: &[HirExpr]) -> Option<Vec<Ty>> {
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
    fn substitute_generics_in_return(&self, obj_ty: &Ty, ret_ty: &Ty) -> Ty {
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
            _ => ty.clone(),
        }
    }
}

/// Whether `*Iter[T].sum` should be accepted at typeck.
///
/// The runtime path is `riven_vec_sum`, an integer-only summation over
/// the raw 64-bit slot. Allowing only `Add`-style numeric Items here
/// matches that runtime contract: any pointer-shaped Item (String,
/// Vec, HashMap, custom class) would silently produce nonsense.
///
/// `Ty::Infer` and `Ty::Error` pass through — they will be either
/// pinned to a numeric type by a downstream constraint or already
/// surfaced as a separate diagnostic. Phase 2 stdlib (#05 batch 3).
/// Extract the element type from a container type produced by a v1
/// collection macro: `Array[T]`, `Set[T]`, or `FixedArray[T; N]`.
fn container_elem_ty(ty: &Ty) -> Option<Ty> {
    match ty {
        Ty::Array(elem) | Ty::Set(elem) | Ty::Option(elem) => Some((**elem).clone()),
        Ty::FixedArray(elem, _) => Some((**elem).clone()),
        _ => None,
    }
}

/// Extract `(K, V)` from a `Map[K, V]` type.
fn map_kv_tys(ty: &Ty) -> Option<(Ty, Ty)> {
    match ty {
        Ty::Map(k, v) => Some(((**k).clone(), (**v).clone())),
        _ => None,
    }
}

pub(super) fn is_iter_sum_compatible(ty: &Ty) -> bool {
    matches!(
        ty,
        Ty::Int
            | Ty::Int8
            | Ty::Int16
            | Ty::Int32
            | Ty::Int64
            | Ty::ISize
            | Ty::UInt
            | Ty::UInt8
            | Ty::UInt16
            | Ty::UInt32
            | Ty::UInt64
            | Ty::USize
            | Ty::Float
            | Ty::Float32
            | Ty::Float64
            | Ty::Infer(_)
            | Ty::Error
    )
}

/// Infer the concrete `generic_args` for a user-defined enum variant
/// constructor.  Builds a substitution from each declared generic param
/// name to the concrete arg type observed at the matching payload slot,
/// then resolves each declared generic param through that substitution.
/// Generic params with no matching payload slot (e.g. the unit variant
/// `MyOpt.None` has no payload) are filled from the enclosing return
/// type (if it's the same enum) or a fresh inference variable.
fn infer_user_enum_generic_args(
    engine: &mut InferenceEngine<'_>,
    type_def: DefId,
    variant_idx: usize,
    fields: &[(String, HirExpr)],
    type_name: &str,
) -> Vec<Ty> {
    use crate::resolve::symbols::{DefKind, VariantDefKind};

    let (generic_param_names, variant_field_tys): (Vec<String>, Vec<Ty>) = {
        let enum_def = match engine.symbols.get(type_def) {
            Some(d) => d,
            None => return vec![],
        };
        let info = match &enum_def.kind {
            DefKind::Enum { info } => info,
            _ => return vec![],
        };
        let param_names: Vec<String> = info
            .generic_params
            .iter()
            .map(|gp| gp.name.clone())
            .collect();
        if param_names.is_empty() {
            return vec![];
        }
        let variant_def_id = match info.variants.get(variant_idx).copied() {
            Some(id) => id,
            None => return vec![],
        };
        let variant_def = match engine.symbols.get(variant_def_id) {
            Some(d) => d,
            None => return vec![],
        };
        let field_tys: Vec<Ty> = match &variant_def.kind {
            DefKind::EnumVariant { kind, .. } => match kind {
                VariantDefKind::Tuple(tys) => tys.clone(),
                VariantDefKind::Struct(fs) => fs.iter().map(|(_, t)| t.clone()).collect(),
                VariantDefKind::Unit => vec![],
            },
            _ => vec![],
        };
        (param_names, field_tys)
    };

    // Match each declared payload slot to the actual arg type and build
    // a name -> concrete-ty substitution.
    let mut subst: std::collections::HashMap<String, Ty> = std::collections::HashMap::new();
    for (decl_ty, (_, arg_expr)) in variant_field_tys.iter().zip(fields.iter()) {
        let arg_ty = engine.ctx.resolve(&arg_expr.ty);
        record_tyvar_binding(decl_ty, &arg_ty, &mut subst);
    }

    // For any generic param we didn't pin, try the enclosing return type
    // (`Ty::Enum { name: type_name, generic_args: [...] }`), else fall
    // back to a fresh inference variable.
    let return_args: Option<Vec<Ty>> =
        engine.current_return_ty.as_ref().and_then(|ret| match ret {
            Ty::Enum { name, generic_args } if name == type_name => Some(generic_args.clone()),
            _ => None,
        });

    generic_param_names
        .iter()
        .enumerate()
        .map(|(idx, name)| {
            if let Some(t) = subst.get(name) {
                return t.clone();
            }
            if let Some(args) = &return_args {
                if let Some(t) = args.get(idx) {
                    return t.clone();
                }
            }
            engine.ctx.fresh_type_var()
        })
        .collect()
}

/// Walk `decl_ty` (a declared variant field type that may contain
/// `Ty::TypeParam { name }` placeholders) alongside `arg_ty` (the
/// concrete argument type) and record each placeholder name's concrete
/// binding into `subst`. Mismatched shapes silently drop their
/// contribution — the type checker's normal unification later flags
/// any genuine error.
fn record_tyvar_binding(
    decl_ty: &Ty,
    arg_ty: &Ty,
    subst: &mut std::collections::HashMap<String, Ty>,
) {
    match (decl_ty, arg_ty) {
        (Ty::TypeParam { name, .. }, concrete) => {
            subst
                .entry(name.clone())
                .or_insert_with(|| concrete.clone());
        }
        (Ty::Option(a), Ty::Option(b)) => record_tyvar_binding(a, b, subst),
        (Ty::Array(a), Ty::Array(b)) => record_tyvar_binding(a, b, subst),
        (Ty::Ref(a), Ty::Ref(b)) => record_tyvar_binding(a, b, subst),
        (Ty::RefMut(a), Ty::RefMut(b)) => record_tyvar_binding(a, b, subst),
        (Ty::Result(a1, a2), Ty::Result(b1, b2)) => {
            record_tyvar_binding(a1, b1, subst);
            record_tyvar_binding(a2, b2, subst);
        }
        (Ty::Tuple(a), Ty::Tuple(b)) if a.len() == b.len() => {
            for (x, y) in a.iter().zip(b.iter()) {
                record_tyvar_binding(x, y, subst);
            }
        }
        (
            Ty::Enum {
                name: an,
                generic_args: aa,
            },
            Ty::Enum {
                name: bn,
                generic_args: ba,
            },
        ) if an == bn && aa.len() == ba.len() => {
            for (x, y) in aa.iter().zip(ba.iter()) {
                record_tyvar_binding(x, y, subst);
            }
        }
        _ => {}
    }
}

/// Walk a loop body collecting the types of every `break VALUE` (and
/// recording `Unit` for bare `break`) so the enclosing `loop` expression
/// can be given a precise type. Recursion stops at nested `Loop`/`While`/
/// `For` bodies — those breaks belong to the inner loop, not ours.
fn collect_break_types(
    expr: &HirExpr,
    ctx: &mut crate::hir::context::TypeContext,
    acc: &mut Option<Ty>,
    loop_span: &Span,
) {
    match &expr.kind {
        HirExprKind::Break(value) => {
            let t = match value {
                Some(v) => ctx.resolve(&v.ty),
                None => Ty::Unit,
            };
            match acc {
                Some(prev) => {
                    let _ = unify(prev, &t, ctx, loop_span);
                }
                None => *acc = Some(t),
            }
        }
        // Nested loops own their own breaks — do not descend.
        HirExprKind::Loop { .. } | HirExprKind::While { .. } | HirExprKind::For { .. } => {}
        // Returns never flow into our loop's result either; skip entirely.
        HirExprKind::Return(_) => {}

        // Structural recursion through every expression kind that can
        // syntactically contain a `break`.
        HirExprKind::Block(stmts, tail) => {
            for s in stmts {
                match s {
                    HirStatement::Let { value: Some(v), .. } => {
                        collect_break_types(v, ctx, acc, loop_span);
                    }
                    HirStatement::Expr(e) => {
                        collect_break_types(e, ctx, acc, loop_span);
                    }
                    _ => {}
                }
            }
            if let Some(t) = tail {
                collect_break_types(t, ctx, acc, loop_span);
            }
        }
        HirExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_break_types(cond, ctx, acc, loop_span);
            collect_break_types(then_branch, ctx, acc, loop_span);
            if let Some(e) = else_branch {
                collect_break_types(e, ctx, acc, loop_span);
            }
        }
        HirExprKind::Match { scrutinee, arms } => {
            collect_break_types(scrutinee, ctx, acc, loop_span);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    collect_break_types(g, ctx, acc, loop_span);
                }
                collect_break_types(&arm.body, ctx, acc, loop_span);
            }
        }
        HirExprKind::BinaryOp { left, right, .. } => {
            collect_break_types(left, ctx, acc, loop_span);
            collect_break_types(right, ctx, acc, loop_span);
        }
        HirExprKind::UnaryOp { operand, .. } => {
            collect_break_types(operand, ctx, acc, loop_span);
        }
        HirExprKind::Borrow { expr: inner, .. } => {
            collect_break_types(inner, ctx, acc, loop_span);
        }
        HirExprKind::Assign { target, value, .. }
        | HirExprKind::CompoundAssign { target, value, .. } => {
            collect_break_types(target, ctx, acc, loop_span);
            collect_break_types(value, ctx, acc, loop_span);
        }
        HirExprKind::FnCall { args, .. } => {
            for a in args {
                collect_break_types(a, ctx, acc, loop_span);
            }
        }
        HirExprKind::MethodCall {
            object,
            args,
            block,
            ..
        } => {
            collect_break_types(object, ctx, acc, loop_span);
            for a in args {
                collect_break_types(a, ctx, acc, loop_span);
            }
            if let Some(b) = block {
                collect_break_types(b, ctx, acc, loop_span);
            }
        }
        HirExprKind::FieldAccess { object, .. } => {
            collect_break_types(object, ctx, acc, loop_span);
        }
        HirExprKind::Interpolation { parts } => {
            for p in parts {
                if let HirInterpolationPart::Expr { expr: e, .. } = p {
                    collect_break_types(e, ctx, acc, loop_span);
                }
            }
        }
        // Other expression kinds cannot contain a break that targets
        // our loop (they are leaves, closures, or type-level nodes).
        _ => {}
    }
}
