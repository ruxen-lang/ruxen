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
pub(super) use helpers::{is_bufio_inner_supported, is_iter_sum_compatible};

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
                // TODO (rondo_v1_blockers.md B14): `Option[A]` /
                // `Result[A, E]` payload coercion. The typeck side of
                // this is one structural retry per container variant
                // (Option(inner) ↔ Option(inner), Result(ok,err) ↔
                // Result(ok,err) → `unify_or_coerce` on each inner).
                // But without an HIR-level expression rewrite at the
                // value site, MIR still sees the raw `&str` pointer
                // where a `String` is expected — runtime segfaults at
                // the first scope-exit `riven_string_free`. The proper
                // landing requires the same path that makes plain
                // `let s: String = "lit"` work, surfaced into the
                // container payload position. Tracking; the typeck-
                // only patch was attempted + reverted in this session.
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

    pub(super) fn error(&mut self, message: String, span: &Span) {
        self.diagnostics
            .push(Diagnostic::error(message, span.clone()));
    }

    /// Returns `true` if the type is acceptable as an argument to `puts`,
    /// `eputs`, or `print`.  Strings in any common form (`String`, `&str`,
    /// `&String`, `&&str`) qualify, as do zero-arg functions that return
    /// such a type (MIR auto-invokes them). `Infer`, `Error`, and `Never`
    /// are permitted to avoid cascading diagnostics.
    pub(super) fn is_puts_compatible(ty: &Ty) -> bool {
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

    pub(super) fn type_error(&mut self, err: TypeError) {
        self.diagnostics
            .push(Diagnostic::error(err.message, err.span));
    }
}
