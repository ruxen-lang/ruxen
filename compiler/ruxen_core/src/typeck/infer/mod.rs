//! Bidirectional type inference engine.
//!
//! Two modes of operation:
//! - **Synthesis (forward):** Given an expression, compute its type.
//! - **Checking (backward):** Given an expression and an expected type, verify compatibility.
//!
//! The inference engine walks the type-checked HIR and resolves all
//! inference variables to concrete types.
//!
//! ## Module layout
//!
//! The engine was originally a single ~2300 LOC file. It is now split:
//!
//! - `mod.rs` (this file) — `InferenceEngine` struct, constructors, the
//!   top-level dispatch (`infer_program`/`infer_item`/`infer_class`/
//!   `infer_impl`/`infer_func`), statement inference, and shared
//!   diagnostic helpers (`error`/`type_error`/`is_puts_compatible`).
//! - `expr.rs` — the giant `infer_expr` case analysis over every
//!   `HirExprKind` variant.
//! - `ops.rs` — binary/unary operator inference and concurrency-bound
//!   checks.
//! - `collect.rs` — method-call resolution, iterator/`collect` typing,
//!   field lookup, generic substitution.
//! - `helpers.rs` — free helper functions (`container_elem_ty`,
//!   `map_kv_tys`, `is_bufio_inner_supported`, `peel_refs`,
//!   `is_iter_sum_compatible`, `infer_user_enum_generic_args`,
//!   `record_tyvar_binding`, `collect_break_types`).
//!
//! All cross-file impl methods on `InferenceEngine` are declared
//! `pub(super)` so sibling modules in this directory can reach them
//! without widening visibility beyond the `typeck` module.

use crate::diagnostics::Diagnostic;
use crate::hir::context::TypeContext;
use crate::hir::nodes::*;
use crate::hir::types::Ty;
use crate::lexer::token::Span;
use crate::parser::ast::Visibility;
use crate::resolve::symbols::SymbolTable;

use super::mixins::MixinResolver;
use super::unify::{can_coerce, unify, TypeError};

mod collect;
mod expr;
mod helpers;
mod ops;

// Free-function helpers that need to be reachable from `super::infer::*`
// (e.g. `typeck::method_resolvers` consumes them through that path).
pub(super) use helpers::is_bufio_inner_supported;

