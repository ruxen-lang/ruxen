//! The places in a body where MIR lowering makes a decision from a static
//! type, and the type helpers the instance engine reads them with.

use crate::hir::nodes::*;
use crate::hir::types::Ty;
use crate::hir::visit::{walk_expr, walk_func, Visit};
use std::collections::HashMap;

/// A site in a body where MIR lowering makes a decision from a static type:
/// a type it lays out, a call (whose callee symbol is chosen from the
/// callee/receiver) or an operand of one of the type-directed lowerings.
pub(super) enum Site<'a> {
    /// Any type the shared HIR walk reaches (parameter, return, binding,
    /// expression and constructor types).
    Type(&'a Ty),
    Function(&'a str, &'a [HirExpr]),
    /// A `MethodCall` with the method typeck resolved, its receiver type,
    /// its arguments and its trailing block.
    Method(DefId, &'a Ty, &'a [HirExpr], Option<&'a HirExpr>),
    /// Any receiver-bearing call — `recv.name(...)` (a `MethodCall`) or the
    /// paren-less `recv.name` shorthand (a `FieldAccess`, which MIR lowers as
    /// a call in `field_access.rs`), or the left operand of an operator a
    /// builtin head lowers to its `.rx` method (`Array#==`). Carries the
    /// receiver's type and the source-level method name.
    Receiver(&'a Ty, &'a str),
    /// A binary-operator operand: `binops.rs` selects the operation from the
    /// operand types.
    Operand(&'a Ty),
    /// A string-interpolation part: `interpolation.rs` selects a
    /// `{Type}_fmt` display helper from the part's type.
    Display(&'a Ty),
}

/// Report every site of `func` (its parameter and return types included).
pub(super) fn walk_func_sites(func: &HirFuncDef, f: &mut impl FnMut(Site<'_>)) {
    walk_func(&mut Sites { f }, func);
}

/// Report every site of `expr`.
pub(super) fn walk_expr_sites(expr: &HirExpr, f: &mut impl FnMut(Site<'_>)) {
    Sites { f }.visit_expr(expr);
}

struct Sites<'f, F> {
    f: &'f mut F,
}

impl<F: FnMut(Site<'_>)> Visit for Sites<'_, F> {
    fn visit_ty(&mut self, ty: &Ty) {
        (self.f)(Site::Type(ty));
    }

    fn visit_expr(&mut self, expr: &HirExpr) {
        let f = &mut *self.f;
        match &expr.kind {
            HirExprKind::FnCall {
                callee_name, args, ..
            } => f(Site::Function(callee_name, args)),
            HirExprKind::FieldAccess {
                object, field_name, ..
            } => f(Site::Receiver(&object.ty, field_name)),
            HirExprKind::MethodCall {
                object,
                method,
                method_name,
                args,
                block,
                ..
            } => {
                f(Site::Method(*method, &object.ty, args, block.as_deref()));
                f(Site::Receiver(&object.ty, method_name));
            }
            HirExprKind::BinaryOp { op, left, right } => {
                if let Some(method) = op.method_name() {
                    f(Site::Receiver(&left.ty, method));
                }
                f(Site::Operand(&left.ty));
                f(Site::Operand(&right.ty));
            }
            HirExprKind::Interpolation { parts } => {
                for p in parts {
                    if let HirInterpolationPart::Expr { expr: e, .. } = p {
                        f(Site::Display(&e.ty));
                    }
                }
            }
            _ => {}
        }
        walk_expr(self, expr);
    }
}

/// True when `ty`, peeled of references, IS one of `params` — a value of
/// the bare element type, whose MIR lowering is type-directed.
pub(super) fn is_bare_type_param(ty: &Ty, params: &[String]) -> bool {
    matches!(ty.peel_refs(), Ty::TypeParam { name, .. } if params.iter().any(|p| p == name))
}

/// True when `ty` mentions one of `params` anywhere.
pub(super) fn mentions_type_param(ty: &Ty, params: &[String]) -> bool {
    ty.any(&mut |t| matches!(t, Ty::TypeParam { name, .. } if params.contains(name)))
}

/// True when `ty` contains no unresolved type: no
/// `TypeParam` or `Infer` anywhere. Only such instantiations are recorded.
pub(super) fn is_fully_concrete(ty: &Ty) -> bool {
    !ty.any(&mut |t| matches!(t, Ty::TypeParam { .. } | Ty::Infer(_)))
}

/// Bind the type parameters `decl` names against the concrete `actual`
/// type at an instance site, through the one shared binder
/// ([`Ty::bind_type_params`]): references bind as written, so `T` against
/// `&String` is `T = &String`. Only fully concrete types bind; the caller
/// gates on "every param bound".
pub(in crate::mir::lower) fn unify_collect_type_params(
    decl: &Ty,
    actual: &Ty,
    out: &mut HashMap<String, Ty>,
) {
    decl.bind_type_params(actual, &|_| true, &is_fully_concrete, out);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_param_detection_separates_bare_from_nested() {
        let params = vec!["T".to_string()];
        let bare = Ty::TypeParam {
            name: "T".to_string(),
            bounds: vec![],
        };
        let nested = Ty::Array(Box::new(bare.clone()));
        // A method-call RECEIVER only needs specializing when it IS the
        // element; `self.push(x)` (receiver `Array[T]`) does not.
        assert!(is_bare_type_param(&bare, &params));
        assert!(is_bare_type_param(
            &Ty::Ref(Box::new(bare.clone())),
            &params
        ));
        assert!(!is_bare_type_param(&nested, &params));
        // A binary OPERAND dispatches on the whole operand type, so a
        // `T`-carrying container counts.
        assert!(mentions_type_param(&nested, &params));
        assert!(!mentions_type_param(&Ty::Array(Box::new(Ty::Int)), &params));
    }
}
