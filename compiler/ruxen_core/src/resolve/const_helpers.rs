//! Free helper functions extracted from `resolve/mod.rs` to keep the
//! file under 5K lines:
//!   - lower a `where` const predicate (`ast::ConstPredicate`) into the
//!     HIR shape that gets evaluated at every `Ty::Class` instantiation;
//!   - evaluate a lowered predicate against a binding map;
//!   - lower an `ast::Expr` to a `ConstExpr` (the v1 const language);
//!   - check whether a lowered `ConstExpr` carries the `Error` marker;
//!   - validate `const N: TY` parameter types (E0705);
//!   - validate a hash-key type (`HashMap[K, V]`, `HashSet[T]`);
//!   - look up a class/struct/enum definition by `Ty`.
//!
//! All of these are pure helpers — they take their `Resolver` state
//! through explicit parameters so they don't need to live inside
//! `impl Resolver`.

use crate::hir::types::Ty;
use crate::parser::ast;
use crate::resolve::symbols::{DefKind, Definition, SymbolTable};

/// E0706 "predicate cannot be satisfied" rather than a silent
/// no-op.
pub(super) fn lower_const_predicate(
    pred: &ast::ConstPredicate,
) -> crate::resolve::symbols::HirConstPredicate {
    use crate::resolve::symbols::{ConstPredOp, HirConstPredicate};
    let sentinel = || HirConstPredicate {
        lhs: crate::hir::types::ConstExpr::Lit(0),
        op: ConstPredOp::Eq,
        rhs: crate::hir::types::ConstExpr::Lit(1),
        span: pred.span.clone(),
    };
    match &pred.expr.as_ref().kind {
        ast::ExprKind::BinaryOp { left, op, right } => {
            let pred_op = match op {
                ast::BinOp::Eq => ConstPredOp::Eq,
                ast::BinOp::NotEq => ConstPredOp::Ne,
                ast::BinOp::Lt => ConstPredOp::Lt,
                ast::BinOp::LtEq => ConstPredOp::Le,
                ast::BinOp::Gt => ConstPredOp::Gt,
                ast::BinOp::GtEq => ConstPredOp::Ge,
                _ => return sentinel(),
            };
            HirConstPredicate {
                lhs: lower_const_expr_from_expr(left),
                op: pred_op,
                rhs: lower_const_expr_from_expr(right),
                span: pred.span.clone(),
            }
        }
        _ => sentinel(),
    }
}

/// T2.02 S9: evaluate a lowered predicate against a binding map.
/// Returns `Some(true)` / `Some(false)` when both sides eval cleanly;
/// `None` when an unresolved param (or other eval failure) means
/// "we can't tell yet" — caller skips the check in that case.
pub(super) fn eval_const_predicate(
    pred: &crate::resolve::symbols::HirConstPredicate,
    bindings: &std::collections::HashMap<String, u64>,
) -> Option<bool> {
    use crate::resolve::symbols::ConstPredOp;
    let lhs = pred.lhs.eval(bindings).ok()?;
    let rhs = pred.rhs.eval(bindings).ok()?;
    Some(match pred.op {
        ConstPredOp::Eq => lhs == rhs,
        ConstPredOp::Ne => lhs != rhs,
        ConstPredOp::Lt => lhs < rhs,
        ConstPredOp::Le => lhs <= rhs,
        ConstPredOp::Gt => lhs > rhs,
        ConstPredOp::Ge => lhs >= rhs,
    })
}

pub(super) fn lower_const_expr_from_expr(expr: &ast::Expr) -> crate::hir::types::ConstExpr {
    use crate::hir::types::{ConstExpr, ConstOp};
    match &expr.kind {
        ast::ExprKind::IntLiteral(v, _) => ConstExpr::Lit(*v as u64),
        ast::ExprKind::Identifier(name) => ConstExpr::Param(name.clone()),
        ast::ExprKind::BinaryOp { left, op, right } => {
            let const_op = match op {
                ast::BinOp::Add => ConstOp::Add,
                ast::BinOp::Sub => ConstOp::Sub,
                ast::BinOp::Mul => ConstOp::Mul,
                ast::BinOp::Div => ConstOp::Div,
                _ => return ConstExpr::Error,
            };
            ConstExpr::Op(
                Box::new(lower_const_expr_from_expr(left)),
                const_op,
                Box::new(lower_const_expr_from_expr(right)),
            )
        }
        _ => ConstExpr::Error,
    }
}