/// The type inference engine — walks HIR and resolves all types.
pub struct InferenceEngine<'a> {
    pub ctx: &'a mut TypeContext,
    pub symbols: &'a mut SymbolTable,
    pub traits: &'a MixinResolver,
    pub diagnostics: Vec<Diagnostic>,
    pub(super) current_return_ty: Option<Ty>,
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
    pub(super) fn unify_or_coerce(
        &mut self,
        expected: &Ty,
        found: &Ty,
        span: &Span,
    ) -> Result<Ty, TypeError> {
        match unify(expected, found, self.ctx, span) {
            Ok(ty) => Ok(ty),
            Err(_) => {
                // Try directional coercions
                let exp = self.ctx.resolve(expected);
                let fnd = self.ctx.resolve(found);

                // (The `&str → String` coercion is gone: a string literal is
                // born `Ty::String`, so there is nothing to coerce.)
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
                // (One-string-type ADR: the old `Option`/`Result` String
                // payload drop hazard — a `Some("ada")` whose HIR `.ty` stayed
                // `Ty::Str` while storage held a heap `String`, causing a
                // storage-vs-payload drop mismatch / double-free — is dissolved.
                // A string literal is born `Ty::String`, so `Some("ada")`
                // builds `Option[String]` directly with no `&str` payload to
                // mistype. The case-116 nested-payload `ok_or` caution dies with
                // it: `opt.ok_or("missing")` now produces `Result[_, String]`.)
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

    pub(super) fn wrap_async_return(&self, signature: &crate::resolve::symbols::FnSignature) -> Ty {
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
            // Struct fields already have types, but inline methods and
            // `include Mixin` impl blocks (ruby-naming.spec.md §3.4a) carry
            // un-inferred bodies — their `self.field` accesses must be
            // type-checked so `FieldAccess.field_idx` / result types are
            // resolved. Skipping this left struct-method field reads with
            // `field_idx = 0` (always field 0) and an `Infer` result type
            // that codegen lowered as a raw i64 load at offset 0.
            HirItem::Struct(s) => {
                for method in &mut s.methods {
                    self.infer_func(method);
                }
                for imp in &mut s.impl_blocks {
                    self.infer_impl(imp);
                }
            }
            // Enum variants already have types; inline methods / impl blocks
            // need the same body inference as structs (same §3.4a surface).
            HirItem::Enum(e) => {
                for method in &mut e.methods {
                    self.infer_func(method);
                }
                for imp in &mut e.impl_blocks {
                    self.infer_impl(imp);
                }
            }
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
                // Overloaded methods are renamed `<name>__overload<N>` by the
                // resolver, so match the BASE name — otherwise an overloaded
                // void method (`drop`, `display`, …) would skip the Unit
                // default and spuriously demand an explicit return type.
                let base_name = func
                    .name
                    .split("__overload")
                    .next()
                    .unwrap_or(func.name.as_str());
                let is_void_method =
                    matches!(base_name, "display" | "display_all" | "init" | "drop");
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

        // Type-directed auto-call in return position: a bare nullary fn
        // reference as the body's tail expression binds its RESULT against
        // the declared return type (unless that type is a `Fn`). Runs
        // before the Option-wrap, same ordering as the let-binding hook.
        self.auto_call_fn_reference(&func.return_ty, &mut func.body);

        // "Drop Some" sugar: when the declared return type is `T?`
        // (= `Option[T]`) but the body's tail expression is a bare
        // `T`, wrap the tail in `Option::Some` automatically. Same
        // hook as let-binding RHS auto-wrap — see infer_statement.
        self.auto_wrap_option_some(&func.return_ty, &mut func.body);

        // (One-string-type ADR: the Q39 tuple-element `&str`→`String`
        // promotion is gone — a string literal is born `Ty::String`, so a
        // `("", false)` tuple is already `(String, Bool)`. No coercion needed.)

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
        self.check_declared_bounds(&func.return_ty, &body_ty, &func.span);

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

    pub(super) fn infer_statement(&mut self, stmt: &mut HirStatement) {
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
                    // Type-directed auto-call: a bare reference to a nullary
                    // function/method binds its RESULT unless the annotation
                    // is a `Fn` type. Runs before the Option-wrap so a
                    // `Fn() -> Int` reference bound to a `T?` slot first
                    // becomes `Int`, then wraps to `Some(Int)`.
                    self.auto_call_fn_reference(ty, val);
                    // "Drop Some" sugar: when the binding type is `T?`
                    // (= `Option[T]`) but the RHS is a bare `T`, wrap
                    // the RHS in `Option::Some` automatically. Pin:
                    // docs/specs/syntax/option-no-some.spec.md.
                    self.auto_wrap_option_some(ty, val);
                    // (One-string-type ADR: the Q38 owned-string-literal
                    // binding promotion and the Q39 tuple-element promotion are
                    // gone. A string literal is born `Ty::String`, so
                    // `let s = "x"` already binds an owned, drop-safe `String`
                    // identical to `let s: String = "x"`. There is no `&str`
                    // binding to promote, and no leak.)
                    let val_ty = self.ctx.resolve(&val.ty);
                    if let Err(e) = self.unify_or_coerce(ty, &val_ty, &val.span) {
                        self.type_error(e);
                    }
                    self.check_declared_bounds(ty, &val_ty, &val.span);
                }
                let resolved = self.ctx.resolve(ty);
                *ty = resolved.clone();
                // Update the symbol table with the resolved type
                self.symbols.update_ty(*def_id, resolved);
            }
            HirStatement::Expr(expr) => {
                self.infer_expr(expr);
                // Statement-position auto-call: a bare reference to a
                // function with no required arguments, used as a statement
                // (its value discarded), is a Ruby-style paren-less call —
                // e.g. `render` (whose only parameter is its optional
                // `&block`). The expected type is irrelevant here (the result
                // is discarded), so pass `Unit`, which never suppresses the
                // rewrite. Pin: 909_block_defined_and_yield_value (the
                // blockless `render` call between other statements).
                self.auto_call_fn_reference(&Ty::Unit, expr);
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

    /// Type-directed auto-call of a bare function reference.
    ///
    /// When `val` is a bare reference (`VarRef`) to a named function or
    /// method and the expected type at this position is **not** a function
    /// type, rewrite `val` in place into a zero-argument call. A `Fn`-typed
    /// `expected` (annotation or `Fn`-typed parameter) suppresses the
    /// rewrite — the function is referenced, not called.
    ///
    /// A reference to a function that *requires* arguments cannot be
    /// auto-called with zero args; that is reported as E0726, which names
    /// both escape routes (call it, or annotate a `Fn` type to reference it).
    ///
    /// Mirrors `auto_wrap_option_some` / `coerce_array_literal_to_fixed`:
    /// a contextual rewrite applied at known value positions (let binding,
    /// function return, call arguments, branches).
    ///
    /// See `docs/superpowers/specs/2026-05-29-auto-call-fn-references-design.md`.
    pub(super) fn auto_call_fn_reference(&mut self, expected: &Ty, val: &mut HirExpr) {
        // A `Fn`-typed context references the function without calling it.
        // (Only a *concrete* function type suppresses; an unconstrained
        // inference variable — e.g. a `let` with no annotation — defaults
        // to auto-call, the Ruby-style behaviour.)
        let expected_resolved = self.ctx.resolve(expected);
        if matches!(
            expected_resolved,
            Ty::Fn { .. } | Ty::FnMut { .. } | Ty::FnOnce { .. }
        ) {
            return;
        }
        // Descend into structural tails so return-position and branch
        // positions see the same context, mirroring auto_wrap_option_some:
        // the value assigned to the slot is the block tail / each branch.
        match &mut val.kind {
            HirExprKind::Block(_, Some(tail)) => {
                self.auto_call_fn_reference(&expected_resolved, tail);
                val.ty = self.ctx.resolve(&tail.ty);
                return;
            }
            HirExprKind::If {
                then_branch,
                else_branch,
                ..
            } => {
                self.auto_call_fn_reference(&expected_resolved, then_branch);
                if let Some(e) = else_branch {
                    self.auto_call_fn_reference(&expected_resolved, e);
                }
                return;
            }
            HirExprKind::Match { arms, .. } => {
                for arm in arms.iter_mut() {
                    self.auto_call_fn_reference(&expected_resolved, &mut arm.body);
                }
                return;
            }
            _ => {}
        }
        // Only a bare reference to a *named* function/method auto-calls.
        let def_id = match &val.kind {
            HirExprKind::VarRef(def_id) => *def_id,
            _ => return,
        };
        // The value must currently hold a function type. Guards against a
        // VarRef whose slot was already rewritten, and against references
        // to non-function definitions.
        let val_ty = self.ctx.resolve(&val.ty);
        if !matches!(val_ty, Ty::Fn { .. } | Ty::FnMut { .. } | Ty::FnOnce { .. }) {
            return;
        }
        let (name, signature) = match self.symbols.get(def_id) {
            Some(def) => {
                let name = def.name.clone();
                match &def.kind {
                    crate::resolve::symbols::DefKind::Function { signature }
                    | crate::resolve::symbols::DefKind::Method { signature, .. } => {
                        (name, signature.clone())
                    }
                    _ => return,
                }
            }
            None => return,
        };
        // Parameters with a default value are optional and need not be
        // supplied at a zero-argument auto-call. This includes the
        // optional `&block` slot (Ruby-block-semantics ADR D5: a `nil`
        // default makes the block optional), so `render` — whose only
        // parameter is its block — auto-calls blocklessly.
        let required_params = signature
            .params
            .iter()
            .filter(|p| p.default.is_none())
            .count();
        if required_params == 0 {
            // No required args: rewrite into a call so the value takes the
            // function's return type. The resolver's `append_default_args`
            // only runs on explicit `Call` AST nodes — a bare-identifier
            // auto-call never went through it — so materialize each param's
            // DEFAULT value here (each param is optional, since required==0):
            // the optional `&block` slot's `nil` becomes a null sentinel
            // (ADR D1), and an ordinary `x: Int = 5` default becomes `5` —
            // NOT a blanket null. A default that can't be lowered yields a
            // null fallback of the param type.
            let param_specs: Vec<(Option<crate::parser::ast::Expr>, Ty)> = signature
                .params
                .iter()
                .map(|p| (p.default.clone(), self.ctx.resolve(&p.ty)))
                .collect();
            let mut args: Vec<HirExpr> = Vec::with_capacity(param_specs.len());
            for (default, param_ty) in param_specs {
                let hir = default
                    .and_then(|d| self.default_ast_to_hir(&d, &param_ty))
                    .unwrap_or_else(|| HirExpr {
                        kind: HirExprKind::NullLiteral,
                        ty: param_ty.clone(),
                        span: val.span.clone(),
                    });
                args.push(hir);
            }
            let ret = self.wrap_async_return(&signature);
            val.kind = HirExprKind::FnCall {
                callee: def_id,
                callee_name: name,
                args,
            };
            val.ty = ret;
        } else {
            // Requires arguments — cannot auto-call. Name both escape routes.
            self.diagnostics.push(crate::diagnostics::Diagnostic::error_with_code(
                format!(
                    "`{name}` is a function that needs {} argument{}; call it like `{name}(...)`, or annotate a `Fn` type to reference it without calling (e.g. `let f: Fn(...) -> ... = {name}`)",
                    required_params,
                    if required_params == 1 { "" } else { "s" },
                ),
                val.span.clone(),
                "E0726",
            ));
            // Mark the slot as Error so the subsequent unify against a
            // concrete annotation doesn't emit a second, cascading
            // type-mismatch diagnostic for the same expression.
            val.ty = Ty::Error;
        }
    }

    /// "Drop Some" sugar: when an Option-typed slot receives a bare
    /// value of the inner type, rewrite the HIR expression in place
    /// from `<expr>` to `Option::Some(<expr>)`. Mirrors how Crystal /
    /// Sorbet treat `T?` — the user never writes `Some(...)` again
    /// for construction. `nil` keeps its existing polymorphic
    /// behaviour (MIR's NullLiteral arm already lowers it to a
    /// tagged-None enum when the slot is `Option[T]`).
    ///
    /// Touched by: let-binding RHS, function-body tail expression.
    /// Function arguments are NOT yet rewritten here — callers
    /// should write `Some(x)` explicitly at call sites for now;
    /// sweep planned.
    pub(super) fn auto_wrap_option_some(&mut self, expected: &Ty, val: &mut HirExpr) {
        let expected_resolved = self.ctx.resolve(expected);
        let inner = match &expected_resolved {
            Ty::Option(inner) => self.ctx.resolve(inner),
            _ => return,
        };
        // Recurse into structural expressions whose tail values
        // are what end up assigned to the Option slot: if/else
        // branches, blocks, and match arms. Without this descent,
        // an `if x then nil else id * 2` wraps as a WHOLE — the
        // `nil` branch then evaluates to Int 0 (via NullLiteral's
        // non-Option fallback) and gets wrapped as `Some(0)`, not
        // `None`. Descending lets each branch see the Option
        // context separately so `nil → None` and `Int → Some(Int)`
        // each fire in their own arm.
        match &mut val.kind {
            HirExprKind::If {
                then_branch,
                else_branch,
                ..
            } => {
                self.auto_wrap_option_some(&expected_resolved, then_branch);
                if let Some(e) = else_branch {
                    self.auto_wrap_option_some(&expected_resolved, e);
                }
                val.ty = expected_resolved.clone();
                return;
            }
            HirExprKind::Block(_, Some(tail)) => {
                self.auto_wrap_option_some(&expected_resolved, tail);
                val.ty = expected_resolved.clone();
                return;
            }
            HirExprKind::Match { arms, .. } => {
                for arm in arms.iter_mut() {
                    self.auto_wrap_option_some(&expected_resolved, &mut arm.body);
                }
                val.ty = expected_resolved.clone();
                return;
            }
            _ => {}
        }
        let found = self.ctx.resolve(&val.ty);
        if matches!(found, Ty::Option(_)) {
            return;
        }
        if matches!(val.kind, HirExprKind::NullLiteral) {
            // `nil` already lowers to a tagged-None enum in MIR when
            // val.ty is Option; just pin the type so downstream sees
            // Option, not the original Infer var.
            val.ty = expected_resolved;
            return;
        }
        if !crate::typeck::unify::can_coerce(&found, &inner, self.ctx) {
            return;
        }
        let option_def_id = match self.find_option_enum_def_id() {
            Some(id) => id,
            None => return,
        };
        let original_kind = std::mem::replace(&mut val.kind, HirExprKind::UnitLiteral);
        let original_ty = std::mem::replace(&mut val.ty, Ty::Unit);
        let inner_expr = HirExpr {
            kind: original_kind,
            ty: original_ty,
            span: val.span.clone(),
        };
        val.kind = HirExprKind::EnumVariant {
            type_def: option_def_id,
            type_name: "Option".to_string(),
            variant_name: "Some".to_string(),
            variant_idx: 1,
            fields: vec![("0".to_string(), inner_expr)],
        };
        val.ty = Ty::Option(Box::new(inner));
    }

    fn find_option_enum_def_id(&self) -> Option<crate::hir::nodes::DefId> {
        // Bootstrap merge's namespace-anchor mode re-attaches class-body
        // lib decls onto the Option Enum DefId originally registered by
        // `register_option`. In some bootstrap orderings the kind in
        // the symbol table ends up `Class` (anchor mutation), not
        // `Enum`. We don't care which — we just need the DefId so MIR
        // can dispatch EnumVariant lowering by type_name + variant_idx.
        self.symbols
            .iter()
            .find(|d| d.name == "Option")
            .map(|d| d.id)
    }

    pub(super) fn error(&mut self, message: String, span: &Span) {
        self.diagnostics
            .push(Diagnostic::error(message, span.clone()));
    }

    /// Q13: `obj.foo?` lexes the trailing `?` into the member NAME (Ruby
    /// predicate names like `empty?` are legal), so when such a name resolves
    /// to no field/method the user most likely meant the try-operator or
    /// safe navigation. Return a hint distinguishing the three Ruby forms, or
    /// an empty string when the name has no `?` suffix. Appended to the
    /// "no field/method" message (the typeck `Diagnostic` has no help slot).
    pub(super) fn predicate_suffix_hint(name: &str) -> String {
        match name.strip_suffix('?') {
            Some(base) if !base.is_empty() => format!(
                " — note: `?` is part of a predicate method name here; \
                 for the try-operator write `{base}()?`, and for safe \
                 navigation use Ruby's `&.` (e.g. `x&.{base}`)"
            ),
            _ => String::new(),
        }
    }

    /// Returns `true` if the type is acceptable as an argument to `puts`,
    /// `eputs`, or `print`.  Strings in any common form (`String`,
    /// `&String`) qualify, as do zero-arg functions that return such a type
    /// (MIR auto-invokes them). `Infer`, `Error`, and `Never` are permitted
    /// to avoid cascading diagnostics.
    pub(super) fn is_puts_compatible(ty: &Ty) -> bool {
        match ty {
            Ty::String | Ty::Infer(_) | Ty::Error | Ty::Never => true,
            Ty::Ref(inner)
            | Ty::RefMut(inner)
            | Ty::RefLifetime(_, inner)
            | Ty::RefMutLifetime(_, inner) => Self::is_puts_compatible(inner),
            Ty::Fn { params, ret } if params.is_empty() => Self::is_puts_compatible(ret),
            _ => false,
        }
    }

    pub(super) fn type_error(&mut self, err: TypeError) {
        self.diagnostics
            .push(Diagnostic::error(err.message, err.span));
    }
}