/// T2.02 §B8 (E0702 plumbing): does this `ConstExpr` tree contain
/// any `Error` marker?  The marker is what
/// `lower_const_expr_from_expr` emits for AST shapes the v1 const
/// language doesn't support — a single hit anywhere in the tree is
/// enough to flag the whole construction site.
pub(super) fn contains_const_expr_error(expr: &crate::hir::types::ConstExpr) -> bool {
    use crate::hir::types::ConstExpr;
    match expr {
        ConstExpr::Error => true,
        ConstExpr::Lit(_) | ConstExpr::Param(_) => false,
        ConstExpr::Op(a, _, b) => contains_const_expr_error(a) || contains_const_expr_error(b),
    }
}

/// T2.02 §B8 (E0705): valid types for a `const N: TY` parameter.
///
/// Accepts every integer width (`Int`, `Int8`..`Int64`, `UInt8`..`UInt64`,
/// `USize`, `ISize`) and `Bool`.  Rejects floats, strings, chars, units,
/// references, tuples, arrays, classes, traits, and every other
/// shape — those are spec non-goals (NG2 / NG3 / OQ-3).
///
/// `Ty::Error` is treated as valid so a stand-alone E0705 doesn't fire
/// on top of whatever earlier diagnostic produced the `Error` placeholder.
pub(super) fn is_valid_const_param_ty(ty: &Ty) -> bool {
    if matches!(ty, Ty::Error) {
        return true;
    }
    ty.is_integer() || matches!(ty, Ty::Bool)
}

pub(super) fn ty_is_valid_hash_key(
    ty: &Ty,
    symbols: &crate::resolve::symbols::SymbolTable,
) -> bool {
    use crate::resolve::symbols::ty_has_derive_trait;
    if ty.is_integer() || ty.is_float() {
        return true;
    }
    if matches!(
        ty,
        Ty::Bool | Ty::Char | Ty::Unit | Ty::String | Ty::Str | Ty::Never
    ) {
        return true;
    }
    match ty {
        // Compound containers are NOT Hash even when their elements are.
        // Vec/HashMap/HashSet have interior heap pointers whose hash would
        // not be stable across allocations; v1 chooses not to derive a
        // structural hash for them.
        Ty::Array(_) | Ty::Set(_) | Ty::Map(_, _) => false,
        // Tuples / arrays / Option / Result : recurse — Hash if every
        // component is Hash.
        Ty::FixedArray(inner, _) | Ty::Option(inner) => ty_is_valid_hash_key(inner, symbols),
        Ty::Result(a, b) => ty_is_valid_hash_key(a, symbols) && ty_is_valid_hash_key(b, symbols),
        Ty::Tuple(elems) => elems.iter().all(|e| ty_is_valid_hash_key(e, symbols)),
        Ty::Ref(inner)
        | Ty::RefMut(inner)
        | Ty::RefLifetime(_, inner)
        | Ty::RefMutLifetime(_, inner) => ty_is_valid_hash_key(inner, symbols),
        Ty::Alias { target, .. } => ty_is_valid_hash_key(target, symbols),
        Ty::Newtype { inner, .. } => ty_is_valid_hash_key(inner, symbols),
        Ty::Struct { .. } | Ty::Class { .. } | Ty::Enum { .. } => {
            ty_has_derive_trait(ty, symbols, "Hash") || ty_has_derive_trait(ty, symbols, "Hashable")
        }
        // Type-vars / param types: assume satisfiable (typeck unifies later).
        // Returning true here keeps generic code (e.g. `def f[K]`) compiling;
        // the actual Hash bound check on call sites is enforced via trait
        // bounds elsewhere.
        Ty::TypeParam { .. } | Ty::Infer(_) | Ty::Error => true,
        _ => false,
    }
}

pub(super) fn nominal_type_definition_mut<'a>(
    target_ty: &Ty,
    symbols: &'a mut SymbolTable,
) -> Option<&'a mut Definition> {
    let name = match target_ty {
        Ty::Class { name, .. } | Ty::Struct { name, .. } | Ty::Enum { name, .. } => name,
        _ => return None,
    };

    let def_id = symbols
        .iter()
        .find(|def| {
            def.name == *name
                && matches!(
                    def.kind,
                    DefKind::Class { .. } | DefKind::Struct { .. } | DefKind::Enum { .. }
                )
        })
        .map(|def| def.id)?;

    symbols.get_mut(def_id)
}
